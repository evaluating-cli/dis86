# Source references

The porting investigation and review are based on these upstream projects and source areas.

## Projects

- `xorvoid/dis86` — x86-16 disassembler/decompiler and differential validator
- `xorvoid/hydra` — hybrid native/x86-16 runtime
- `xorvoid/dosbox-x` — patched baseline emulator used by Hydra
- `dosemu2/dosemu2` — target DOS execution environment

## dosemu2 areas requiring direct verification

- `src/base/emu-i386/simx86/interp.c` — translated execution loop and block behavior
- `src/include/memory.h` — `mem_base`, `lowmem_base`, `MEM_BASE32`, segment/linear helpers
- `src/include/coopth.h` — cooperative threading facilities
- `src/include/hlt.h` — HLT handler API
- `src/plugin/` — plugin integration model

## dis86 / Hydra areas expected to change

- `src/emu86/validator/` — lockstep validator and emulator-process control
- Hydra machine/register ABI
- Hydra hook dispatch and control-flow result handling
- shared-memory IPC and memory-view integration

## Validation policy

Performance estimates and exact hook locations should remain provisional until demonstrated against the concrete dosemu2 revision used by the port. In particular, normal Hydra execution and strict one-instruction lockstep validation must be benchmarked separately.
