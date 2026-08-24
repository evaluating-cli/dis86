/*
 * host_driver.c - Phase 4 execution loop for the Hydra-on-dosemu2 host.
 *
 * See host_driver.h for the design. The driver runs the guest CPU through
 * dosemu2's int3 breakpoints, dispatching each stop through the Hydra core
 * (hydra_machine_exec). When a hook requests raw-code execution, the driver
 * single-steps (dosdebug 't') through the raw-code slot until RETF/RET
 * lands at the return address, then feeds the state back to the exec engine.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <string.h>

#include "host_driver.h"
#include "internal.h"

/* ------------------------------------------------------------------ */
/* breakpoint bookkeeping                                              */
/* ------------------------------------------------------------------ */

typedef struct host_bp_entry {
  uint32_t linear;   /* seg*16+off exactly as dosemu sees it */
  int      index;    /* dosemu bp index */
  uint8_t  used;
  uint8_t  is_stub;  /* one-shot return stub (cleared on hit) */
} host_bp_entry_t;

static host_bp_entry_t *host_bp_find(host_bp_entry_t *bps, uint32_t linear)
{
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (bps[i].used && bps[i].linear == linear)
      return &bps[i];
  }
  return NULL;
}

static int host_bp_add(host_bp_entry_t *bps, int index, uint32_t linear,
                        int is_stub)
{
  if (host_bp_find(bps, linear))
    return 0; /* already tracked (dosemu rejects duplicates anyway) */
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (!bps[i].used) {
      bps[i].used = 1;
      bps[i].index = index;
      bps[i].linear = linear;
      bps[i].is_stub = (uint8_t)is_stub;
      return 0;
    }
  }
  return -1; /* table full */
}

/* Clear every tracked breakpoint (call before discarding the bps table). */
static void host_bp_clear_all(host_ctx_t *ctx, host_bp_entry_t *bps)
{
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (bps[i].used)
      host_clear_bp(ctx, bps[i].index);
    bps[i].used = 0;
  }
}

/* ------------------------------------------------------------------ */
/* hook breakpoint installation                                        */
/* ------------------------------------------------------------------ */

static void hook_counter(const hydra_hook_t *hook, void *user);
static void bp_visitor(const hydra_hook_t *hook, void *user);

typedef struct bp_install {
  host_ctx_t *ctx;
  host_bp_entry_t *bps;
  int count;
  int failed;   /* hooks that could not get a breakpoint */
} bp_install_t;

static void bp_visitor(const hydra_hook_t *hook, void *user)
{
  bp_install_t *ins = user;

  if (hook->flags & HYDRA_HOOK_FLAGS_OVERLAY)
    return; /* overlay hooks are paged in dynamically; no static bp */

  u16 seg = (u16)(ins->ctx->code_load_offset + addr_seg(hook->addr));
  u16 off = addr_off(hook->addr);
  int idx = host_set_bp(ins->ctx, seg, off);
  if (idx < 0) {
    /* dosemu bp table full (64 max) or dosdebug error: the hook would
     * silently never fire — record the failure loudly instead. */
    fprintf(stderr, "host_run: FAILED to plant breakpoint for hook at rel %04x:%04x\n",
            addr_seg(hook->addr), addr_off(hook->addr));
    ins->failed++;
    return;
  }
  uint32_t linear = ((uint32_t)seg << 4) + off;
  if (host_bp_add(ins->bps, idx, linear, 0) != 0) {
    /* local tracking table full: clear the dosemu bp we just planted so
     * nothing is left dangling, then record the failure. */
    host_clear_bp(ins->ctx, idx);
    fprintf(stderr, "host_run: breakpoint table full (%d); hook at rel %04x:%04x dropped\n",
            HOST_RUN_MAX_BPS, addr_seg(hook->addr), addr_off(hook->addr));
    ins->failed++;
    return;
  }
  ins->count++;
}

int host_hook_breakpoint_count(host_ctx_t *ctx)
{
  (void)ctx;
  int count = 0;
  hydra_hook_foreach(hook_counter, &count);
  return count;
}

