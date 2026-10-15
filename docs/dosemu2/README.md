# dosemu2 hosting

Hydra hosts on the **stock, unmodified** dosemu2 binary. No rebuild, no patching, no
fork. Hydra runs as an external process that drives dosemu2 through its built-in
debugger protocol (dosdebug FIFOs) and maps guest memory via `/proc/<pid>/fd/`. The
design is in [`HOST_DESIGN.md`](HOST_DESIGN.md).

> **Status:** Hydra's stock-dosemu2 host and opt-in overlay path are implemented.
> PR [#39](https://github.com/evaluating-cli/dis86/pull/39) was merged as
> `5eb5567`; its exact PR head, `b7ee70d`, passed both the
> [stock dosemu2 host integration](https://github.com/evaluating-cli/dis86/actions/runs/37386457601)
> and [Rust/shared-memory checks](https://github.com/evaluating-cli/dis86/actions/runs/37386457675)
> before merge. The runtime matrix covered MZ loading, repeated COM runs,
> HYDSNAP capture/restore, COM overlay page-in and paged/unpaged restore, and
> generated-metadata MZ/FBOV overlay dispatch. It also covered verified lowmem
> selection, guest-visible IF=0 restoration, persistent register+lowmem
> snapshots, nested callthrough, guest-owned raw-code scratch, and default-mode
> overlay rejection.
>
> Overlay hooks are available with the explicit `overlays=armed` host option.
> Without that option, the host still fails closed before guest execution.

dosemu2 (+ its simx86 CPU core and FDPP) is a **trusted third-party component**:
it is the runtime substrate for Hydra hybrid execution and the oracle used to run
original binaries in the native-recompile workflow. We neither validate, patch,
nor fork it; see the development direction in [`../../README.md`](../../README.md).

## How it works

1. **Launch stock dosemu2** with simx86 (`$_cpu_vm = "emulated"`, `$_cpuemu = (1)`),
   `$_mapping = "mapmshm"` (memfd lowmem), `$_hdimage = "+1"` (FreeDOS boot), and
   the target DOS program.
2. **Connect via dosdebug** — two FIFOs in `$XDG_RUNTIME_DIR/dosemu2/` (`dosemu.dbgin.<pid>`,
   `dosemu.dbgout.<pid>`), created automatically by the stock binary.
3. **Map guest memory** — enumerate the candidate dosemu2 lowmem memfds under
   `/proc/<dosemu_pid>/fd/`, cross-check them against independent dosdebug reads, and
   require exactly one matching backing before mapping it `MAP_SHARED`.
4. **Set breakpoints** at supported static function entry points (`bp ADDR`). Continue (`g`).
5. **On hit** — parse the register dump (including VIF-aware reconstruction of guest
   IF), dispatch the hook through the Hydra exec engine, write return registers, verify
   guest-visible FLAGS, and continue. Guest-opcode/raw-code requests are single-stepped
   until their magic return and use fresh, never-reused addresses inside an explicitly
   guest-owned reservation.
6. **Capture/restore** — clear tracked software breakpoints before memory imaging,
   persist registers plus the `0x110000` lowmem+HMA window, restore memory then CPU
   state, and re-arm breakpoints before guest execution resumes.

Function-level hooking is the right granularity for the supported decompilation path.
Instruction-level interception is used only for bounded raw-code/native→guest tracing.

## Evidence snapshot

**Current Hydra host:** exact behavioral candidate `3beac798...` passed both normal CI
and the stock-dosemu runtime workflow. The runtime suite includes the default overlay
fail-closed fixture, `test_host`, and the full function-hook driver with 21 hook
dispatches, 45 raw/native→guest runs, 45 returns, 66 redirects, CF preservation, IF=0
preservation, byte-for-byte raw scratch restoration, and a live dosemu2 process at
completion.

“Implemented,” “host-only tested,” and “exact-head runtime tested” are distinct claims.
Correctness smoke tests establish no performance claim.

## Document map

| Document | Content |
| --- | --- |
| [`HOST_DESIGN.md`](HOST_DESIGN.md) | Host design: external Hydra client via dosdebug + verified `/proc/pid/fd` mmap. |
| [`OVERLAY_DESIGN.md`](OVERLAY_DESIGN.md) | Opt-in overlay-hook support (`overlays=armed`), fail-closed by default. |
| [`TESTING.md`](TESTING.md) | Current hosting tests, build/run commands, and exact-head evidence. |
| [`HISTORY.md`](HISTORY.md) | One-page history of the retired ABI-v1 differential validator. |
