<!-- arena-triage-verify-r2 -->
Verified on current `main` (`ca4981b`, PR #74) with the same 75-byte synthetic. `--analyze` now prints the diagnostic immediately before the suggestion:

```text
# UNDECODABLE GAP | main | 0000:0000 | linear decode of [0000:0000, 0000:000b) fails at 0000:0003; the suggested range spans non-code bytes - audit/shorten the range before use
F( "main",                        None,   None,        "0000:0000", "0000:000b", flags = "RET_UNKNOWN_CONFIG_NEAR" ),
```

That is exactly the first-failing-offset + audit hint, so the operator can act on the suggestion. Semantics are unchanged by design: building the config verbatim still fails (`Unsupported or invalid instruction at 0000:0003`, exit 1) while the corrected `start 0000:0006` config exits 0.

Two smaller items from my earlier comment are still open (fine to track separately): `--analyze` still returns 1 after a successful scan (`app_analyze.rs` returns 1 unconditionally), and the "Gaps" report is still unwired (call site commented out).

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
