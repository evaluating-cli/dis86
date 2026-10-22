# Parity fixture (#96)

A standing, end-to-end parity lane: one DOS image (`fixture.asm`) runs twice —
byte-identically — through two independent stacks:

| lane   | engine | reader |
|--------|--------|--------|
| golden | stock dosemu2, free-run `-E fixture.exe`, `mapmshm` lowmem | `golden_capture.c` reads live guest memory |
| native | dis86 `--codegen-hydra` output on hydra + standalone `native_host` (no dosemu2) | `native_runner.c` reads the host's memory image |

Both lanes compare the same 80-byte snapshot: a 16-byte outbox header plus 64
reserved lane bytes. The header is written **last**, so its presence proves a
complete world; both readers scan for it instead of hardcoding a guest
address, so load-segment skew cannot leak into the diff.

## Contract

`fixture.asm` zero-fills its outbox, computes lanes 0..29 plus the lowering
lanes 30..38, writes the checksum (lane 39 = sum of lanes 0..38 mod 256), then
publishes magic `PHYPAR16`, version 3 and the lane reserve count. `outbox.h` is
the shared contract and known-answer table; `build.sh` asserts the `.asm`
layout constants match its defines. Expected lanes:

```
c0 c1 00 01 10 00 02 81 81 80 0b 2c 78 02 8c 02
34 34 12 34 20 01 78 12 34 34 7a 11 80 88 f8 08
04 e0 ff 23 56 cd ff fe ff ce 00
```

The fixture covers: ADC/SBB carry-in seeds, word carry, SHL/SAR/RCL/RCR/ROL/ROR,
REP MOVSB/STOSW/CMPSB under a `cld`-ed DF, byte/word aliasing, byte/word MUL,
IMUL, word DIV/IDIV (quotient and exact remainder), XCHG,
push/pop and one near call to a real second function. It issues no INTs and
reads no DOS-seeded state. The tail returns to a raw `jmp $` outside both
annotated functions; the native runner pre-releases the fixture's park
sentinel and seeds the near-return landing so it stops on a hook miss instead
of executing those bytes.

`annotations.py` declares **two** functions, both `ret="u16"` and
`flags="NEAR"`:

* `F_parity_main` `[0x0000, 0x01D9)`
* `F_parity_leaf` `[0x01D9, 0x01DD)` (near-call target)

Hydra hook lookup matches exact entry addresses and the call site lowers
through the generated callstub (`F_parity_leaf(m)`), so the leaf must be its
own registered hook. `NEAR` is required because bsl emission defaults to
`far`; a mismatch trips `ir_build.rs`'s call-mode assertion.

## Running

```sh
bash parity/build.sh     # Gate 1 (hooks, no UNIMPL) + native Gate 2 + trace Gate 3
bash parity/run_golden.sh # golden Gate 2 (needs dosemu2+fdpp) + compare
```

`build.sh` needs `nasm` and `cc`; it builds dis86 with cargo if needed. Golden
needs a stock dosemu2 with `mapmshm` and fdpp. The native compile links the
emitted code plus confgen appdata against `hydra/src/*.c` and
`hydra/src/native_host`, with no meson and no dosemu2.

`run_golden.sh` boots a private, headless session, waits up to
`PARITY_TIMEOUT` (default 90 s) for the outbox, verifies it against the
known-answer table, compares it with `native.lanes`, and then proves the
comparison is live by flipping one byte in a copy and requiring `cmp` to fail.

## Gate 3: the trace lane (issue #97)

The instrumented-execution tool has to be able to say what the machine did,
and its correctness has to ride on the anchored fixture rather than on
itself. So the fixture is run once more with the Hydra trace sink open
(`native_runner --trace`), and the observations the trace holds are checked
against the fixture's known layout:

- the entry function `F_parity_main` was dispatched at `0001:0000`;
- the call into the leaf `F_parity_leaf` was recorded at `0001:01d9` with
  `nargs=0`;
- the value the leaf returned was recorded at that same boundary.

The expected addresses are read from the generated `.bsl` (which comes from
`parity/annotations.py`) and shifted by the code load segment the native
runner uses, so the expectation moves with the annotations instead of being
a second copy of the layout.

The lane also carries a **mutation check**: a copy of the trace with the
leaf address tampered with must *fail* the same check. A green run then
means "the tool saw what the fixture says", not merely "the tool ran".

### Do not use `-H1` for the golden lane

dosemu2's `-H1` locks the guest before the bootsector until a dosdebug client
attaches. A free-running capture lane never attaches, so with `-H1` the guest
never boots and the outbox never appears. The earlier `-H1` + attach recipe is
only needed when poking/capturing pre-boot state.

## Native-host enablement note

`hydra_impl_nop/cld/std/cli/sti` used to execute one-instruction raw snippets
on the emulated CPU, which requires a guest-reserved raw-code region. That path
is retained whenever the host provides a region (stock dosemu2, so dispatch
accounting is unchanged). The standalone native host reserves none and has no
CPU to execute snippets, so these five operand-free flag ops fall back to
setting the mirrored flag register directly (propagated to the CPU via
`machine_to_regs` on hosts that have one). Port I/O and interrupt helpers
remain raw-code/host-seam operations and the fixture deliberately avoids them.

## Known dis86 lowering gaps (fixture avoids; follow-ups to file)

None outstanding: the former gaps (byte `MUL`/`IMUL` panics, the loud-only
`DIV`/`IDIV` remainder, and the byte-`SAR` sign loss) are now lowered exactly
and covered by fixture lanes 30..40, and the word `IDIV`/`IMUL` signed-cast
truncation is width-aware. A follow-up fixture version can add byte DIV/IDIV
lanes the same way.
