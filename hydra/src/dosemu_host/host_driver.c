/*
 * host_driver.c - function-level Hydra execution loop for stock dosemu2.
 */

#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <string.h>
#include <time.h>

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

static int host_bp_tracked_count(host_bp_entry_t *bps)
{
  int n = 0;
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++)
    if (bps[i].used)
      n++;
  return n;
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

static int host_bp_ensure(host_ctx_t *ctx, host_bp_entry_t *bps,
                          uint16_t seg, uint16_t off, int is_stub)
{
  uint32_t linear = ((uint32_t)seg << 4) + off;
  if (host_bp_find(bps, linear))
    return 0;

  int idx = host_set_bp(ctx, seg, off);
  if (idx < 0)
    return -1;
  if (host_bp_add(bps, idx, linear, is_stub) != 0) {
    host_clear_bp(ctx, idx);
    return -1;
  }
  return 0;
}

/* Clear every tracked software breakpoint. A failed clear remains tracked so
 * cleanup can be retried by the caller; this is especially important before
 * capture/restore because an uncleared INT3 must never enter or be overwritten
 * by a memory snapshot. */
static int host_bp_clear_all(host_ctx_t *ctx, host_bp_entry_t *bps)
{
  int failed = 0;
  for (size_t i = 0; i < HOST_RUN_MAX_BPS; i++) {
    if (!bps[i].used)
      continue;
    if (host_clear_bp(ctx, bps[i].index) != 0) {
      failed = 1;
      continue;
    }
    bps[i].used = 0;
  }
  return failed ? -1 : 0;
}

static void hook_counter(const hydra_hook_t *hook, void *user);
static void bp_visitor(const hydra_hook_t *hook, void *user);

typedef struct bp_install {
  host_ctx_t *ctx;
  host_bp_entry_t *bps;
  int count;
  int failed;
} bp_install_t;

/* Plant one debugger breakpoint for a hook at its absolute guest address. */
static int plant_hook_bp(host_ctx_t *ctx, host_bp_entry_t *bps,
                         u16 seg, uint16_t off)
{
  return host_bp_ensure(ctx, bps, seg, off, 0);
}

static void bp_visitor(const hydra_hook_t *hook, void *user)
{
  bp_install_t *ins = user;

  /* Production overlay hooks are physical entry-stub addresses carrying the
   * OVERLAY flag; the corresponding logical overlay body is separate metadata.
   * Keep the default policy fail-closed without assuming either address form. */
  if (hook->flags & HYDRA_HOOK_FLAGS_OVERLAY) {
    if (!ins->ctx->overlays_armed) {
      fprintf(stderr,
              "host_run: overlay hook " ADDR_FMT
              " requires conf overlays=armed; refusing to run\n",
              ADDR_ARG(hook->addr));
      ins->failed++;
    }
    return;
  }

  if (addr_is_overlay(hook->addr)) {
    fprintf(stderr,
            "host_run: non-overlay hook unexpectedly has logical overlay address "
            ADDR_FMT "\n", ADDR_ARG(hook->addr));
    ins->failed++;
    return;
  }

  u16 seg = (u16)(ins->ctx->code_load_offset + addr_seg(hook->addr));
  u16 off = addr_off(hook->addr);
  if (host_bp_ensure(ins->ctx, ins->bps, seg, off, 0) != 0) {
    fprintf(stderr,
            "host_run: FAILED to plant breakpoint for hook at rel %04x:%04x\n",
            addr_seg(hook->addr), addr_off(hook->addr));
    ins->failed++;
    return;
  }
  ins->count++;
}

