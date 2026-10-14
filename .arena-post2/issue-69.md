<!-- arena-triage-verify-r2 -->
Status update after re-checking `main` (`ca4981b`): several of the ops listed here are now lowered.

Lowered since the report: ROL/ROR (value-exact shifts+or, flags clobbered loudly; #75), REP MOVS/STOS (runtime bulk macro with CX -> 0 and SI/DI += CX0*size; #79), single-step MOVS/STOS/LODS/CMPS/SCAS SI/DI auto-update (#76), and 16-bit ADC/SBB from the tracked in-block carry (#80).

Still `UNIMPL` on `main`: RCL/RCR (they need the incoming CF from the opaque flags value), REP LODS/CMPS/SCAS/INS/OUTS (and any REP form when DF may be set), non-REP INS/OUTS, and IN/OUT/STI/CLI/STD/CLD. STC/CLC/CMC now update the tracked carry so a following ADC/SBB lowers, while still clobbering the opaque flags value.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
