# Dis86

Dis86 is a decompiler for 16-bit real-mode x86 DOS binaries. This repository is the `evaluating-cli` fork of [xorvoid/dis86](https://github.com/xorvoid/dis86); the upstream project is unfinished and development continues here.

# Purpose

Dis86 has been built for doing reverse-engineering work such as analyzing and re-implementing old DOS video games from the early 1990s. The project is a work-in-progress and the development team makes no guarantees it will work or be useful out-of-the-box for any applications other than their own. Features and improvements are made on-demand as needed.

## Current direction

1. **Port the runtime and validation strategy onto dosemu2, replacing the historical patched DOSBox-X fork.** The dosemu2 differential-validator transport is delivered and its shared-memory contract is frozen at ABI v1 (see [`docs/dosemu2/`](docs/dosemu2/README.md)). Remaining port work is the expanded differential corpus and porting Hydra's native-function hosting onto dosemu2.
2. **Continue decompiler development beyond upstream.** The fork carries and continues to land decompiler, SSA, optimizer, and AST improvements that upstream does not have.

## Repository layout

| Path | Contents |
| --- | --- |
| `dis86/` | The decompiler (Rust): decode → analysis → SSA IR → optimization → control flow → AST → C codegen. Also contains emu86, the in-repo 8086/286 reference CPU emulator, and the differential validator that checks execution against it. |
| `bsl/` | Barebones Specification Language (small C parser) for configuration/annotation tables. |
| `confgen/` | Python generators that produce `.bsl` configuration. |
| `hydra/` | Hybrid runtime (C, Meson) linking native decompiled code with remaining x86-16 machine code. Emulator-independent core; see [`hydra/README.md`](hydra/README.md). |
| `docs/` | 8086/DOS reference notes and the dosemu2 migration corpus (`docs/dosemu2/`). |
| `patches/dosemu2/` | Frozen two-patch carrier applied on top of a pinned dosemu2 base. |
| `scripts/` | Patch application and runtime-probe tooling. |

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

See [`docs/dosemu2/TESTING.md`](docs/dosemu2/TESTING.md) for the dosemu2 testing strategy and the optional interactive `emu86` build (SDL-gated).

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

Other modes: `--analyze` (config-driven analysis), `--start-addr`/`--end-addr` (address-range slices), `--codeseg-name` (decompile a whole configured code segment), and `--codegen-hydra` (Hydra-flavored codegen). Run `dis86 --help` for the full list. The `mzfile` companion binary inspects and extracts MZ executables.

## Caveats & Limitations

Primary development goal is to support an ongoing reverse-engineering and reimplementation project. The decompiler is designed to emit code that integrates well with the Hydra hybrid runtime. As such, uses that fall out of this scope have been unconsidered and may have numerous unknown issues.

Some specific known limitations:

- Both MZ executables and flat raw binary regions are accepted, but loading relies on annotation/config tables rather than full-blown generic executable analysis.
- Handling of many 8086 opcodes is unimplemented in the assembly->ir build step. Implementations are added as needed.
- Handling of some IR ops is unimplemented in the ir->ast convert step. Implementations are added as needed.
- Control-flow synthesis is limited to while-loops, if-stmts, and switch-stmts. If-else is unimplemented.
- Block scheduling and placement is very unoptimal for more complicated control-flow.
- emu86 implements only the instruction/device subset exercised by the project's target binaries; the dosemu2 differential validator is used to bring behavior into alignment incrementally.
- ... and many more ...

## Future Plans / Wishlist

Feature wishlist:

- Array accesses
- Compound types (struct and unions)
- Synthesizing struct/union member access
- If-else statements
- Pointer analysis and arithmetic
- More "u16 pair -> u32" fusing
- Improved type-aware IR
- Less verbose output C code patterns for common operations (e.g. passing pointer as a function call arg)
- dosemu2 as the Hydra hybrid-runtime host (native-function interception; see [`hydra/README.md`](hydra/README.md))
- Expanded dosemu2 differential validation corpus (see [`docs/dosemu2/TESTING.md`](docs/dosemu2/TESTING.md))

## Lineage

Dis86 began life as a simple disassembler and 1-to-1 instruction => C-statement decompiler that integrated with the Hydra runtime. Over time it gained complexity and became difficult to extend, so it was rebuilt and rearchitected with a proper SSA IR. The older non-SSA versions are no longer carried in this repository; they remain available in the git history of this fork and of upstream [xorvoid/dis86](https://github.com/xorvoid/dis86).