static int install_hook_breakpoints(host_ctx_t *ctx, host_bp_entry_t *bps,
                                    int *count_out)
{
  bp_install_t ins = { ctx, bps, 0, 0 };
  hydra_hook_foreach(bp_visitor, &ins);
  if (count_out)
    *count_out = ins.count;
  return ins.failed ? -1 : 0;
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

typedef struct hook_budget {
  int static_hooks;
  int overlay_hooks;
} hook_budget_t;

static void hook_budget_cb(const hydra_hook_t *hook, void *user)
{
  hook_budget_t *b = user;
  if (hook->flags & HYDRA_HOOK_FLAGS_OVERLAY)
    b->overlay_hooks++;
  else
    b->static_hooks++;
}

/* ------------------------------------------------------------------ */
/* overlay registry (Phase 7 Item E)                                   */
/* ------------------------------------------------------------------ */

#define HOST_RUN_MAX_OVERLAYS 16
#define HOST_OVERLAY_NUMS 64

typedef enum overlay_stub_state {
  OVERLAY_STUB_UNPAGED = 0,
  OVERLAY_STUB_PAGED = 1,
} overlay_stub_state_t;

typedef struct overlay_stub_info {
  u16 phys_seg;       /* absolute guest segment of the physical entry stub */
  uint16_t off;
  uint16_t overlay_num;
  uint16_t body_off;  /* logical overlay body offset from generated metadata */
  uint16_t mapped_seg;
  uint8_t state;
} overlay_stub_info_t;

static overlay_stub_info_t overlay_stubs[HOST_RUN_MAX_OVERLAYS];
static int n_overlay_stubs;

typedef struct overlay_collect {
  u16 code_load;
  int overflow;
  int invalid;
} overlay_collect_t;

static void overlay_collect_cb(const hydra_hook_t *hook, void *user)
{
  overlay_collect_t *oc = user;
  if (!(hook->flags & HYDRA_HOOK_FLAGS_OVERLAY))
    return;

  if (n_overlay_stubs >= HOST_RUN_MAX_OVERLAYS) {
    oc->overflow = 1;
    return;
  }
  if (addr_is_overlay(hook->addr)) {
    fprintf(stderr,
            "host_run: overlay entry hook " ADDR_FMT
            " is logical; expected a physical entry stub\n",
            ADDR_ARG(hook->addr));
    oc->invalid = 1;
    return;
  }
  if (!hook->name || !hook->name[0]) {
    fprintf(stderr,
            "host_run: armed overlay hook at %04x:%04x has no metadata name; "
            "cannot resolve its logical overlay body\n",
            addr_seg(hook->addr), addr_off(hook->addr));
    oc->invalid = 1;
    return;
  }

  char body_name[256];
  int nw = snprintf(body_name, sizeof(body_name), "%s_OVERLAY", hook->name);
  if (nw < 0 || (size_t)nw >= sizeof(body_name)) {
    fprintf(stderr, "host_run: overlay metadata name is too long: %s\n",
            hook->name);
    oc->invalid = 1;
    return;
  }

  addr_t body;
  if (!hydra_function_addr(body_name, &body) || !addr_is_overlay(body)) {
    fprintf(stderr,
            "host_run: overlay hook %s has no logical %s metadata address\n",
            hook->name, body_name);
    oc->invalid = 1;
    return;
  }

  overlay_stub_info_t *os = &overlay_stubs[n_overlay_stubs++];
  memset(os, 0, sizeof(*os));
  os->phys_seg = (u16)(oc->code_load + addr_seg(hook->addr));
  os->off = addr_off(hook->addr);
  os->overlay_num = addr_overlay_num(body);
  os->body_off = addr_off(body);
  os->state = OVERLAY_STUB_UNPAGED;
}

/* Reset/rebuild the overlay registry after code_load_offset is final. */
static int overlay_registry_reset(host_ctx_t *ctx)
{
  for (int i = 0; i < n_overlay_stubs; i++)
    hydra_overlay_segment_clear(overlay_stubs[i].overlay_num);

  overlay_collect_t oc = { ctx->code_load_offset, 0, 0 };
  n_overlay_stubs = 0;
  hydra_hook_foreach(overlay_collect_cb, &oc);
  if (oc.overflow) {
    fprintf(stderr, "host_run: more than %d overlay hooks registered\n",
            HOST_RUN_MAX_OVERLAYS);
    return -1;
  }
  return oc.invalid ? -1 : 0;
}

/* ------------------------------------------------------------------ */
/* register conversion                                                 */
/* ------------------------------------------------------------------ */

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

static uint32_t regs_linear(const dosdebug_regs_t *dr)
{
  return ((uint32_t)dr->cs << 4) + dr->ip;
}

static int is_capture_stop(const dosdebug_regs_t *dr)
{
  if (HYDRA_MODE->mode != HYDRA_MODE_CAPTURE)
    return 0;
  uint16_t seg = (uint16_t)(CODE_START_SEG + addr_seg(HYDRA_MODE->capture_addr));
  return dr->cs == seg && dr->ip == addr_off(HYDRA_MODE->capture_addr);
}

static int is_restore_stop(const dosdebug_regs_t *dr)
{
  if (HYDRA_MODE->mode != HYDRA_MODE_RESTORE)
    return 0;
  addr_t entry = hydra_hook_entry_addr();
  return dr->cs == addr_seg(entry) && dr->ip == addr_off(entry);
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
static int overlay_reconcile_all(run_ctx_t *r,
                                 const overlay_stub_info_t *skip_arm);

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

    if (host_bp_find(r->bps, regs_linear(dr))) {
      if (r->verbose > 1)
        printf("  nested hook/special stop at %04x:%04x (depth %d)\n",
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
    const int capture_stop = is_capture_stop(dr);
    const int restore_stop = is_restore_stop(dr);

    if ((capture_stop || restore_stop) && host_bp_clear_all(r->ctx, r->bps) != 0) {
      fprintf(stderr,
              "host_run: failed to clear all breakpoints before %s\n",
              capture_stop ? "state capture" : "state restore");
      return HOST_RUN_STOP_ERROR;
    }

    regs_to_machine(dr, r->m->registers);
    int mode_before = HYDRA_MODE->mode;
    int ret = hydra_machine_exec(r->m, 0);

    /* Restore replaces clean memory and can also change code_load_offset.
     * Rebuild every derived breakpoint/mapping from the restored bytes before
     * the guest is released. */
    if (restore_stop && mode_before == HYDRA_MODE_RESTORE &&
        HYDRA_MODE->mode == HYDRA_MODE_NORMAL) {
      if (host_get_regs(r->ctx, dr) != 0)
        return HOST_RUN_STOP_ERROR;
      regs_to_machine(dr, r->m->registers);
      hydra_machine_notify(r->m);

      if (overlay_registry_reset(r->ctx) != 0)
        return HOST_RUN_STOP_ERROR;
      int count = 0;
      if (install_hook_breakpoints(r->ctx, r->bps, &count) != 0) {
        fprintf(stderr,
                "host_run: failed to re-arm hook breakpoints after state restore\n");
        return HOST_RUN_STOP_ERROR;
      }
      if (r->ctx->overlays_armed && overlay_reconcile_all(r, NULL) != 0) {
        fprintf(stderr,
                "host_run: failed to reconstruct overlay state after restore\n");
        return HOST_RUN_STOP_ERROR;
      }
      r->st->hook_breakpoints = (uint64_t)host_bp_tracked_count(r->bps);
      return HOST_RUN_STOP_NONE;
    }

    hydra_machine_notify(r->m);
    int rtype = hydra_exec_last_result_type();

    if (ret == 0)
      break;

    if (ret == 2) {
      if (host_get_regs(r->ctx, dr) != 0)
        return HOST_RUN_STOP_ERROR;
      regs_to_machine(dr, r->m->registers);
      if (overlay_registry_reset(r->ctx) != 0)
        return HOST_RUN_STOP_ERROR;
      if (install_hook_breakpoints(r->ctx, r->bps, NULL) != 0)
        return HOST_RUN_STOP_ERROR;
      if (r->ctx->overlays_armed && overlay_reconcile_all(r, NULL) != 0)
        return HOST_RUN_STOP_ERROR;
      return HOST_RUN_STOP_NONE;
    }

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

    dosdebug_regs_t base = *dr;
    machine_to_regs(r->m->registers, dr);
    if (host_set_regs_diff(r->ctx, dr, &base) != 0)
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

  dosdebug_regs_t base = *dr;
  machine_to_regs(r->m->registers, dr);
  if (host_set_regs_diff(r->ctx, dr, &base) != 0)
    return HOST_RUN_STOP_ERROR;

  return HOST_RUN_STOP_NONE;
}

static int install_special_mode_breakpoint(host_ctx_t *ctx,
                                           host_bp_entry_t *bps)
{
  if (HYDRA_MODE->mode == HYDRA_MODE_CAPTURE) {
    uint16_t seg = (uint16_t)(CODE_START_SEG + addr_seg(HYDRA_MODE->capture_addr));
    uint16_t off = addr_off(HYDRA_MODE->capture_addr);
    if (host_bp_ensure(ctx, bps, seg, off, 1) != 0) {
      fprintf(stderr,
              "host_run: failed to plant capture breakpoint at %04x:%04x\n",
              seg, off);
      return -1;
    }
  } else if (HYDRA_MODE->mode == HYDRA_MODE_RESTORE) {
    addr_t entry = hydra_hook_entry_addr();
    if (host_bp_ensure(ctx, bps, addr_seg(entry), addr_off(entry), 1) != 0) {
      fprintf(stderr,
              "host_run: failed to plant restore-entry breakpoint at %04x:%04x\n",
              addr_seg(entry), addr_off(entry));
      return -1;
    }
  }
  return 0;
}

/* ------------------------------------------------------------------ */
/* overlay arming / reconciliation (Phase 7 Item E)                    */
/* ------------------------------------------------------------------ */

static uint32_t overlay_linear(const overlay_stub_info_t *os)
{
  return ((uint32_t)os->phys_seg << 4) + os->off;
}

static overlay_stub_info_t *overlay_find_stub(uint32_t linear)
{
  for (int i = 0; i < n_overlay_stubs; i++)
    if (overlay_linear(&overlay_stubs[i]) == linear)
      return &overlay_stubs[i];
  return NULL;
}

static int overlay_replant_bp(host_ctx_t *ctx, host_bp_entry_t *bps,
                              const overlay_stub_info_t *os)
{
  uint32_t linear = overlay_linear(os);
  host_bp_entry_t *stale = host_bp_find(bps, linear);
  if (stale)
    stale->used = 0;
  return plant_hook_bp(ctx, bps, os->phys_seg, os->off);
}

/* Classify a stopped-state stub. dosemu's r0 clears live INT3 patches before
 * it prints the register dump, so the shared mapping must contain clean CD3F
 * or EA bytes here. Anything else is corruption, not an unresolved state. */
static int overlay_classify(run_ctx_t *r, overlay_stub_info_t *os)
{
  uint32_t lin = overlay_linear(os);
  uint8_t b0 = lowmem_read8(r->ctx->lm, lin);
  uint8_t b1 = lowmem_read8(r->ctx->lm, lin + 1);

  if (b0 == 0xCD && b1 == 0x3F) {
    host_bp_entry_t *stale = host_bp_find(r->bps, lin);
    if (stale)
      stale->used = 0; /* pager/invalidation overwrote it server-side */
    os->state = OVERLAY_STUB_UNPAGED;
    os->mapped_seg = 0;
    return 0;
  }

  if (b0 == 0xEA) {
    uint16_t off = lowmem_read16(r->ctx->lm, lin + 1);
    uint16_t seg = lowmem_read16(r->ctx->lm, lin + 3);
    if (off != os->body_off) {
      fprintf(stderr,
              "host_run: overlay %u stub %04x:%04x jumps to unexpected "
              "offset %04x (metadata body offset %04x)\n",
              os->overlay_num, os->phys_seg, os->off, off, os->body_off);
      return -1;
    }
    os->state = OVERLAY_STUB_PAGED;
    os->mapped_seg = seg;
    return 1;
  }

  fprintf(stderr,
          "host_run: malformed overlay %u stub %04x:%04x: "
          "expected CD 3F or EA, found %02x %02x\n",
          os->overlay_num, os->phys_seg, os->off, b0, b1);
  return -1;
}

/* Rebuild core overlay mappings from clean guest bytes, then ensure every
 * paged entry stub has a debugger breakpoint. Re-running this is idempotent.
 * skip_arm is used only during the first page-in: its body executes natively
 * once before the stub is armed. */
static int overlay_reconcile_all(run_ctx_t *r,
                                 const overlay_stub_info_t *skip_arm)
{
  uint8_t seen[HOST_OVERLAY_NUMS] = {0};
  uint16_t segs[HOST_OVERLAY_NUMS] = {0};

  for (int i = 0; i < n_overlay_stubs; i++) {
    overlay_stub_info_t *os = &overlay_stubs[i];
    int kind = overlay_classify(r, os);
    if (kind < 0)
      return -1;
    if (kind == 0)
      continue;
    if (os->overlay_num >= HOST_OVERLAY_NUMS) {
      fprintf(stderr, "host_run: invalid overlay number %u\n", os->overlay_num);
      return -1;
    }
    if (seen[os->overlay_num] && segs[os->overlay_num] != os->mapped_seg) {
      fprintf(stderr,
              "host_run: overlay %u entry stubs disagree on mapped segment "
              "%04x vs %04x\n",
              os->overlay_num, segs[os->overlay_num], os->mapped_seg);
      return -1;
    }
    seen[os->overlay_num] = 1;
    segs[os->overlay_num] = os->mapped_seg;
  }

  /* Update each logical overlay exactly once. */
  uint8_t touched[HOST_OVERLAY_NUMS] = {0};
  for (int i = 0; i < n_overlay_stubs; i++) {
    unsigned n = overlay_stubs[i].overlay_num;
    if (touched[n])
      continue;
    touched[n] = 1;
    if (seen[n])
      hydra_overlay_segment_set((u16)n, segs[n]);
    else
      hydra_overlay_segment_clear((u16)n);
  }

  for (int i = 0; i < n_overlay_stubs; i++) {
    overlay_stub_info_t *os = &overlay_stubs[i];
    if (os->state != OVERLAY_STUB_PAGED || os == skip_arm)
      continue;
    if (overlay_replant_bp(r->ctx, r->bps, os) != 0) {
      fprintf(stderr,
              "host_run: failed to arm overlay %u stub %04x:%04x\n",
              os->overlay_num, os->phys_seg, os->off);
      return -1;
    }
  }

  r->st->hook_breakpoints = (uint64_t)host_bp_tracked_count(r->bps);
  return 0;
}

/* First page-in is intercepted by dosemu's non-patching BPINT 3F. The host
 * steps into the guest pager (preserving the real interrupt frame), traces it
 * until IRET rewinds to the now-EA stub, then jumps directly to the paged body
 * for the first native execution. Crucially the EA stub itself is never fetched
 * after the pager's guest write, so planting CC after the native body returns
 * cannot lose to a stale simx86 EA translation. */
static int overlay_handle_pagein(run_ctx_t *r, overlay_stub_info_t *os,
                                 dosdebug_regs_t *dr, int depth)
{
  if (depth > HOST_RUN_MAX_DEPTH) {
    fprintf(stderr, "host_run: nested overlay page-in depth exceeded\n");
    return -1;
  }

  if (overlay_classify(r, os) != 0) {
    fprintf(stderr,
            "host_run: BPINT 3f stop at %04x:%04x is not an unpaged overlay stub\n",
            dr->cs, dr->ip);
    return -1;
  }

  const size_t max_steps =
      (r->opts && r->opts->max_trace_steps) ? r->opts->max_trace_steps
                                            : HOST_RUN_DEFAULT_TRACE_STEPS;
  const int step_timeout =
      (r->timeout_ms < 0 || r->timeout_ms > HOST_STEP_TIMEOUT_MS)
          ? HOST_STEP_TIMEOUT_MS : r->timeout_ms;

  /* ti executes the INT instruction with its real stacked return frame and
   * stops at the interrupt handler entry. */
  if (dosdebug_step_into(r->ctx->db) != 0 ||
      dosdebug_wait_stop(r->ctx->db, dr, step_timeout) != 0) {
    fprintf(stderr, "host_run: failed to step into overlay INT 3f pager\n");
    return -1;
  }
  dosdebug_drain(r->ctx->db);

  /* Trace the pager until its IRET returns to the same stub. */
  size_t i;
  for (i = 0; i < max_steps; i++) {
    if (dr->cs == os->phys_seg && dr->ip == os->off)
      break;
    if (dosdebug_step(r->ctx->db) != 0 ||
        dosdebug_wait_stop(r->ctx->db, dr, step_timeout) != 0) {
      fprintf(stderr, "host_run: overlay pager trace failed\n");
      return -1;
    }
    dosdebug_drain(r->ctx->db);
  }
  if (i == max_steps) {
    fprintf(stderr,
            "host_run: overlay pager did not return to %04x:%04x within %zu steps\n",
            os->phys_seg, os->off, max_steps);
    return -1;
  }

  if (overlay_classify(r, os) != 1) {
    fprintf(stderr,
            "host_run: overlay pager returned without producing a valid EA stub\n");
    return -1;
  }

  hydra_overlay_segment_set(os->overlay_num, os->mapped_seg);

  /* The far-call frame is still on the caller stack; EA would only change
   * CS:IP, so redirect straight to the native body and leave the frame intact. */
  uint32_t sp_lin = ((uint32_t)dr->ss << 4) + dr->sp;
  uint16_t ret_ip = lowmem_read16(r->ctx->lm, sp_lin);
  uint16_t ret_cs = lowmem_read16(r->ctx->lm, sp_lin + 2);
  dosdebug_regs_t base = *dr;
  dr->cs = os->mapped_seg;
  dr->ip = os->body_off;
  if (host_set_regs_diff(r->ctx, dr, &base) != 0)
    return -1;

  /* Execute the paged body natively once, single-stepping so nested static
   * hooks are still dispatched before their instruction executes. */
  for (i = 0; i < max_steps; i++) {
    if (dr->cs == ret_cs && dr->ip == ret_ip)
      break;

    overlay_stub_info_t *nested = overlay_find_stub(regs_linear(dr));
    if (nested && overlay_classify(r, nested) == 0) {
      if (overlay_handle_pagein(r, nested, dr, depth + 1) != 0)
        return -1;
      continue;
    }

    if (host_bp_find(r->bps, regs_linear(dr))) {
      host_run_stop_reason_t dn = dispatch_hook(r, dr, depth + 1);
      if (dn != HOST_RUN_STOP_NONE)
        return -1;
      continue;
    }

    if (dosdebug_step(r->ctx->db) != 0 ||
        dosdebug_wait_stop(r->ctx->db, dr, step_timeout) != 0) {
      fprintf(stderr, "host_run: native overlay body trace failed\n");
      return -1;
    }
    dosdebug_drain(r->ctx->db);
  }
  if (i == max_steps) {
    fprintf(stderr,
            "host_run: native overlay body did not return to %04x:%04x within %zu steps\n",
            ret_cs, ret_ip, max_steps);
    return -1;
  }

  /* First native call is complete; now arm the clean EA stub before any
   * subsequent guest instruction can call it again. */
  if (overlay_reconcile_all(r, NULL) != 0)
    return -1;

  if (r->verbose)
    printf("host_run: overlay %u paged at %04x:%04x -> %04x:%04x; "
           "first body call native, stub now armed\n",
           os->overlay_num, os->phys_seg, os->off,
           os->mapped_seg, os->body_off);
  return 0;
}

/* ------------------------------------------------------------------ */
/* MZ (.exe) guest loading via dosemu2 'bpload' (Phase 7 Item C)       */
/* ------------------------------------------------------------------ */

#define HOST_MZ_DEFAULT_TIMEOUT_MS 60000

static int mz_entry_validate(host_ctx_t *ctx, const dosdebug_regs_t *dr,
                             uint16_t e_cs, uint16_t e_ip, uint16_t *psp_out)
{
  uint16_t psp = dr->ds;
  uint16_t want_cs = (uint16_t)(psp + 0x10u + e_cs);

  if (((dr->cs & 0xffffu) != want_cs || dr->ip != e_ip))
    return 0;
  if (dr->es != dr->ds)
    return 0;

  uint8_t sig0 = lowmem_read8(ctx->lm, ((uint32_t)psp << 4));
  uint8_t sig1 = lowmem_read8(ctx->lm, ((uint32_t)psp << 4) + 1);
  if (sig0 != 0xCD || sig1 != 0x20)
    return 0;

  uint16_t owner = lowmem_read16(ctx->lm, (((uint32_t)(psp - 1)) << 4) + 1);
  if (owner != psp)
    return 0;

  *psp_out = psp;
  return 1;
}

/* Wait for — and validate — the MZ guest's entry stop. */
static int mz_load_and_wait(host_ctx_t *ctx, const host_run_options_t *opts,
                            uint16_t *psp_out)
{
  const int timeout_ms = (opts->mz_timeout_ms > 0) ? opts->mz_timeout_ms
                                                   : HOST_MZ_DEFAULT_TIMEOUT_MS;
  struct timespec t0;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  uint16_t psp = 0;

  if (opts->mz_parked_at_entry) {
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0 ||
        !mz_entry_validate(ctx, &dr, opts->mz_entry_cs, opts->mz_entry_ip,
                           &psp)) {
      fprintf(stderr, "host_run: parked state is not a validated MZ entry stop\n");
      return -1;
    }
    if (opts->verbose)
      printf("host_run: MZ guest loaded, PSP=%04x, load seg=%04x\n",
             psp, (uint16_t)(psp + 0x10u));
    *psp_out = psp;
    return 0;
  }

  if (dosdebug_go(ctx->db) != 0) {
    fprintf(stderr, "host_run: go after launcher release failed\n");
    return -1;
  }

  for (;;) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    long elapsed = (long)(now.tv_sec - t0.tv_sec) * 1000
                 + (now.tv_nsec - t0.tv_nsec) / 1000000;
    long remaining = (long)timeout_ms - elapsed;
    if (remaining <= 0) {
      fprintf(stderr, "host_run: no MZ entry stop within %dms "
              "(did the loader issue its EXEC?)\n", timeout_ms);
      return -1;
    }
    if (!dosdebug_is_alive(ctx->db)) {
      fprintf(stderr, "host_run: dosemu2 died while waiting for the MZ entry stop\n");
      return -1;
    }

    long slice = remaining > 8000 ? 8000 : remaining;
    dosdebug_regs_t dr;
    if (dosdebug_wait_stop(ctx->db, &dr, (int)slice) != 0)
      continue;

    if (mz_entry_validate(ctx, &dr, opts->mz_entry_cs, opts->mz_entry_ip,
                          &psp)) {
      if (opts->verbose)
        printf("host_run: MZ guest loaded, PSP=%04x, load seg=%04x\n",
               psp, (uint16_t)(psp + 0x10u));
      *psp_out = psp;
      return 0;
    }

    fprintf(stderr, "host_run: skipping non-entry stop at %04x:%04x "
            "(ds=%04x es=%04x ax=%04x) during MZ load\n",
            dr.cs, dr.ip, dr.ds, dr.es, dr.ax);
    dosdebug_drain(ctx->db);
    dosdebug_go(ctx->db);
  }
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
  int overlay_bpint = 0;
  host_run_stop_reason_t reason = HOST_RUN_STOP_NONE;

  if (!host_raw_code_ready(ctx)) {
    fprintf(stderr,
            "host_run: no guest-owned raw-code region reserved; call host_reserve_raw_code() first\n");
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

  hook_budget_t budget = {0};
  hydra_hook_foreach(hook_budget_cb, &budget);
  int special_bps = (HYDRA_MODE->mode == HYDRA_MODE_CAPTURE ||
                     HYDRA_MODE->mode == HYDRA_MODE_RESTORE) ? 1 : 0;
  int requested_bps = budget.static_hooks + budget.overlay_hooks + special_bps;
  if (budget.overlay_hooks > HOST_RUN_MAX_OVERLAYS) {
    fprintf(stderr,
            "host_run: %d overlay hooks exceed host overlay-site limit %d\n",
            budget.overlay_hooks, HOST_RUN_MAX_OVERLAYS);
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }
  if (requested_bps > HOST_RUN_MAX_BPS) {
    fprintf(stderr,
            "host_run: worst-case %d hook/special breakpoints exceed dosdebug's "
            "%d-breakpoint limit (%d static + %d overlay + %d special)\n",
            requested_bps, HOST_RUN_MAX_BPS, budget.static_hooks,
            budget.overlay_hooks, special_bps);
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

  if (opts && opts->mz_load) {
    uint16_t psp = 0;
    if (mz_load_and_wait(ctx, opts, &psp) != 0) {
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
    st.mz_psp = psp;
    ctx->mz_psp = psp;
    host_set_code_load(ctx, (u16)(psp + 0x10u));
    if (verbose)
      printf("host_run: MZ guest loaded, PSP=%04x, load seg=%04x\n",
             psp, ctx->code_load_offset);
  }

  int static_count = 0;
  if (install_hook_breakpoints(ctx, bps, &static_count) != 0 ||
      overlay_registry_reset(ctx) != 0) {
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

  if (install_special_mode_breakpoint(ctx, bps) != 0) {
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

  run_ctx_t r = { ctx, m, bps, &st, opts, timeout_ms, verbose };

  /* Initial classification catches malformed stubs before guest execution and
   * eagerly arms already-paged stubs (including a restored paged snapshot). */
  if (ctx->overlays_armed && n_overlay_stubs > 0) {
    if (overlay_reconcile_all(&r, NULL) != 0) {
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
    if (dosdebug_set_bpint(ctx->db, 0x3f) != 0) {
      fprintf(stderr, "host_run: failed to arm run-owned BPINT 3f\n");
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
    overlay_bpint = 1;
  }
  st.hook_breakpoints = (uint64_t)host_bp_tracked_count(bps);

  if (opts && opts->before_go_fn &&
      opts->before_go_fn(ctx, opts->before_go_user) != 0) {
    fprintf(stderr, "host_run: before_go callback failed; guest not released\n");
    reason = HOST_RUN_STOP_ERROR;
    goto done;
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

    host_bp_entry_t *bp = host_bp_find(bps, regs_linear(&dr));
    int pagein_handled = 0;

    if (!bp) {
      overlay_stub_info_t *os = overlay_find_stub(regs_linear(&dr));
      if (overlay_bpint && os &&
          lowmem_read8(ctx->lm, overlay_linear(os)) == 0xCD &&
          lowmem_read8(ctx->lm, overlay_linear(os) + 1) == 0x3F) {
        if (overlay_handle_pagein(&r, os, &dr, 0) != 0) {
          reason = HOST_RUN_STOP_ERROR;
          break;
        }
        pagein_handled = 1;
      } else if (overlay_bpint &&
                 lowmem_read8(ctx->lm, regs_linear(&dr)) == 0xCD &&
                 lowmem_read8(ctx->lm, regs_linear(&dr) + 1) == 0x3F) {
        /* BPINT is interrupt-wide. Preserve unrelated INT 3F behavior by
         * stepping into its handler, then let the next GO run it normally. */
        if (dosdebug_step_into(ctx->db) != 0 ||
            dosdebug_wait_stop(ctx->db, &dr, timeout_ms) != 0) {
          fprintf(stderr, "host_run: failed to pass through unrelated INT 3f\n");
          reason = HOST_RUN_STOP_ERROR;
          break;
        }
        dosdebug_drain(ctx->db);
        continue;
      } else {
        fprintf(stderr,
                "host_run: unexpected debugger stop at %04x:%04x "
                "(not a tracked Hydra stop)\n",
                dr.cs, dr.ip);
        reason = HOST_RUN_STOP_ERROR;
        break;
      }
    }

    if (!pagein_handled) {
      if (ctx->overlays_armed && overlay_reconcile_all(&r, NULL) != 0) {
        reason = HOST_RUN_STOP_ERROR;
        break;
      }

      host_run_stop_reason_t dn = dispatch_hook(&r, &dr, 0);
      if (dn != HOST_RUN_STOP_NONE) {
        reason = dn;
        break;
      }
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

done:
  if (overlay_bpint && dosdebug_is_alive(ctx->db)) {
    if (dosdebug_clear_bpint(ctx->db, 0x3f) != 0) {
      fprintf(stderr, "host_run: failed to clear run-owned BPINT 3f\n");
      if (reason != HOST_RUN_STOP_DOSEMU_EXIT)
        reason = HOST_RUN_STOP_ERROR;
    }
  }
  if (host_bp_clear_all(ctx, bps) != 0) {
    fprintf(stderr, "host_run: failed to clear all breakpoints during cleanup\n");
    if (reason != HOST_RUN_STOP_DOSEMU_EXIT)
      reason = HOST_RUN_STOP_ERROR;
  }
  if (stats)
    *stats = st;
  return reason;
}
