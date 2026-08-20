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
 */

#include <stddef.h>
#include <stdint.h>

#include "hydra_machine.h"
#include "host.h"

/* Breakpoint table size (hook bps + far/near return stubs). */
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
  uint64_t stub_hits;         /* far/near return stubs hit & cleared */
  uint64_t redirects;         /* hydra_exec_run() returned redirect (1) */
  uint64_t hook_breakpoints;  /* hook bps installed by host_run */
} host_run_stats_t;

/* Called after each dispatch/continue; return nonzero to stop the loop. */
typedef int (*host_run_stop_fn_t)(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                                  const host_run_stats_t *stats, void *user);

typedef struct host_run_options {
  size_t max_steps;           /* 0 = unlimited */
  int timeout_ms;             /* per go_and_wait; 0 = default (3000) */
  int verbose;                /* print each stop to stdout */
  host_run_stop_fn_t stop_fn; /* called after each dispatch; nonzero stops */
  void *stop_user;
} host_run_options_t;

/* Number of currently registered hydra hooks (breakpoints to install). */
int host_hook_breakpoint_count(host_ctx_t *ctx);

/* Install dosemu2 breakpoints at every registered hydra hook address.
 * Returns the number of breakpoints set (>= 0), or -1 on error. */
int host_run_install_hook_breakpoints(host_ctx_t *ctx);

/* Run the guest until one of the stop conditions is met. */
host_run_stop_reason_t host_run(host_ctx_t *ctx, hydra_machine_t *m,
                                const host_run_options_t *opts,
                                host_run_stats_t *stats);

#endif /* HYDRA_DOSEMU_HOST_DRIVER_H */