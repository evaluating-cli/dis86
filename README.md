# Dis86

Dis86 is a decompiler for 16-bit real-mode x86 DOS binaries. This repository is the `evaluating-cli` fork of [xorvoid/dis86](https://github.com/xorvoid/dis86); the upstream project is unfinished and development continues here.

# Purpose

Dis86 has been built for doing reverse-engineering work such as analyzing and re-implementing old DOS video games from the early 1990s. The project is a work-in-progress and the development team makes no guarantees it will work or be useful out-of-the-box for any applications other than their own. Features and improvements are made on-demand as needed.

## Development direction

**The product is the decompiler**: translating 16-bit real-mode DOS binaries into
semantically-correct C, usable standalone and through [Hydra](hydra/README.md)
for the native-recompile workflow.
Everything else in the repository supports that.

| Component | Role |
| --- | --- |
| `dis86/` decompiler | **The product**: decode → analysis → SSA IR → optimization → control flow → AST → C codegen. |
| `hydra/` | Support runtime: links decompiled C with remaining x86-16 machine code for native recompiles. |
| dosemu2 (+ simx86, FDPP) | Trusted third-party component: runs the original binary as the parity oracle and hosts Hydra hybrid execution. We neither validate, patch, nor fork it. |
| `bsl/`, `confgen/` | Configuration/annotation tooling for the decompiler. |
| `dis86/src/emu86/` + SST harness | **Frozen offline tooling**: a retained 8086/286 interpreter and its SingleStepTests hardware-corpus harness. No development; no role in product validation; not built by the default lane (enable with `--features emu86`). |

**Validation.** Decompiler correctness rests on its in-repo unit/integration
tests plus end-to-end observable parity in the native-recompile workflow (same
inputs → same observable outputs, original vs native). Non-goals:
hardware-anchoring the runtime, proving emu86↔simx86 equivalence, and
developing a standalone emulator.

**Reuse before building.** Prefer prior art: the shared instruction decoder
(`dis86/src/asm/`), the vendored MOO crate, the frozen SST corpus, dosemu2/FDPP,
and confgen/bsl. A new CPU/DOS/transport/validation subsystem must name the
prior art it relies on and why that is insufficient.

**Direction corrections (2026-10).** This section replaces earlier drift: emu86
was hardware-anchored and developed despite never being deployed by the product;
its differential validator against dosemu2 was retired without that question
being resolved, leaving dead code and reference-only docs; and its DOS layer
approximated FDPP without authority. The codebase now states one story instead
of three half-retired ones; the retired validator's history is a one-page note
under [`docs/dosemu2/`](docs/dosemu2/README.md).

## Repository layout

| Path | Contents |
| --- | --- |
| `dis86/` | The decompiler (Rust): decode → analysis → SSA IR → optimization → control flow → AST → C codegen. Also contains emu86, the frozen 8086/286 reference emulator + SST harness ([`docs/emu86/sst.md`](docs/emu86/sst.md)); not built by default. |
| `bsl/` | Barebones Specification Language (small C parser) for configuration/annotation tables. |
| `confgen/` | Python generators that produce `.bsl` configuration. |
| `hydra/` | Hybrid runtime (C, Meson) linking native decompiled code with remaining x86-16 machine code. Emulator-independent core; see [`hydra/README.md`](hydra/README.md). |
| `docs/` | 8086/DOS reference notes and the dosemu2 migration corpus (`docs/dosemu2/`). |
| `scripts/` | SST corpus-fetch tooling. |

## Goals and Non-goals

Goals:

- Support reverse-engineering 16-bit real-mode x86 DOS binaries
- Generate code that is semantically correct (in so far as practical)
- Generate code that integrates well with the hybrid-runtime system (Hydra)
- Avoid making many assumptions or using heuristics that can lead to broken decompiled code
- Be hackable and easy to extend as required
- Automate away common manual transformations and let a human reverser focus on the subjective tasks a computer cannot do well (e.g. naming things)

Non-goals:

- Output code beauty (semantic correctness is more important)
- Re-compilable to equivalent binaries

Also, we generally prefer manual configuration/annotation tables to flawed heuristics that will generate incorrect code.

## Discussion of Internals

Discussion of the upstream internals is published periodically on the original author's blog: [xorvoid](https://www.xorvoid.com)

## Building

Assuming you have rust, cargo, and meson/ninja installed:

```
just build
```

This builds the dis86 crate (Rust) and the Hydra runtime (C, Meson).

For the host-independent test suite used by pull requests (no dosemu2 checkout or SDL required), install `just` and run:

```
just check
```

See [`docs/emu86/sst.md`](docs/emu86/sst.md) for the frozen emu86/SST offline tooling (built only with `--features emu86`; the interactive emu86 frontend additionally needs `sdl`). [`docs/dosemu2/TESTING.md`](docs/dosemu2/TESTING.md) covers the Hydra host integration tests.

## Some Commands

Input binaries are either an MZ executable (`--binary-exe`) or a raw flat text region (`--binary-raw`); exactly one is required:

Emit disassembly:

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-dis <output-file>
```

Emit initial Intermediate Representation (IR):

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-ir-initial <output-file>
```

Emit final (optimized) Intermediate Representation (IR):

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-ir-final <output-file>
```

Visualize the control-flow graph with graphviz:

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-graph /tmp/ctrlflow.dot
dot -Tpng /tmp/ctrlflow.dot > /tmp/control_flow_graph.png
open /tmp/control_flow_graph.png
```

Emit inferred higher-level control-flow structure:

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-ctrlflow <output-file>
```

Emit an Abstract Syntax Tree (AST):

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-ast <output-file>
```

Emit C code:

```
./target/debug/dis86 --config <your_config.bsl> --binary-exe <program.exe> --name <function_name> --emit-code <output-file>
```

Other modes: `--analyze` (config-driven analysis), `--trace-analyze --trace <file>` (analysis of a recorded Hydra execution trace, issue #97), `--start-addr`/`--end-addr` (address-range slices), `--codeseg-name` (decompile a whole configured code segment), `--codegen-hydra` (Hydra-flavored codegen), and `--codegen-hydra --relocatable-seg HEX[,HEX...]` (emit listed absolute link-time segment values as `(CODE_START_SEG + seg)` for relocated Hydra execution). Run `dis86 --help` for the full list. The `mzfile` companion binary inspects and extracts MZ executables via subcommands (`mzfile info <program.exe>`, plus `extract`, `map`, and `dis`).

The end-to-end, from-zero walkthrough — MZ forensics to a working native binary, including the stock-dosemu2 oracle — is the operator's local recompile prompt, kept out of this tree; the in-repo end-to-end lane is [`parity/`](parity/README.md).

## Caveats & Limitations

Primary development goal is to support an ongoing reverse-engineering and reimplementation project. The decompiler is designed to emit code that integrates well with the Hydra hybrid runtime. As such, uses that fall out of this scope have been unconsidered and may have numerous unknown issues.

Some specific known limitations:

- Both MZ executables and flat raw binary regions are accepted, but loading relies on annotation/config tables rather than full-blown generic executable analysis.
- Handling of many 8086 opcodes is unimplemented in the assembly->ir build step. Implementations are added as needed.
- Handling of some IR ops is unimplemented in the ir->ast convert step. Implementations are added as needed.
- Control-flow synthesis recognizes reducible if/else diamonds in addition to while-loops, single-arm if-statements, and switch-statements. Irreducible control flow remains explicit.
- Block scheduling and placement is very unoptimal for more complicated control-flow.
- emu86 (frozen offline tooling) implements only the instruction/device subset exercised by the project's target binaries and is hardware-anchored against the SST corpus; see [`docs/emu86/sst.md`](docs/emu86/sst.md). It is not built by default and plays no role in product validation.
- Signed comparison constant folding uses the source operand width when known, including signed 8-bit normalization; comparisons with unknown width are left unfolded.
- ... and many more ...

## Future Plans / Wishlist

Feature wishlist:

- Array accesses
- Compound types (struct and unions)
- Synthesizing struct/union member access
- Pointer analysis and arithmetic
- More "u16 pair -> u32" fusing
- Improved type-aware IR
- Less verbose output C code patterns for common operations (e.g. passing pointer as a function call arg)
- Production-target integration of Dis86-generated functions and Hydra hooks with stock dosemu2 through dosdebug.

## Lineage

Dis86 began life as a simple disassembler and 1-to-1 instruction => C-statement decompiler that integrated with the Hydra runtime. Over time it gained complexity and became difficult to extend, so it was rebuilt and rearchitected with a proper SSA IR. The older non-SSA versions are no longer carried in this repository; they remain available in the git history of this fork and of upstream [xorvoid/dis86](https://github.com/xorvoid/dis86).
