#ifndef HYDRA_DOSEMU_HOST_DRIVER_H
#define HYDRA_DOSEMU_HOST_DRIVER_H

/*
 * host_driver.h - Phase 4 execution loop for the Hydra-on-dosemu2 host.
 *
 * The driver owns the guest CPU between breakpoints:
 *   - plants dosemu2 int3 breakpoints at every registered Hydra hook,
 *   - on a breakpoint stop pulls the registers, dispatches the hook through
 *     hydra_machine_exec(), pushes the updated register state back, and
 *     continues,
 *   - when hydra_machine_exec() returns a CALL / CALL_NEAR result (a snippet
 *     of guest opcode from hydra_impl_raw_code()), it single-steps (dosdebug
 *     't') through the raw-code slot on dosemu2 until the RETF/RET lands at
 *     the return address, then feeds the post-raw-code state back to the
 *     Hydra exec engine to resume the hook.
 *   - on exit, clears all breakpoints it planted.
 *
 * Overlay hooks (HYDRA_HOOK_FLAGS_OVERLAY, Phase 7 Item E) sit on
 * VROOMM-style page-in stubs. They are armed lazily: the first call runs
 * the guest's pager natively (no breakpoint is ever planted on an unpaged
 * stub), and once the stub has been patched to a far jump the next stop
 * registers the overlay segment with the core (hydra_overlay_segment_set)
 * and plants the stub's breakpoint — from then on the core's redirect path
 * dispatches the decompiled hook into the paged body (RETURN_FAR completes
 * the far-call frame).
 */

#include <stddef.h>
#include <stdint.h>

#include "hydra_machine.h"
#include "host.h"

/* Hook breakpoint table size. dosemu2's dosdebug caps breakpoints at 64. */
#define HOST_RUN_MAX_BPS 64

/* How a host_run() loop ended. */
typedef enum {
  HOST_RUN_STOP_NONE = 0,
  HOST_RUN_STOP_CALLBACK,     /* stop_fn returned nonzero */
  HOST_RUN_STOP_MAXSTEPS,     /* max_steps reached */
  HOST_RUN_STOP_DOSEMU_EXIT,  /* dosemu2 died while running */
  HOST_RUN_STOP_TIMEOUT,      /* go_and_wait timed out */
  HOST_RUN_STOP_ERROR,        /* dosdebug protocol / host error */
} host_run_stop_reason_t;

/* Per-run counters; zeroed at the start of host_run(). */
typedef struct host_run_stats {
  uint64_t stops;             /* debugger stops observed */
  uint64_t hook_dispatches;   /* hydra_exec_hook_dispatch_count delta */
  uint64_t raw_code_runs;     /* CALL/CALL_NEAR results (guest opcodes) */
  uint64_t raw_code_returns;  /* trace-detected returns (CALL/CALL_NEAR completed) */
  uint64_t redirects;         /* hydra_exec_run() returned redirect (1) */
  uint64_t hook_breakpoints;  /* hook bps installed by host_run */
  uint64_t mz_psp;            /* MZ load: PSP segment of the guest (0 = n/a) */
} host_run_stats_t;

/* Called once after all run-owned breakpoints are armed and before the first
 * guest GO. Return nonzero to abort the run while the CPU is still stopped. */
typedef int (*host_run_before_go_fn_t)(host_ctx_t *ctx, void *user);

/* Called after each dispatch/continue; return nonzero to stop the loop. */
typedef int (*host_run_stop_fn_t)(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                                  const host_run_stats_t *stats, void *user);

typedef struct host_run_options {
  size_t max_steps;           /* 0 = unlimited */
  size_t max_trace_steps;     /* per CALL/CALL_NEAR trace; 0 = default (10000) */
  int timeout_ms;             /* per go_and_wait; 0 = default (3000), <0 = wait
                                  forever (rely on stop_fn / dosemu exit) */
  int verbose;                /* print each stop to stdout */
  host_run_before_go_fn_t before_go_fn; /* one-shot, after bp install */
  void *before_go_user;
  host_run_stop_fn_t stop_fn; /* called after each dispatch; nonzero stops */
  void *stop_user;

  /* MZ (.exe) guest loading (Phase 7 Item C). When mz_load is nonzero,
   * host_run() releases the harness's parked launcher .COM, which loads
   * the guest via INT21 AH=4B01 and hands off at its relocated entry
   * (see launch.asm for why dosemu2's own bpload/DBGload stub cannot be
   * used against stock fdpp). host_run() waits for that entry stop and
   * validates it against the expectations below (parsed from the MZ
   * header): CS == PSP+0x10+e_cs, IP == e_ip, DS == ES, PSP:0 == CD 20,
   * MCB owner word == PSP. On success it points code_load_offset at
   * PSP+0x10 dynamically and only then plants the hook breakpoints and
   * enters the normal run loop. */
  int      mz_load;
  /* When set, the harness has already parked the machine at the guest's
   * entry (see launch.asm: it loads the child via INT21 AH=4B01, publishes
   * its PSP and idles; the driver-side test validates the image and pushes
   * the entry registers). mz_load_and_wait then only validates the current
   * register state instead of releasing and waiting for a stop. */
  int      mz_parked_at_entry;
  uint16_t mz_entry_cs;       /* expected entry segment (paras past load seg) */
  uint16_t mz_entry_ip;       /* expected entry offset */
  int      mz_timeout_ms;     /* load-phase budget; 0 = default (60 s) */
} host_run_options_t;

/* Number of currently registered hydra hooks (breakpoints to install). */
int host_hook_breakpoint_count(host_ctx_t *ctx);

/* Run the guest until one of the stop conditions is met. */
host_run_stop_reason_t host_run(host_ctx_t *ctx, hydra_machine_t *m,
                                const host_run_options_t *opts,
                                host_run_stats_t *stats);

#endif /* HYDRA_DOSEMU_HOST_DRIVER_H */
