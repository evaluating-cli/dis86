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

    /* Breakpoints are software patches in guest memory. A capture must not
     * serialize those patches, and a restore must not overwrite live patched
     * bytes underneath dosdebug's breakpoint table. Require every tracked bp
     * to be removed before either memory-image operation. */
    if ((capture_stop || restore_stop) && host_bp_clear_all(r->ctx, r->bps) != 0) {
      fprintf(stderr,
              "host_run: failed to clear all breakpoints before %s\n",
              capture_stop ? "state capture" : "state restore");
      return HOST_RUN_STOP_ERROR;
    }

    regs_to_machine(dr, r->m->registers);
    int mode_before = HYDRA_MODE->mode;
    int ret = hydra_machine_exec(r->m, 0);

    /* state_restore() updates the real CPU directly through the host vtable.
     * The machine struct still contains the pre-restore copy, so pull the
     * restored CPU back before any generic push can overwrite it. Then re-arm
     * static hook breakpoints against the restored memory image. */
    if (restore_stop && mode_before == HYDRA_MODE_RESTORE &&
        HYDRA_MODE->mode == HYDRA_MODE_NORMAL) {
      if (host_get_regs(r->ctx, dr) != 0)
        return HOST_RUN_STOP_ERROR;
      regs_to_machine(dr, r->m->registers);
      hydra_machine_notify(r->m);

      int count = 0;
      if (install_hook_breakpoints(r->ctx, r->bps, &count) != 0) {
        fprintf(stderr,
                "host_run: failed to re-arm hook breakpoints after state restore\n");
        return HOST_RUN_STOP_ERROR;
      }
      r->st->hook_breakpoints = (uint64_t)count;
      return HOST_RUN_STOP_NONE;
    }

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

    /* dr holds the last parsed dump from the real CPU (bp stop via
     * host_go_and_wait, trace steps via dosdebug_wait_stop, nested
     * completions via their verified final push) — i.e. the live state the
     * diff-write below can skip. Capture it before machine_to_regs clobbers
     * dr. */
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

  /* Push the final state into the guest CPU (idempotent for the caller of
   * host_run; required before a nested trace continues stepping). Same base
   * provenance as the redirect push above. The recursion's own final push
   * handles its own diff base; trace_to_return itself is untouched. */
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
/* MZ (.exe) guest loading via dosemu2 'bpload' (Phase 7 Item C)       */
/* ------------------------------------------------------------------ */

/* Default budget for the whole bpload phase (arm → EXEC interception →
 * load → entry stop). Headless dosemu boots in seconds; keep margin. */
#define HOST_MZ_DEFAULT_TIMEOUT_MS 60000

/*
 * Validate one debugger stop dump as "the loaded program's entry":
 *   - CS == PSP+0x10+e_cs and IP == e_ip   (relocated entry, PSP = DS)
 *   - DS == ES                             (DOS gives both = PSP)
 *   - PSP:0 holds 'INT 20h' (CD 20)        (PSP signature)
 *   - word at ((PSP-1)<<4)+1 == PSP        (MCB immediately before the
 *                                           PSP owned by the program)
 */
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

/*
 * Wait for — and validate — the MZ guest's entry stop.
 *
 * The guest is loaded by a real-mode launcher .COM (see launch.asm): the
 * harness parks the machine inside the launcher's spin loop, then calls
 * into host_run, which releases it. The launcher issues INT21 AH=4B01
 * (load-don't-execute) for TESTPROG.EXE and performs the handoff to the
 * child itself: switch to the child's initial stack, DS=ES=child PSP,
 * zeroed GP registers, TF set, far-jump to the relocated entry. The
 * single-step trap stops the debugger AT the entry with no executed image
 * byte; this loop validates that stop against the parsed MZ header.
 *
 * Why not dosemu2's own bpload/DBGload machinery: stock fdpp never
 * publishes the 4B01 results (initial SS:SP / entry CS:IP, RBIL EXEC-
 * paramblock offsets 0Eh/12h) back into guest memory — neither into the
 * caller's parameter block nor into dosemu's DBGload block in the BIOS —
 * so dosemu's stub jumps to 0000:0000 and kills dosemu with SIGILL.
 * Verified against dosemu2 2.0pre9 + libfdpp 1.11 (2026-08); see
 * dosdebug_bpload() for the arming wrapper kept for future DOS versions.
 *
 * Everything seen on the way (register dumps mid-run, RVC door traps from
 * other simulated int21s) is resumed transparently; only a dump passing
 * every identity check in mz_entry_validate ends the loop. Registers must
 * NEVER be written between an RVC trap and its resume: that corrupts
 * dosemu's reflection handshake; plain 'g' resumes cleanly.
 *
 * On success *psp_out holds the guest's PSP segment.
 */
