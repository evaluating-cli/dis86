Resolved by #116, merged as `2a1176b`. A guest that terminates now exits the process with the code from `AL`, so this is no longer an abort.

**Verified against the repro in this issue**, not just inferred from the code. A guest ending in `mov ax,0x4c05; int 21h` (`B8 05 4C CD 21`) exits with **status 5**. The in-tree assertions cover `AL=0x42` and `AL=0x00`; I confirmed the `AL=5` case from this issue's fixture in a scratch build, and the exit is `exit((int)(m->registers->ax & 0xff))`, so the whole `AL` range follows. There is no livelock either — the process ends inside the hook, so it never reaches the 100000-step cap.

The other acceptance criteria also hold:

- The test this issue asked to flip (`probe_aborts(probe_impl_exit)`) is now `probe_exits_with(probe, expected)`, asserting the code rather than the abort.
- Only `AH=4Ch` terminates; unhandled vectors and unimplemented `AH` values still fail loud.
- The dosemu host is untouched — it executes `int 21h` as raw guest code and never reaches this path.

**One deliberate difference from the shape proposed here.** This issue sketched a `HYDRA_RESULT_TYPE_TERMINATE` that the exec loop could observe. #116 instead ends the process from inside `hydra_user_int`, which is why `machine.c` no longer exits at all and the policy lives in the host. Functionally equivalent for every criterion listed here, but it means termination is *not* observable to the host: `parity/native_runner.c` would still skip its outbox epilogue if a fixture ever terminated.

That residual is real but is a distinct change, and it is now written down rather than left implicit — see the "Guest termination" section of `hydra/README.md`, which records both consequences for embedders and names host-observable termination as the target design for #95 Tier 2 (save/exit behaviour, multiple guests per process). Opening a separate issue for it would be more accurate than leaving this one open, since everything this issue actually asked for now works.

Closing.