static void hook_counter(const hydra_hook_t *hook, void *user)
{
  int *count = user;
  if (!(hook->flags & HYDRA_HOOK_FLAGS_OVERLAY))
    (*count)++;
}

/* ------------------------------------------------------------------ */
/* register conversion                                                 */
/* ------------------------------------------------------------------ */

static void regs_to_machine(const dosdebug_regs_t *dr, hydra_machine_registers_t *hr)
{
  hr->ax = dr->ax;   hr->bx = dr->bx;   hr->cx = dr->cx;   hr->dx = dr->dx;
  hr->si = dr->si;   hr->di = dr->di;   hr->bp = dr->bp;   hr->sp = dr->sp;
  hr->ip = dr->ip;   hr->cs = dr->cs;   hr->ds = dr->ds;   hr->es = dr->es;
  hr->ss = dr->ss;   hr->flags = dr->flags;
}

static void machine_to_regs(const hydra_machine_registers_t *hr, dosdebug_regs_t *dr)
{
  dr->ax = hr->ax;   dr->bx = hr->bx;   dr->cx = hr->cx;   dr->dx = hr->dx;
  dr->si = hr->si;   dr->di = hr->di;   dr->bp = hr->bp;   dr->sp = hr->sp;
  dr->ip = hr->ip;   dr->cs = hr->cs;   dr->ds = hr->ds;   dr->es = hr->es;
  dr->ss = hr->ss;   dr->flags = hr->flags;
}

/* ------------------------------------------------------------------ */
/* raw-code / native->guest call execution (trace-based)              */
/* ------------------------------------------------------------------ */

/* When hydra_exec_run returns CALL/CALL_NEAR, the hook either
 *   (a) called hydra_impl_raw_code (opcodes at a raw-code slot), or
 *   (b) called through to guest code (native->guest call via a callstub).
 * In both cases hydra_impl_call_far/near pushed a magic return address on
 * the guest stack (0xffff:exec_id far, <caller_cs>:0xff00+exec_id near) and
 * the driver has pushed CS:IP = call target into dosemu2.
 *
 * Instead of planting an INT3 breakpoint at the return address (which
 * fails because the simx86 JIT cache doesn't see memfd writes), we trace
 * with `t` (step-over: nested calls and INTs execute in one step) until
 * RETF/RET lands at the magic return address. Real guest functions need a
 * generous step budget; the default is HOST_RUN_DEFAULT_TRACE_STEPS.
 *
 * If the traced guest enters another hook's breakpoint, that hook is
 * dispatched recursively (nested native->guest call into a hooked fn). */

#define HOST_RUN_DEFAULT_TRACE_STEPS 10000u

/* Max nested-hook recursion (hook -> guest call -> hook -> ...). */
#define HOST_RUN_MAX_DEPTH 8

/* Max redirects (CALL/JUMP results) per hook dispatch. A hook that issues
 * more than this is either pathological or the driver lost sync — bail out
 * instead of spinning forever. */
#define HOST_RUN_MAX_REDIRECTS 256

typedef struct run_ctx {
  host_ctx_t *ctx;
  hydra_machine_t *m;
  host_bp_entry_t *bps;
  host_run_stats_t *st;
  const host_run_options_t *opts;
  int timeout_ms;
  int verbose;
} run_ctx_t;

static host_run_stop_reason_t dispatch_hook(run_ctx_t *r, dosdebug_regs_t *dr,
                                            int depth);

/* Trace until the magic return address (or budget exhaustion / error).
 * On HOST_RUN_STOP_NONE, *dr holds the CPU at ret_cs:ret_ip. */