static int mz_load_and_wait(host_ctx_t *ctx, const host_run_options_t *opts,
                            uint16_t *psp_out)
{
  const int timeout_ms = (opts->mz_timeout_ms > 0) ? opts->mz_timeout_ms
                                                   : HOST_MZ_DEFAULT_TIMEOUT_MS;
  struct timespec t0;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  uint16_t psp = 0;

  /* Harness-parked variant: the machine already sits at the entry; only
   * validate. (See launch.asm / the harness for how this state is
   * produced without relying on fdpp's broken 4B01 result publishing.) */
  if (opts->mz_parked_at_entry) {
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0 ||
        !mz_entry_validate(ctx, &dr, opts->mz_entry_cs, opts->mz_entry_ip,
                           &psp)) {
      fprintf(stderr, "host_run: parked state is not a validated MZ "
              "entry stop\n");
      return -1;
    }
    if (opts->verbose)
      printf("host_run: MZ guest loaded, PSP=%04x, load seg=%04x\n",
             psp, (uint16_t)(psp + 0x10u));
    *psp_out = psp;
    return 0;
  }

  /* The harness left the machine parked inside the launcher's spin loop
   * after setting the go flag; release it now. */
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
      fprintf(stderr, "host_run: dosemu2 died while waiting for the "
              "MZ entry stop\n");
      return -1;
    }

    /* Bound each individual wait so stream hiccups cannot eat the whole
     * budget unnoticed. */
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
  host_run_stop_reason_t reason = HOST_RUN_STOP_NONE;

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

  /* MZ (.exe) guests: load via bpload and validate the entry BEFORE the
   * hook breakpoints are planted (bp_visitor resolves image-relative hook
   * addresses against code_load_offset, which only becomes known here). */
  if (opts && opts->mz_load) {
    uint16_t psp = 0;
    if (mz_load_and_wait(ctx, opts, &psp) != 0) {
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
    st.mz_psp = psp;
    /* The image lives at PSP+0x10; all core logic now works with
     * image-relative (CODE_START_SEG-relative) addresses unchanged. */
    host_set_code_load(ctx, (u16)(psp + 0x10u));
    if (verbose)
      printf("host_run: MZ guest loaded, PSP=%04x, load seg=%04x\n",
             psp, ctx->code_load_offset);
  }

  /* Plant a breakpoint at every registered hook. Every hook MUST get one;
   * a silent drop means guest code runs unhooked — refuse to start. */
  {
    int count = 0;
    if (install_hook_breakpoints(ctx, bps, &count) != 0) {
      reason = HOST_RUN_STOP_ERROR;
      goto done;
    }
    st.hook_breakpoints = (uint64_t)count;
  }

  /* Capture and restore are instruction-boundary modes in the core. The
   * external host therefore needs explicit breakpoints for those non-hook
   * addresses; otherwise the special-mode code is unreachable. */
  if (install_special_mode_breakpoint(ctx, bps) != 0) {
    reason = HOST_RUN_STOP_ERROR;
    goto done;
  }

  /* Some guests use a memory flag or equivalent handoff to leave a parked
   * wait loop. Perform that release only after every run-owned breakpoint is
   * armed, while dosdebug still has the CPU stopped. */
  if (opts && opts->before_go_fn &&
      opts->before_go_fn(ctx, opts->before_go_user) != 0) {
    fprintf(stderr, "host_run: before_go callback failed; guest not released\n");
    reason = HOST_RUN_STOP_ERROR;
    goto done;
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

      if (!host_bp_find(bps, regs_linear(&dr))) {
        fprintf(stderr,
                "host_run: unexpected debugger stop at %04x:%04x (not a tracked Hydra stop)\n",
                dr.cs, dr.ip);
        reason = HOST_RUN_STOP_ERROR;
        break;
      }

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
  if (host_bp_clear_all(ctx, bps) != 0) {
    fprintf(stderr, "host_run: failed to clear all breakpoints during cleanup\n");
    if (reason != HOST_RUN_STOP_DOSEMU_EXIT)
      reason = HOST_RUN_STOP_ERROR;
  }
  if (stats)
    *stats = st;
  return reason;
}
