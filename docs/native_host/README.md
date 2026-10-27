# Native host

`hydra/src/native_host/` is a second implementation of the
`hydra_machine_hardware_t` bridge declared in `hydra/src/hydra_machine.h`,
beside `dosemu_host/`. It runs decompiled guest code with no emulator process
and no dosemu2 checkout: guest memory is one flat 1 MiB array
(`NATIVE_HOST_MEM_SIZE`), and everything the skeleton does not model fails loud
with a diagnostic instead of inventing a plausible value.

| File | Role |
| --- | --- |
| `native_host.c` | The hardware vtable: linear guest memory, port-device routing (8042 keyboard, 8253/8254 PIT), state save/restore, and the text-window mirror. |
| `hydra_backend.h` | The device HAL: one vtable (`hydra_backend_t`), the selective INT dispatch entry point, and the host-facing backend registry. |
| `backend_terminal.c` | Reference backend: ANSI text mode through a shadow buffer, keyboard through stdin, 18.2 Hz ticks from `CLOCK_MONOTONIC`. |
| `backend_host.c` | The `hydra_user_int` hook the core resolves with `dlsym`, plus the active-backend registry. |
| `int_dispatch.c` | The two-level INT table (vector → AH handler). Unhandled vectors and unhandled services abort. |

`test_native_host.c` is the single test binary; the HAL/INT tests live in
`backend_selftest.c` and are folded into its exit status.

## What the guest can see

Text output reaches the terminal by two routes, and both are required: most
DOS programs use the BIOS, and plenty of them write the text window directly.

- **BIOS services.** `INT 10h` text mode: `AH=00h` (set mode), `AH=02h`/`03h`
  (set/get cursor), `AH=06h`/`07h` (scroll up/down), `AH=08h` (read character
  and attribute), `AH=09h`/`0Ah` (write character with/without attribute),
  `AH=0Eh` (teletype), and `AH=0Fh` (video state); `INT 21h` `AH=02h`,
  `AH=09h`, `AH=4Ch`. Graphics modes are refused by the backend, so `AH=00h`
  fails closed with a diagnostic rather than rendering garbage.

  All of these address the active video page, which is page 0; a call naming
  another page fails loud instead of drawing somewhere the guest cannot see.
  Cursor positions are zero-based cells of the current geometry, and a move is
  a `present()` change. A cursor position or scroll window outside the
  geometry fails loud instead of being clipped. Scrolling takes an inclusive
  window, treats `AL=0` (or at least the window height) as "clear", uses `BH`
  as the blank attribute, and leaves the cursor where it was; the character
  services also write at the cursor without moving it.

  These services draw into the backend's shadow frame, not into guest video
  memory. Direct stores remain the authoritative path for guest read-back;
  BIOS output is only what is rendered. Keeping BIOS output in guest memory —
  so a `state_save`/`state_restore` after a BIOS-only screen is faithful — is a
  separate architectural change.

- **Direct video memory.** Stores into the text window are mirrored into the
  backend's shadow buffer: `B8000h` for modes `00h`-`03h`, `B0000h` for MDA
  mode `07h`. Guest memory stays authoritative — the mirror is a projection,
  so reading back a byte the guest wrote returns the guest's byte even when no
  backend renders it. A store outside the displayed window (page 1 and above,
  or `B8000h` while in MDA mode) lands in guest memory and renders nowhere,
  which is what the hardware address space does too.

  A word store is routed per byte, because on 8086-class hardware it is two
  bus cycles. The two halves can straddle an end of the window — the word at
  `B8F9Fh` in a 80x25 mode puts its low byte in the last cell's attribute and
  its high byte past the end — so each half is clipped on its own; the half
  that is in the window renders and the other does not.

  The projection never fails. It runs after the bytes are committed to guest
  memory, and rendering must not be the thing that ends a run, so a store that
  arrives before a backend has been selected — or with a backend that renders
  no text — simply lands in memory and renders nowhere.

  Cost: the out-of-range gate is two comparisons, and each in-window store
  re-reads the geometry through the vtable. A full 80x25 redraw is 2,000
  stores and measures ~31 µs at `-O2` (15.4 ns per store, 0.2% of a 70 Hz
  frame), so the geometry is not cached. A cache would need a generation
  counter bumped on `set_video_mode`, because the window moves with the mode.

`present()` is the frame boundary. It repaints only when something was drawn
since the last call, and it writes nothing when stdout is not a terminal, so
batch and CI runs stay clean. A cursor move counts as a change and is tracked
apart from the cells, so a guest that only repositions the cursor — CR/LF to a
prompt, backspace over an echoed key — still moves the terminal cursor without
forcing a repaint. Two things drive it:

- the terminal backend itself, immediately before `read_key()` blocks. A guest
  that draws a title screen or menu and then waits for a key must have that
  screen on the terminal while it waits;
- the port driver, through `hydra_native_backend_present()`, once per frame.

A snapshot restores the display too, for text the guest wrote directly.
`state_restore()` loads the whole image with a single read that never passes
through the per-store mirror, so it re-projects the text window afterwards;
without that the screen would keep showing the pre-restore frame while the
guest reads the restored bytes.

## SDL2: a new dependency, not a reuse

Issue #95 lists an SDL2 backend as "reusing dis86's already-vendored optional
dependency". That premise does not hold, and this note records why so the work
is not attempted from the wrong starting point.

**The vendored dependency is a Rust crate.** `dis86/Cargo.toml` declares

```toml
[features]
sdl = ["dep:sdl2"]

[dependencies]
sdl2 = { version = "0.38", features = ["use-pkgconfig", "unsafe_textures"], optional = true } ## Used for the interactive emu86 frontend only
```

and `required-features = ["sdl", "emu86"]`. A C vtable implementation in
`hydra/` cannot link a Rust crate; it needs the C library that crate wraps
(`libsdl2`), found through `pkg-config`.

**Nothing else in the tree supplies it.** `hydra/meson.build` is a plain
`project('hydra', 'c')` with no dependency list; there is no
`meson_options.txt`. No workflow installs SDL2 — `dosemu-host-runtime.yml`
installs dosemu2/FDPP, and `parity-fixture.yml` installs `nasm` plus the same.
`just check` compiles `hydra/src/native_host/*.c` with a bare `cc` and no extra
libraries, which is why the HAL note in `hydra_backend.h` insists that every
backend must build without new deps.

**Decision (2026-10-20).** SDL2 stays out of the default lane until a Tier-1
target needs graphics. When it does:

- it enters as an opt-in meson dependency — `dependency('sdl2',
  required : get_option('sdl2'))` in `meson_options.txt`, default `disabled` —
  and `backend_sdl2.c` joins the library only when it is found;
- `hydra_backend_by_name("sdl2")` keeps returning `NULL` in a lane built
  without it (a test already asserts that), so a misconfigured port fails with
  "no backend named 'sdl2'" instead of a link error;
- `just check` must stay dependency-free, so the new backend cannot be reached
  from the host-independent lane.

Consequences: the "SDL2 backend" deliverable in #95 is a new optional C
dependency plus a CI lane that installs it, not a reuse. Tier-1's graphics
criterion is blocked on that decision, not on the HAL plumbing: the vtable,
the fail-closed mode check and the dispatch table are already in place.