static host_run_stop_reason_t trace_to_return(run_ctx_t *r, dosdebug_regs_t *dr,
                                              uint16_t ret_cs, uint16_t ret_ip,
                                              int depth)
{
  const size_t max_steps =
      (r->opts && r->opts->max_trace_steps) ? r->opts->max_trace_steps
                                            : HOST_RUN_DEFAULT_TRACE_STEPS;
  for (size_t i = 0; i < max_steps; i++) {
    /* Single-step one instruction; wait_stop blocks for the post-step dump.
     * Drain after it, too: a step can produce extra unsolicited output
     * (e.g. landing on a breakpoint mid-trace) which would otherwise
     * desynchronize the command/response stream by one block. */
    if (dosdebug_step(r->ctx->db) != 0) return HOST_RUN_STOP_ERROR;
    if (dosdebug_wait_stop(r->ctx->db, dr, r->timeout_ms) != 0)
      return HOST_RUN_STOP_ERROR;
    dosdebug_drain(r->ctx->db);
    if (r->verbose > 1)
      printf("  trace[%zu] %04x:%04x (want %04x:%04x)\n",
             i, dr->cs, dr->ip, ret_cs, ret_ip);

    /* Arrived at the magic return address? */
    if (dr->cs == ret_cs && dr->ip == ret_ip) {
      r->st->raw_code_returns++;
      return HOST_RUN_STOP_NONE;
    }

    /* Stepped onto another hook's breakpoint: dispatch it nested. */
    uint32_t lin = ((uint32_t)dr->cs << 4) + dr->ip;
    if (host_bp_find(r->bps, lin)) {
      if (r->verbose > 1)
        printf("  nested hook at %04x:%04x (depth %d)\n", dr->cs, dr->ip, depth + 1);
      host_run_stop_reason_t dn = dispatch_hook(r, dr, depth + 1);
      if (dn != HOST_RUN_STOP_NONE)
        return dn;
      /* Nested dispatch ends with its final state pushed to dosemu (dr); the
       * trace continues stepping from there. */
      if (r->verbose > 1)
        printf("  nested hook done -> %04x:%04x\n", dr->cs, dr->ip);
    }
  }
  fprintf(stderr, "host_run: trace budget (%zu steps) exhausted before return "
          "to %04x:%04x (cpu at %04x:%04x)\n",
          max_steps, ret_cs, ret_ip, dr->cs, dr->ip);
  return HOST_RUN_STOP_ERROR;
}

/* Dispatch one hook through the Hydra core, handling all of its redirects
 * (CALL/CALL_NEAR traces, JUMP state updates). On success the hook is done
 * and *dr holds the final guest state, already pushed to the dosemu2 CPU. */
static host_run_stop_reason_t dispatch_hook(run_ctx_t *r, dosdebug_regs_t *dr,
                                            int depth)
{
  if (depth > HOST_RUN_MAX_DEPTH) {
    fprintf(stderr, "host_run: nested hook depth %d exceeded\n",
            HOST_RUN_MAX_DEPTH);
    return HOST_RUN_STOP_ERROR;
  }

  int redirects_this_dispatch = 0;
  for (;;) {
    regs_to_machine(dr, r->m->registers);
    int ret = hydra_machine_exec(r->m, 0);
    hydra_machine_notify(r->m);
    int rtype = hydra_exec_last_result_type();

    if (ret == 0)
      break;  /* hook completed, no redirect */

    r->st->redirects++;
    if (++redirects_this_dispatch > HOST_RUN_MAX_REDIRECTS) {
      if (r->verbose)
        printf("host_run: redirect cap (%d) exceeded at %04x:%04x\n",
               HOST_RUN_MAX_REDIRECTS, dr->cs, dr->ip);
      return HOST_RUN_STOP_ERROR;
    }

    if (rtype != HYDRA_RESULT_TYPE_CALL &&
        rtype != HYDRA_RESULT_TYPE_CALL_NEAR)
      break;  /* JUMP or other redirect — no guest call to trace */

    r->st->raw_code_runs++;

    /* Push the redirect CS:IP (call target) into dosemu2. */
    machine_to_regs(r->m->registers, dr);
    if (host_set_regs(r->ctx, dr) != 0)
      return HOST_RUN_STOP_ERROR;

    /* Compute the expected magic return address. */
    u16 eid = hydra_exec_active_id();
    uint16_t ret_cs, ret_ip;
    if (rtype == HYDRA_RESULT_TYPE_CALL) {
      ret_cs = 0xffff;
      ret_ip = eid;
    } else {
      ret_cs = r->m->registers->cs;
      ret_ip = (uint16_t)(0xff00 + eid);
    }

    /* Trace until RETF/RET lands at the magic return address. */
    host_run_stop_reason_t tr = trace_to_return(r, dr, ret_cs, ret_ip, depth);
    if (tr != HOST_RUN_STOP_NONE) {
      if (r->verbose)
        printf("host_run: raw-code trace failed (%d) near %04x:%04x\n",
               tr, dr->cs, dr->ip);
      return tr;
    }
    /* dr now holds the post-call state; loop back to resume exec. */
  }

  /* Push the final state into the guest CPU (idempotent for the caller of
   * host_run; required before a nested trace continues stepping). */
  machine_to_regs(r->m->registers, dr);
  if (host_set_regs(r->ctx, dr) != 0)
    return HOST_RUN_STOP_ERROR;

  return HOST_RUN_STOP_NONE;
}

