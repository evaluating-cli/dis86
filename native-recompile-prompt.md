# DOS binary native recompilation — autonomous execution prompt

> Copy-paste this whole file as the task prompt for a fresh agent session,
> filling in `<TARGET.EXE>` (path to the DOS binary) and `<TARGET_NAME>`
> (short lowercase name derived from its filename, e.g. `fcd` for
> `FCD.EXE`). That agent works solo until the task is done or proven
> impossible.

---

## 1. Mission

Recompile the DOS binary `<TARGET.EXE>` (gitignored — never commit or add
it) into a **working native Linux binary** (e.g.
`build/recompile-<TARGET_NAME>/<TARGET_NAME>`), covering the **whole
binary**, starting **from zero configuration**, using this repository's
decompiler (`dis86`) and hybrid runtime (`hydra`) as your tools.

**Victory = observable parity:** the native binary, given the same inputs as
the original `<TARGET.EXE>`, produces the same observable outputs
(stdout/files/screen effects/exit behavior — whatever this program's
observable surface turns out to be; see §6). Prove it with golden-vs-native
logs + diffs and repro commands.

**If it cannot be done,** do not just stop: exhaust every reasonable
workaround first, then deliver a best-effort report: what works, exact
blocker(s), what was tried, partial artifacts, and why no workaround exists
(§8). Partial progress with a precise blocker diagnosis counts as a result.

## 2. Target handoff + discovery

- The launcher provides the target file and filename. The binary is
  **opaque — treat it as unknown and discover it** (Phase 0 is mandatory).
  Record its size, MZ header fields, relocation table, entry point, initial
  stack, and segment layout in `FORENSICS.md` before doing anything else.
- There is **no existing `.bsl` config or annotation source** for it. You
  start from zero with `--analyze` + `confgen` (Phase 1).
- Repo `README.md` lists "re-compilable to equivalent binaries" as a
  non-goal. **This mission overrides that** for `<TARGET.EXE>`.
- `<TARGET.EXE>` and `build/` are gitignored (see `.gitignore`). **All** of
  your work — scratch, docs, harness, logs — lives under
  `build/recompile-<TARGET_NAME>/` (gitignored). Nothing you produce for this
  task is committed; the final report is delivered as session output plus files
  under `build/recompile-<TARGET_NAME>/docs/`.

## 3. Repository map (your tools)

| Path | What it is |
| --- | --- |
| `dis86/` | Decompiler (Rust): decode → analysis → SSA IR → opt → control flow → AST → C codegen. Also the `mzfile` companion binary. Build: `cd dis86 && cargo build` (or `just build-dis86`; both binaries land in `build/bin/`). |
| `bsl/` | Barebones Specification Language (config/annotation tables). |
| `confgen/generate.py` + `confgen/hydra/{annotations.py,gen/}` | Generate `.bsl` config / appdata headers from an annotation source: `PYTHONPATH=confgen python3 confgen/generate.py SRC --dis86-conf\|--appdata-hdr\|--appdata-src`. `SRC` must be an **importable Python source file** (give it a `.py` extension — `confgen/hydra/gen/load.py` loads it via `importlib` and will not accept an arbitrary extension such as `.src`). |
| `hydra/` | Hybrid runtime (C, Meson). **You may use Hydra headers/macros as a runtime library** in the final native binary. Current host = external dosdebug client in `hydra/src/dosemu_host/` (see `docs/dosemu2/README.md`). Build: `just build-hydra`. |
| `docs/` | 8086/DOS notes (`dos_int21h.txt`, `dos_interrupts.txt`, `dos_memory_map.txt`, `dos_psp.txt`, `mz_header.txt`), `docs/dosemu2/` (host design, `TESTING.md`, `SOURCES.md`), `docs/emu86/sst.md` (hardware-anchored validation ledger for emu86). |
| `just check` | Host-independent gate (confgen tests + `cargo test` + `cc -fsyntax-only` on hydra). **Keep it green throughout.** If `just` is not installed, run its underlying commands directly (`PYTHONPATH=confgen python3 confgen/test_*.py`, `cargo test --manifest-path dis86/Cargo.toml --locked --all-targets`, and the `cc -fsyntax-only` lines from the `justfile`). |

Key `dis86` invocations (run `build/bin/dis86 --help` for the full list;
substitute your generated `.bsl` for `X.bsl`):

