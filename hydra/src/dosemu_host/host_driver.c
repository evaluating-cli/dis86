/*
 * host_driver.c - function-level Hydra execution loop for stock dosemu2.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <string.h>

#include "host_driver.h"
#include "internal.h"

typedef struct host_bp_entry {
  uint32_t linear;
  int index;
  uint8_t used;
  uint8_t is_stub;
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
    return 0;
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (!bps[i].used) {
      bps[i].used = 1;
      bps[i].index = index;
      bps[i].linear = linear;
      bps[i].is_stub = (uint8_t)is_stub;
      return 0;
    }
  }
  return -1;
}

static void host_bp_clear_all(host_ctx_t *ctx, host_bp_entry_t *bps)
{
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (bps[i].used)
      host_clear_bp(ctx, bps[i].index);
    bps[i].used = 0;
  }
}

static void hook_counter(const hydra_hook_t *hook, void *user);
static void bp_visitor(const hydra_hook_t *hook, void *user);

typedef struct bp_install {
  host_ctx_t *ctx;
  host_bp_entry_t *bps;
  int count;
  int failed;
} bp_install_t;

static void bp_visitor(const hydra_hook_t *hook, void *user)
{
  bp_install_t *ins = user;

  /* The external debugger cannot discover a logical overlay hook before the
   * overlay-to-physical segment mapping exists. Silently skipping such hooks
   * is incorrect because guest code would execute unhooked. Until dynamic
   * overlay breakpoint re-arming is implemented, reject the run before any
   * guest instruction is released. */
  if (hook->flags & HYDRA_HOOK_FLAGS_OVERLAY) {
    fprintf(stderr,
            "host_run: overlay hook %u:%04x is unsupported by the dosdebug host; refusing to run\n",
            (unsigned)addr_overlay_num(hook->addr), (unsigned)addr_off(hook->addr));
    ins->failed++;
    return;
  }

  u16 seg = (u16)(ins->ctx->code_load_offset + addr_seg(hook->addr));
  u16 off = addr_off(hook->addr);
  int idx = host_set_bp(ins->ctx, seg, off);
  if (idx < 0) {
    fprintf(stderr,
            "host_run: FAILED to plant breakpoint for hook at rel %04x:%04x\n",
            addr_seg(hook->addr), addr_off(hook->addr));
    ins->failed++;
    return;
  }
  uint32_t linear = ((uint32_t)seg << 4) + off;
  if (host_bp_add(ins->bps, idx, linear, 0) != 0) {
    host_clear_bp(ins->ctx, idx);
    fprintf(stderr,
            "host_run: breakpoint table full (%d); hook at rel %04x:%04x dropped\n",
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

static void regs_to_machine(const dosdebug_regs_t *dr,
                            hydra_machine_registers_t *hr)
{
  hr->ax = dr->ax;   hr->bx = dr->bx;   hr->cx = dr->cx;   hr->dx = dr->dx;
  hr->si = dr->si;   hr->di = dr->di;   hr->bp = dr->bp;   hr->sp = dr->sp;
  hr->ip = dr->ip;   hr->cs = dr->cs;   hr->ds = dr->ds;   hr->es = dr->es;
  hr->ss = dr->ss;   hr->flags = dr->flags;
}

static void machine_to_regs(const hydra_machine_registers_t *hr,
                            dosdebug_regs_t *dr)
{
  dr->ax = hr->ax;   dr->bx = hr->bx;   dr->cx = hr->cx;   dr->dx = hr->dx;
  dr->si = hr->si;   dr->di = hr->di;   dr->bp = hr->bp;   dr->sp = hr->sp;
  dr->ip = hr->ip;   dr->cs = hr->cs;   dr->ds = hr->ds;   dr->es = hr->es;
  dr->ss = hr->ss;   dr->flags = hr->flags;
}

#define HOST_RUN_DEFAULT_TRACE_STEPS 10000u
#define HOST_STEP_TIMEOUT_MS 10000
#define HOST_RUN_MAX_DEPTH 8
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

static host_run_stop_reason_t trace_to_return(run_ctx_t *r, dosdebug_regs_t *dr,
                                              uint16_t ret_cs, uint16_t ret_ip,
                                              int depth)
{
  const size_t max_steps =
      (r->opts && r->opts->max_trace_steps) ? r->opts->max_trace_steps
                                            : HOST_RUN_DEFAULT_TRACE_STEPS;
  const int step_timeout =
      (r->timeout_ms < 0 || r->timeout_ms > HOST_STEP_TIMEOUT_MS)
          ? HOST_STEP_TIMEOUT_MS : r->timeout_ms;

  for (size_t i = 0; i < max_steps; i++) {
    if (dosdebug_step(r->ctx->db) != 0)
      return HOST_RUN_STOP_ERROR;
    if (dosdebug_wait_stop(r->ctx->db, dr, step_timeout) != 0) {
      fprintf(stderr,
              "host_run: guest step did not complete within %dms at %04x:%04x\n",
              step_timeout, dr->cs, dr->ip);
      return HOST_RUN_STOP_ERROR;
    }
    dosdebug_drain(r->ctx->db);

    if (r->verbose > 1)
      printf("  trace[%zu] %04x:%04x (want %04x:%04x)\n",
             i, dr->cs, dr->ip, ret_cs, ret_ip);

    if (dr->cs == ret_cs && dr->ip == ret_ip) {
      r->st->raw_code_returns++;
      return HOST_RUN_STOP_NONE;
    }

    uint32_t lin = ((uint32_t)dr->cs << 4) + dr->ip;
    if (host_bp_find(r->bps, lin)) {
      if (r->verbose > 1)
        printf("  nested hook at %04x:%04x (depth %d)\n",
               dr->cs, dr->ip, depth + 1);
      host_run_stop_reason_t dn = dispatch_hook(r, dr, depth + 1);
      if (dn != HOST_RUN_STOP_NONE)
        return dn;
    }
  }

  fprintf(stderr,
          "host_run: trace budget (%zu steps) exhausted before return to %04x:%04x (cpu at %04x:%04x)\n",
          max_steps, ret_cs, ret_ip, dr->cs, dr->ip);
  return HOST_RUN_STOP_ERROR;
}

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
      break;

    r->st->redirects++;
    if (++redirects_this_dispatch > HOST_RUN_MAX_REDIRECTS) {
      fprintf(stderr, "host_run: redirect cap (%d) exceeded at %04x:%04x\n",
              HOST_RUN_MAX_REDIRECTS, dr->cs, dr->ip);
      return HOST_RUN_STOP_ERROR;
    }

    if (rtype != HYDRA_RESULT_TYPE_CALL &&
        rtype != HYDRA_RESULT_TYPE_CALL_NEAR)
      break;

    r->st->raw_code_runs++;

    machine_to_regs(r->m->registers, dr);
    if (host_set_regs(r->ctx, dr) != 0)
      return HOST_RUN_STOP_ERROR;

    u16 eid = hydra_exec_active_id();
    uint16_t ret_cs, ret_ip;
    if (rtype == HYDRA_RESULT_TYPE_CALL) {
      ret_cs = 0xffff;
      ret_ip = eid;
    } else {
      ret_cs = r->m->registers->cs;
      ret_ip = (uint16_t)(0xff00 + eid);
    }

    host_run_stop_reason_t tr = trace_to_return(r, dr, ret_cs, ret_ip, depth);
    if (tr != HOST_RUN_STOP_NONE)
      return tr;
  }

  machine_to_regs(r->m->registers, dr);
  if (host_set_regs(r->ctx, dr) != 0)
    return HOST_RUN_STOP_ERROR;

  return HOST_RUN_STOP_NONE;
}

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

  /* Fail before releasing the CPU on every known unsupported/resource case. */
  if (!host_raw_code_ready(ctx)) {
    fprintf(stderr,
            "host_run: no guest-owned raw-code region reserved; call host_reserve_raw_code() first\n");
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }
  int requested_bps = host_hook_breakpoint_count(ctx);
  if (requested_bps > HOST_RUN_MAX_BPS) {
    fprintf(stderr,
            "host_run: %d static hooks exceed dosdebug's %d-breakpoint limit\n",
            requested_bps, HOST_RUN_MAX_BPS);
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

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

      uint32_t stop_linear = ((uint32_t)dr.cs << 4) + dr.ip;
      if (!host_bp_find(bps, stop_linear)) {
        fprintf(stderr,
                "host_run: unexpected debugger stop at %04x:%04x (not a Hydra hook)\n",
                dr.cs, dr.ip);
        reason = HOST_RUN_STOP_ERROR;
        break;
      }

      hydra_impl_raw_code_reset();

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
    }
  }

done:
  host_bp_clear_all(ctx, bps);
  if (stats)
    *stats = st;
  return reason;
}
