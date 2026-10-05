# OPTION E DESIGN — Opt-in overlay-hook support (external dosdebug host)

Status: design draft for the Phase 7 follow-up PR ("overlay PR"). This document
is the contract for that work, per the review agreement on PR #34/#36: overlays
stay **fail-closed by default**, and any dynamic-arming implementation must be
**explicitly opted in** and independently verified before it changes that
default.

## 1. Problem statement

A logical overlay hook (`HYDRA_HOOK_FLAGS_OVERLAY`, address built via
`ADDR_MAKE_EXT(ovl#, seg, off)`) has no stable physical breakpoint location
until the overlay manager has mapped the containing segment into the guest
window. The stock dosdebug breakpoint table is physical-only, so today the
backend refuses such runs outright (`host_driver.c`: "overlay hook … is
unsupported"), enforced as a regression contract by `test_overlay_reject`.

This PR adds *lazy, dynamic arming*: overlay hooks are planted when their
physical location materializes during a run, and re-armed after events that
invalidate the mapping or the planted byte.

## 2. Non-negotiable constraints (inherited from #34/#36 review)

1. **Default mode is unchanged.** Without the opt-in (§3), overlay-typed hooks
   still fail the run exactly as today; `test_overlay_reject` must pass
   unmodified. No silent skipping is acceptable in any mode.
2. **dosdebug breakpoint semantics are load-bearing.** Upstream
   `mhp_bpset()` saves the guest byte and arms `0xCC` via `WRITE_BYTE`;
   `mhp_bpclr()` restores the saved opcode. Therefore:
   - cleanup-before-capture remains a *snapshot correctness* requirement, not
     debugger bookkeeping;
   - any restore path must reconstruct arming from the restored **clean**
     guest bytes before resume — never assume a previously planted `CC`
     survived.
3. **Stub-byte classification before arming (CD 3F vs EA).** The two stub
   states have opposite policies:
   - `CD 3F` (overlay-dispatcher INT) ⇒ **unpaged**: never plant a
     breakpoint here — the int must execute natively so the overlay manager
     can page the body in and patch the stub itself;
   - `EA off16 seg16` (JMP FAR) ⇒ **paged**: the mapping exists; derive the
     physical destination and arm;
   - anything else ⇒ fail closed (mapping corruption).
   In other words breakpoints are planted *only* on paged `EA` stubs, and
   only after the guest's own pager produced that state — which is why lazy
   arming never trips the simx86 translation hazard cited by #34's
   rejection.
4. **Idempotence.** Arming an already-armed site, disarming an unarmed site,
   and duplicate stops on an already-dispatched hook are all no-ops or
   tolerated, never errors and never double-dispatch.
5. **Breakpoint budget.** Dynamic sites consume dosdebug table entries;
   accounting goes through the existing `HOST_RUN_MAX_BPS` preflight so
   exhaustion still fails closed before guest execution.
6. **Verification standard.** Host-independent unit coverage plus the
   real-dosemu exact-head gate (both workflows green) with the local Rev 7076
   matrix, same as #36.

## 3. Opt-in mechanism

Conf key on the dosemu backend string: `overlays=armed`.

- absent / anything else → current reject-before-execution behavior;
- `overlays=armed` → overlay hooks participate in `host_run()` with dynamic
  arming as described here.

The key is parsed once in `user_init()` alongside `code_load=`/`data_seg=`
and stored on the host ctx; `install_special_mode_breakpoint()` /
static-install paths branch on it. `test_overlay_reject` keeps running with
no conf, which is precisely why it remains valid untouched.

## 4. Arming lifecycle

For each OVERLAY-flagged hook the run loop maintains a small site record:

    { ovl#, seg, off, state ∈ {UNRESOLVED, ARMED, DISPATCHED}, saved_byte }

1. **UNRESOLVED (unpaged).** While the site reads `CD 3F`, no breakpoint is
   planted and the dispatcher runs natively; the run loop simply watches for
   the transition.
2. **UNRESOLVED → ARMED (page-in detected).** Once the overlay segment is
   registered with the core (`hydra_overlay_segment_set()`, done when the
   stop handler sees the stub patched to `EA`) and the physical byte matches
   `EA`, save it, plant the breakpoint, state = ARMED. From then on the
   stub's real bytes never execute natively again — dispatch redirects into
   the paged body.
   - already `0xCC` → treat as armed (idempotence);
   - anything else → fail the run (mapping corruption).
2. **ARMED → DISPATCHED → re-ARM.** The stop handler recognizes the site like
   any static hook; after the native handler returns, the existing
   clear/re-arm path applies unchanged.
3. **Invalidation.** Any operation that writes guest memory under an armed
   site (snapshot capture, raw-code scratch adjacency checks, explicit
   unmaps) clears tracked overlay breakpoints first — the same rule static
   hooks already obey.

## 5. Interaction with HYDSNAP capture/restore

Because `mhp_bpset()` patches guest memory (verified against dosemu2 source
in #34 review), snapshot cleanliness rests entirely on clearing breakpoints
before capture, and arming state must be treated as *derivable*, never
stored:

- Capture keeps the existing contract: breakpoints (static *and* armed
  overlay sites) are cleared before the window copy, so the snapshot contains
  clean guest bytes and a self-consistent CRC.
- Restore runs an **eager reconstruction pass** before releasing the guest:
  clear bps → restore clean mem/regs → rescan all OVERLAY-flagged stubs in
  the restored image → rebuild `overlay_segments[]`/site states purely from
  the bytes (`int 3f` = unpaged, `jmp far` = paged) → plant only valid paged
  breakpoints → release. A stub classifying as neither pattern degrades to
  the default fail-closed behavior, never to silent misdispatch.
- All reconciliation is idempotent: repeated passes must not duplicate
  debugger entries or perturb guest bytes.
- VIF/FLAGS handling uses the existing hardened register APIs (no new
  hazard); raw-slot addresses stay process-monotonic with fatal exhaustion —
  no reusable trampoline address for overlay mechanics.

## 6. Test plan

Host-independent:

1. Site-state machine unit tests (classification, idempotence, corruption
   failure) against the fake machine.
2. `test_overlay_reject` unmodified and still green (default-mode contract).

Agent-agreed suite (from the #34 follow-up contract), on top of the
unchanged `test_overlay_reject` default-mode coverage:

3. first call sees `CD 3F` execute natively — no breakpoint exists on the
   unpaged stub;
4. post-page-in (`EA`) calls dispatch the decompiled hook; native and
   decompiled results agree, counter threshold met;
5. **EA→CC→EA stub-byte assertion through the shared mapping** (guards the
   tc/snapshot premise directly);
6. cap/restore round-trip in BOTH the unpaged and paged states, each with
   reconstruction;
7. malformed stub fails closed; repeated reconciliation is idempotent.
8. Full acceptance gate: `.exe` ×1, `.com` ×2 consecutive, capture ×1,
   restore ×2 fresh instances, plus the overlay flavor above — all on the
   exact PR head, both CI workflows green.

## 7. Explicitly out of scope

- Overlay *managers* themselves remain guest-side (VROOMM-style); Hydra only
  observes remap results through its own segment table.
- No change to static-hook install order, raw-code reservation rules, or the
  64-breakpoint failure semantics beyond accounting for dynamic sites.
