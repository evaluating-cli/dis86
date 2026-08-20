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
} bp_install_t;

static void bp_visitor(const hydra_hook_t *hook, void *user)
{
  bp_install_t *ins = user;

  if (hook->flags & HYDRA_HOOK_FLAGS_OVERLAY)
    return; /* overlay hooks are paged in dynamically; no static bp */

  u16 seg = (u16)(ins->ctx->code_load_offset + addr_seg(hook->addr));
  u16 off = addr_off(hook->addr);
  int idx = host_set_bp(ins->ctx, seg, off);
  if (idx >= 0) {
    uint32_t linear = ((uint32_t)seg << 4) + off;
    if (host_bp_add(ins->bps, idx, linear, 0) == 0)
      ins->count++;
  }
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

int host_run_install_hook_breakpoints(host_ctx_t *ctx)
{
  host_bp_entry_t bps[HOST_RUN_MAX_BPS] = {{0}};
  bp_install_t ins = { ctx, bps, 0 };
  hydra_hook_foreach(bp_visitor, &ins);
  return ins.count;
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
/* raw-code execution (trace-based, avoids JIT cache issues)          */
/* ------------------------------------------------------------------ */

/* When hydra_exec_run returns CALL/CALL_NEAR, the hook called
 * hydra_impl_raw_code which wrote opcodes (e.g. CLI;RETF) at a raw-code
 * slot and hydra_impl_call_far pushed a return address (ffff:exec_id for
 * far, cs:0xff00+exec_id for near) on the guest stack. The driver has
 * already pushed CS:IP = raw-code slot to dosemu2.
 *
 * Instead of planting an INT3 breakpoint at the return address (which
 * fails because the simx86 JIT cache doesn't see memfd writes), we trace
 * through the raw code with `t` (step-over). The raw code is at most
 * 3 bytes (INT n;RETF) = 2 instructions, so 2 trace steps always reaches
 * the RETF and lands at the return address. */

/* Trace through the raw code and detect the return.
 * Returns 1 if the return was detected, 0 otherwise. */
static int host_trace_raw_code(host_ctx_t *ctx, dosdebug_regs_t *dr,
                               uint16_t ret_cs, uint16_t ret_ip,
                               host_run_stats_t *st)
{
  for (int i = 0; i < 6; i++) {
    /* Single-step one instruction. dosdebug_wait_stop blocks until the
     * step completes and returns the new register state. */
    if (dosdebug_step(ctx->db) != 0) return 0;
    if (dosdebug_wait_stop(ctx->db, dr, 2000) != 0) return 0;
    /* Check if we arrived at the return address */
    if (dr->cs == ret_cs && dr->ip == ret_ip) {
      st->stub_hits++;
      return 1;
    }
  }
  return 0; /* didn't arrive after 6 steps */
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
  const int timeout_ms = (opts && opts->timeout_ms > 0) ? opts->timeout_ms : 3000;
  const int verbose = opts && opts->verbose;
  const size_t max_steps = opts ? opts->max_steps : 0;
  const size_t start_hooks = hydra_exec_hook_dispatch_count();
  dosdebug_regs_t dr;
  size_t step = 0;
  host_run_stop_reason_t reason = HOST_RUN_STOP_NONE;

  /* Plant a breakpoint at every registered hook. */
  {
    bp_install_t ins = { ctx, bps, 0 };
    hydra_hook_foreach(bp_visitor, &ins);
    st.hook_breakpoints = (uint64_t)ins.count;
  }

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
     * so reusing slot 0 for the next hook is safe. */
    hydra_impl_raw_code_reset();

    /* Dispatch the hook through the Hydra core. The hook may issue
     * multiple raw-code requests (CLI, STI, INT, INB, OUTB, ...) before
     * completing. Each raw-code request produces a CALL/CALL_NEAR result;
     * we trace the raw code on dosemu2 and feed the result back. */
    int rtype;
    for (;;) {
      regs_to_machine(&dr, m->registers);
      int ret = hydra_machine_exec(m, 0);
      hydra_machine_notify(m);
      rtype = hydra_exec_last_result_type();

      if (ret == 0)
        break;  /* hook completed, no redirect */

      st.redirects++;

      if (rtype != HYDRA_RESULT_TYPE_CALL &&
          rtype != HYDRA_RESULT_TYPE_CALL_NEAR)
        break;  /* JUMP or other redirect — no raw code needed */

      st.raw_code_runs++;

      /* Push the redirect CS:IP (raw-code slot) into dosemu2. */
      machine_to_regs(m->registers, &dr);
      if (host_set_regs(ctx, &dr) != 0) {
        reason = HOST_RUN_STOP_ERROR;
        goto done;
      }

      /* Compute the expected return address. */
      u16 eid = hydra_exec_active_id();
      uint16_t ret_cs, ret_ip;
      if (rtype == HYDRA_RESULT_TYPE_CALL) {
        ret_cs = 0xffff;
        ret_ip = eid;
      } else {
        ret_cs = m->registers->cs;
        ret_ip = (uint16_t)(0xff00 + eid);
      }

      /* Trace through the raw code (single-step until RETF/RET
       * lands at the return address). */
      if (!host_trace_raw_code(ctx, &dr, ret_cs, ret_ip, &st)) {
        if (verbose)
          printf("host_run: raw-code trace failed at %04x:%04x\n",
                 dr.cs, dr.ip);
        reason = HOST_RUN_STOP_ERROR;
        goto done;
      }
      /* dr now holds the post-raw-code state; loop back to resume exec. */
    }

    st.hook_dispatches = hydra_exec_hook_dispatch_count() - start_hooks;

    /* Push the updated state back into the guest CPU. */
    machine_to_regs(m->registers, &dr);
    if (host_set_regs(ctx, &dr) != 0) {
      reason = HOST_RUN_STOP_ERROR;
      break;
    }

    if (verbose) {
      printf("host_run: stop %04x:%04x rtype=%d hooks=%lu raw=%lu stubs=%lu\n",
             dr.cs, dr.ip, rtype,
             (unsigned long)st.hook_dispatches,
             (unsigned long)st.raw_code_runs,
             (unsigned long)st.stub_hits);
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
  }

done:
  host_bp_clear_all(ctx, bps);
  if (stats)
    *stats = st;
  return reason;
}