```sh
build/bin/mzfile info <TARGET.EXE>                 # inspect MZ structure
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --analyze
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --name FN --emit-dis /tmp/x.dis
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --name FN --emit-ir-final /tmp/x.ir
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --name FN --emit-ast /tmp/x.ast
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --name FN --emit-code /tmp/x.c
build/bin/dis86 --config X.bsl --binary-exe <TARGET.EXE> --codeseg-name SEG \
  --emit-code out.c --codegen-hydra [--relocatable-seg HEX,...]
```

Note: whole-codeseg batch decompile **continues past per-function failures**
but exits nonzero if any function failed — read its stderr carefully and fix
each failure class at its root (config, opcode coverage, IR→AST gap) rather
than by deleting functions from scope.

### Known decompiler traps and their diagnostics (audit for these; report, don't hide)

This fork's `dis86` has known traps that surface on real DOS binaries. Detect
them early and account for them explicitly: where a bullet names a fixed
issue, follow its guidance (do not "work around" a non-issue); where it names
a live limitation, pick a workaround or a named exclusion with justification —
never a silent scope cut:

- **`INT` register setup is preserved — do not work around dropped setup.**
  `process_int` pins the live guest registers as writes ahead of the
  `INT(...)` dispatch, so `mov ah,<fn>` / `mov bx,<handle>` immediately
  before an `INT` survives DCE (issue #66, fixed). If registers look stale
  at an `INT` in emitted code, that is a new bug: report it with the
  disassembly + IR, don't build shims around assumed-dropped setup.
- **Branches to a target outside a function's declared `[start, end)` are
  dropped — but each dropped edge now warns.** The build prints a `WARN`
  naming the branch and its out-of-range target (issue #67, diagnostic
  added; the drop itself is by design). Treat every such warning as a
  range-split task: split ranges so every branch target lands inside a
  range, or route the tail through an in-range copy. Symptom if you ignore
  the warning: a function ends without a `return`/`goto`, or a body simply
  stops mid-flow.
- **Embedded data inside a suggested range aborts the linear decoder —
  and `--analyze` now flags the bad ranges for you.** `--analyze` ranges
  are reachability-based but decode is linear, so a runtime banner/string/
  table inside a range fails with `Unsupported or invalid instruction`.
  Each offending suggestion carries a `# UNDECODABLE GAP` diagnostic naming
  the exact failing address (issue #65): shorten the range at the named
  address and declare the data bytes in `text_section` (issue #65, fixed).
- **`adc`/`sbb`/`rcl`/`rcr`/`rol`/`ror` and `REP`-prefixed string ops are
  exact when their state is known — a loud marker means genuinely unknown
  state, not a missing opcode.** `ADC`/`SBB` chain on a known incoming
  carry, `RCL`/`RCR` lower exactly with a known carry-in, `ROL`/`ROR` are
  value-exact (flags clobbered), and `REP MOVS`/`STOS`/`LODS` model exactly
  (issue #69, fixed). If one of these still emits `UNIMPL`/`UNIMPL_FLAGS`,
  investigate the state at that point (unknown carry? possibly-set DF?) —
  don't blanket-exclude the opcode class from scope.
- **`--analyze` insists on at least one seeded function.** With an empty config
  it has nothing to scan from; seed your best guess at the entry first.

## 4. Autonomy grant

You have **unrestricted autonomy**: build, run, install tooling, create
configs/scripts, run dosemu2/emu86 probes, capture traces, and iterate freely
until done or impossible. Prefer small verifiable steps with recorded
evidence over big unverified leaps. Keep `just check` green; everything you
make stays under `build/recompile-<TARGET_NAME>/` — no commits are expected
for this task.

## 5. Required working layout (all gitignored under `build/`)

```text
build/recompile-<TARGET_NAME>/
  docs/
    FORENSICS.md      # Phase 0 findings: what <TARGET.EXE> is and does
    STUBS.md          # Phase 3 shim/stub log with fidelity claims
    RESULT.md         # final report or impossibility report
  annotations.py      # your annotation source (confgen input; must be importable)
  <TARGET_NAME>.bsl   # generated dis86 config
  appdata.h / appdata.c (if used)
  emitted/            # raw dis86 --emit-code outputs per codeseg
  native/             # native shims + main + Makefile/CMake/Meson bits
  <TARGET_NAME>       # final native binary
```

## 6. Phase 0 — forensics first (MANDATORY, before any batch decompile)

1. `build/bin/mzfile info <TARGET.EXE>`; `xxd`/`strings` survey; decode the MZ
   header and relocation table; record size, segments, entry, stack.
2. `build/bin/dis86 --config <minimal> --binary-exe <TARGET.EXE> --analyze`
   once you have a first config; before that, probe raw disassembly around
   the entry (`--start-addr`/`--end-addr` slices) and a few relocation
   targets.
3. Inventory the DOS/BIOS surface: `INT 10h/16h/21h`, file I/O, video mode,
   keyboard, timers, mouse, sound. Consult `docs/dos_*.txt` for semantics.
4. Write `build/recompile-<TARGET_NAME>/docs/FORENSICS.md`: what the program
   is, its inputs/outputs, its observable surface, memory/segment layout,
   interrupt usage, and any packer/overlay/self-modifying-code signs. If you
   find packing, encryption, or overlays, say so explicitly and adjust the
   plan — do not silently exclude code from scope.

## 7. Phases 1–3 — config → whole-binary decompile → native recompile

End-to-end loop: seed a minimal `.bsl` naming the entry → `--analyze` and read
the suggested `F( ... )` lines **including their diagnostics** (`# UNDECODABLE
GAP`, `# RET KIND UNKNOWN`, `# MIXED RETURN KINDS` pinpoint exactly which
ranges need auditing; see §3) → **audit every flagged range** (split out
embedded data and out-of-range branch targets) → write `annotations.py`
→ `confgen/generate.py … --dis86-conf` → whole-segment
`--codeseg-name … --codegen-hydra`. Repeat until the batch exits 0.

- **Phase 1 (config from zero):** author `annotations.py` (an importable
  Python source file, see §3), generate `<TARGET_NAME>.bsl` via
  `confgen/generate.py`, iterate with `--analyze`
  until function boundaries, code/data segments, entry/stub addresses, and
  data globals are coherent. Prefer manual annotation tables over heuristics
  that produce wrong code (repo principle).
- **Phase 2 (whole-binary batch):** decompile **every codeseg** with
  `--codeseg-name … --codegen-hydra` into
  `build/recompile-<TARGET_NAME>/emitted/`. Drive the failure count to zero
  by fixing root causes (missing annotations, unimplemented 8086 opcodes in
  `asm→ir`, IR→AST gaps, control-flow issues). The `--relocatable-seg`
  table is operator-audited: you type the absolute link-time segment values;
  they emit as `(CODE_START_SEG + seg)` for relocated execution. Record the
  exact table you used and why.
- **Phase 3 (native recompile):** link emitted C + Hydra-as-library +
  **your own native shims** for every DOS/BIOS interrupt and device access
  the program touches. Shim strategy is **your discretion** (faithful
  reimplementation vs. terminal equivalents), but every stub/shim must be
  logged in `build/recompile-<TARGET_NAME>/docs/STUBS.md` with its fidelity
  claim. **You choose the segment/relocation memory model** (e.g. linearized
  guest-memory array vs. preserved segment semantics) — document the choice
  and justify it. The final `build/recompile-<TARGET_NAME>/<TARGET_NAME>`
  must **not require dosemu2 at runtime** (dosemu2 and emu86 are oracle tools
  only).

## 8. Impossibility bar

Declare impossible only after all reasonable workarounds are exhausted
(extra annotations, opcode/IR/AST contributions to `dis86/`, CFG
restructuring, shim redesign, memory-model change). Blockers that count:
unimplemented 8086 semantics with no feasible lowering, IR→AST gaps with no
encoding, genuinely irreducible control flow, self-modifying/packed code
that defeats static decompilation, DOS/hardware surface that cannot be
shimmed natively. Report: what works, exact failing function(s)/address(es)
with disassembly + IR dumps, everything tried, and why each failed.

## 9. Hard constraints

1. `<TARGET.EXE>` stays local and gitignored. Never `git add`/`commit` it.
2. `just check` green at every stopping point.
3. No silent scope cuts: any excluded address range must be named and
   justified in `RESULT.md`.
4. Evidence over assertion: every parity/fidelity claim needs a log + diff +
   repro command.
5. Everything task-related lives under `build/recompile-<TARGET_NAME>/`. Do
   not create new committed paths for the task.

Begin with Phase 0. End with either a working native binary, or the §8
impossibility report. Good luck.
