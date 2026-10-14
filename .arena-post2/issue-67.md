<!-- arena-triage-verify-r2 -->
Verified on current `main` (`ca4981b`, PR #73) with the report's `synth_b.exe`. Decompiling A now warns and keeps the documented drop semantics:

```text
WARN: A: branch at 0000:0006 targets 0000:000a outside function range [0000:0000, 0000:0009); the edge is dropped from the output
```

with the same C output as before (`*PTR_16(DS, 0x100) = 1;`, no `goto`). The warn/skip matrix is pinned by unit tests (`out_of_range_edge_skips_fallthrough_targets`, `branch_to_next_at_end_links_both_arms_silently`), so branch-to-next padding stays silent. That matches the requested "loud diagnostic without changing drop semantics" — closing looks right.

---
_Posted via CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); raw evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
