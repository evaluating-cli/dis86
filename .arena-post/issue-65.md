<!-- arena-triage-verify -->
Verified end-to-end on `54d2d41` with the 75-byte synthetic in the report:

```text
$ dis86 --config repro65.bsl --binary-exe synth_65.exe --analyze
    F( "main",                        None,   None,        "0000:0000", "0000:000b", flags = "RET_UNKNOWN_CONFIG_NEAR" ),
analyze exit=1

$ dis86 --config repro65.bsl --binary-exe synth_65.exe --codeseg-name CODE --emit-code out.c
Error: Failed to decode instruction: Unsupported or invalid instruction at 0000:0003
Error: Failed to decompile main; continuing with the rest.
Error: Failed to decompile 1 function(s).
exit=1

$ dis86 --config repro65_fixed.bsl --binary-exe synth_65.exe --codeseg-name CODE --emit-code out.c   # start 0000:0006
exit=0
```

Both halves of the mismatch are confirmed in code: `FuncDetails::build` derives `end_addr_inferred` from a control-flow walk, while `decompile_spec` decodes `[start, end)` linearly and aborts on the first undecodable byte (`app.rs:299-316`).

## Additional findings while reproducing

- **`--analyze` exits 1 even when it succeeds**: `app_analyze::run` returns `1` unconditionally (`app_analyze.rs:16`). Any scripted usage of the documented workflow (`--analyze` → annotations → confgen → `--codeseg-name`) will read a successful scan as a failure. Returning 0 when the scan completes is a one-line fix; it may deserve its own issue.
- **The "Gaps" report is currently dead code**, as suspected: `analyze_code_segments_and_report` exists (`analyze/analyze.rs:88`) but its call site in `app_analyze::run` is commented out (`app_analyze.rs:14`). Even when re-wired it reports gaps *between* annotated ranges (`RangeSet::gaps_within`, `analyze.rs:64-83`) and would not flag non-code bytes *inside* a suggested range — exactly this issue's case. Two separate work items.
- **Docs**: the intended workflow and the "audit/split suggested ranges" caveat are now documented in `native-recompile-prompt.md` (§3 "Known decompiler limitations" and the §7 loop, lines ~75-156), so the documentation-gap portion of this issue is addressed (by #70). What remains is tool support.

## Suggestion

Have `--analyze` linearly probe each suggested range with the same decoder `decompile_spec` uses (`binary.region_iter` + `Decoder`), and when the probe fails, print the *first* undecodable offset plus a split hint, e.g.:

```text
WARN: main 0000:0000-0000:000b does not decode linearly (fails at 0000:0003);
      likely embedded data — split the range or declare it in text_section
```

That is exactly the information the operator needs to satisfy the audit, it keeps analyze advisory (no consumer behavior change), and the bounds + decoder are already in hand.

---
_Posted from CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); verification evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
