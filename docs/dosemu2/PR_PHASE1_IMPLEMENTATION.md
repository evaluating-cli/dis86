# Pull Request: Phase 1 `simx86` Core Hook & Low-Memory Export Architecture

## Summary

This pull request introduces the concrete implementation blueprint for Phase 1 of the `dosemu2` migration, providing the exact code modifications for `dosemu2`'s `simx86` CPU simulator, low-memory shared mapping export (`mapfile.c`), and lockstep synchronization with `dis86`'s `DosemuProcess`.

---

## Technical Highlights

1. **`simx86` Single-Instruction Hook (`hydra_step.c` & `interp.c`)**:
   - Forces `TheCPU.mode |= MSSTP` to generate exact single-instruction execution nodes (`G->seqlen == 1`).
   - Hooks instruction boundaries in `FindExecCode` inside `interp.c`.
   - Exports all 14 CPU registers (`AX..FLAGS`, `CS:IP`, segments) to `/dev/shm/hydra_remote`.

2. **Low-Memory Export (`src/base/lib/mapping/mapfile.c`)**:
   - Uses `shm_open("/dosemu_mem", ...)` for `lowmem_base`, enabling `dis86`'s `ShmMem::attach()` to bind directly to `dosemu2`'s 0–1MB conventional memory image without memory duplication.

3. **Descriptor Cache Resynchronization (`SetSegReal`)**:
   - Ensures any segment register mutation updates segment bases and limits (`sd->BoundL = sel << 4`).
   - Sets `TheCPU.err = EXCP_EMULEAVE` on external control redirection to force clean block exits without stale translation execution.

4. **Integration Deliverables**:
   - `docs/dosemu2/PHASE1_IMPLEMENTATION.md`: Complete code listings, struct definitions, and integration workflows.