/* ------------------------------------------------------------------ */
/* the run loop                                                        */
/* ------------------------------------------------------------------ */

host_run_stop_reason_t host_run(host_ctx_t *ctx, hydra_machine_t *m,
                                const host_run_options_t *opts,
                                host_run_stats_t *stats)
{
  host_bp_entry_t bps[HOST_RUN_MAX_BPS] = {{0}};
  host_run_stats_t st = {0};
  const int timeout_ms = (opts && opts->timeout_ms) ? opts->timeout_ms : 3000;
  const int verbose = opts ? opts->verbose : 0;
  const size_t max_steps = opts ? opts->max_steps : 0;
  const size_t start_hooks = hydra_exec_hook_dispatch_count();
  dosdebug_regs_t dr;
  size_t step = 0;
  host_run_stop_reason_t reason = HOST_RUN_STOP_NONE;

  /* Plant a breakpoint at every registered hook. Every hook MUST get one;
   * a silent drop means guest code runs unhooked — refuse to start. */
  {
    bp_install_t ins = { ctx, bps, 0, 0 };
    hydra_hook_foreach(bp_visitor, &ins);
    st.hook_breakpoints = (uint64_t)ins.count;
    if (ins.failed > 0) {
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
  }

  {
    run_ctx_t r = { ctx, m, bps, &st, opts, timeout_ms, verbose };

    for (;;) {
      if (host_go_and_wait(ctx, &dr, timeout_ms) != 0) {
        if (!dosdebug_is_alive(ctx->db))
          reason = HOST_RUN_STOP_DOSEMU_EXIT;
        else
          reason = HOST_RUN_STOP_TIMEOUT;
        break;
      }
      st.stops++;

      /* Reset the raw-code slot counter — the previous hook (if any) has
       * completed and the JIT has since executed non-raw-code guest code,
       * so reusing slot 0 for the next hook is safe. Note: only done at this
       * outer boundary, never inside a nested dispatch (which runs back-to-
       * back with its parent and must keep the slots distinct for the JIT). */
      hydra_impl_raw_code_reset();

      /* Dispatch the hook through the Hydra core. The hook may issue
       * multiple raw-code requests (CLI, STI, INT, INB, OUTB, ...) or call
       * through to guest functions before completing. Each produces a
       * CALL/CALL_NEAR result; we trace the guest on dosemu2 and feed the
       * result back. */
      host_run_stop_reason_t dn = dispatch_hook(&r, &dr, 0);
      if (dn != HOST_RUN_STOP_NONE) {
        reason = dn;
        break;
      }

      st.hook_dispatches = hydra_exec_hook_dispatch_count() - start_hooks;

      if (verbose) {
        printf("host_run: stop %04x:%04x hooks=%lu raw=%lu ret=%lu\n",
               dr.cs, dr.ip,
               (unsigned long)st.hook_dispatches,
               (unsigned long)st.raw_code_runs,
               (unsigned long)st.raw_code_returns);
      }

      if (opts && opts->stop_fn &&
          opts->stop_fn(ctx, &dr, &st, opts->stop_user)) {
        reason = HOST_RUN_STOP_CALLBACK;
        break;
      }
      if (max_steps && ++step >= max_steps) {
        reason = HOST_RUN_STOP_MAXSTEPS;
        break;
      }
      /* dispatch_hook already pushed the final guest state; resume. */
    }
  }

done:
  host_bp_clear_all(ctx, bps);
  if (stats)
    *stats = st;
  return reason;
}