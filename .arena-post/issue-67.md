<!-- arena-triage-verify -->
Confirmed on `54d2d41` with the `synth_b.exe` + config from the report. `--name A --emit-code` exits 0 and emits:

```c
u32 A(void)
{
  u16 SP0 = SP;

  *PTR_16(DS, 0x100) = 1;

}
```

No `goto`, no label, no diagnostic — stderr contains only the unrelated `WARN: Could not find global for DS:0100`. `--emit-ctrlflow` for A shows the two basic blocks with **no edges** between them:

```text
ElemId(0) | BasicBlock(0)
ElemId(1) | BasicBlock(1)
```

(B decompiles normally.)

Mechanism confirmed as described: out-of-range targets are inserted into `block_start` like any other target (`ir_build.rs:1257-1270`), yielding a block with no decoded instructions; `block_exits` reports empty blocks as having no exits (`ir/methods.rs:105-115`); `from_ir_naive` copies that into element exits (`control_flow.rs:377`); `schedule_layout_basic_block` then sees `exits.len() == 0` and picks `Jump::None` (`control_flow.rs:920-921`). When the empty target block is still schedulable, the single-exit path takes `UncondFallthrough` and schedules it next (`control_flow.rs:922-928`) — which is why the symptom varies between an empty `addr_XXXX:;` stub and nothing at all.

## On the requested diagnostic

It is cheap and deterministic at build time: where `block_start` is populated (or in `start_next_blk`) both the enclosing spec bounds and the branch address/target are available, and the target offset is already preserved in the empty block's name (`addr_{off}`). Something like:

```text
WARN: <fn>: branch at 0000:XXXX targets 0000:YYYY outside declared range [0000:SSSS, 0000:EEEE)
```

plus an opt-in strict mode would close the silent-wrongness gap without changing the drop semantics, as the report suggests; there is already a `WARN:` convention in the builder for adjacent heuristics.

Two small additions:

- The same "target exists but is not decodable" situation also arises when the target is *inside* the range but is data (#65). A shared diagnostic helper at the range/branch boundary would cover both cases.
- +1 on "external continuation" being out of scope here, but note the needed information is already carried by the empty block, so a later annotation-driven tail-dispatch lowering would not have to change the build step.

---
_Posted from CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); verification evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
