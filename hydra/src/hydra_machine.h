#pragma once
#include <stdint.h>
#include <stdlib.h>

typedef struct hydra_machine hydra_machine_t;
typedef struct hydra_machine_ctx hydra_machine_ctx_t;
typedef struct hydra_machine_hardware hydra_machine_hardware_t;
typedef struct hydra_machine_registers hydra_machine_registers_t;
typedef struct hydra_machine_audio hydra_machine_audio_t;

struct hydra_machine_hardware {
  hydra_machine_ctx_t *ctx;
  uint8_t *(*mem_hostaddr)(hydra_machine_ctx_t *, uint32_t);
  uint8_t (*mem_read8)(hydra_machine_ctx_t *, uint32_t);
  uint16_t (*mem_read16)(hydra_machine_ctx_t *, uint32_t);
  void (*mem_write8)(hydra_machine_ctx_t *, uint32_t, uint8_t);
  void (*mem_write16)(hydra_machine_ctx_t *, uint32_t, uint16_t);
  uint8_t (*io_in8)(hydra_machine_ctx_t *, uint16_t);
  uint16_t (*io_in16)(hydra_machine_ctx_t *, uint16_t);
  void (*io_out8)(hydra_machine_ctx_t *, uint16_t, uint8_t);
  void (*io_out16)(hydra_machine_ctx_t *, uint16_t, uint16_t);
  void (*state_save)(hydra_machine_ctx_t *, const char *);
  void (*state_restore)(hydra_machine_ctx_t *, const char *);
  void (*update_registers)(hydra_machine_ctx_t *, hydra_machine_registers_t *);
};

struct hydra_machine_registers {
  uint16_t ax, bx, cx, dx;
  uint16_t si, di, bp, sp, ip;
  uint16_t cs, ds, es, ss;
  uint16_t flags;
};

struct hydra_machine {
  hydra_machine_hardware_t hardware[1];
  hydra_machine_registers_t registers[1];
};

struct hydra_machine_audio {
  void (*cb)(void *, uint8_t *, int);
  void *ctx;
};

#define HYDRA_MACHINE_INIT_FUNC(name) void name(hydra_machine_hardware_t *hw, hydra_machine_audio_t *audio, const char *conf)
#define HYDRA_MACHINE_EXEC_FUNC(name) int name(hydra_machine_t *m, size_t interrupt_count)
#define HYDRA_MACHINE_NOTIFY_FUNC(name) void name(hydra_machine_t *m)
#define HYDRA_MACHINE_STEP_HOOK_FUNC(name) void name(hydra_machine_t *m)

/* Return values of hydra_machine_exec(). A host driver drives the machine with
 * these; the values are part of the host-facing contract, not an internal
 * detail, so they are named here rather than left as bare numbers.
 *
 *   HYDRA_EXEC_RESUME      nothing was redirected: the hook dispatch is over
 *                          and the guest continues at the current CS:IP. A host
 *                          with a guest CPU (dosemu2) hands control back to
 *                          that CPU; a host without one (the native host) has
 *                          nothing left to run and stops.
 *   HYDRA_EXEC_REDIRECT    the hook redirected the guest: CS:IP was updated
 *                          and the guest continues there.
 *   HYDRA_EXEC_RESTORE     the state-restore special mode stopped the dispatch
 *                          (see hydra_exec_run_special_modes in exec.c).
 *   HYDRA_EXEC_TERMINATED  the guest finished: it asked to terminate (INT 21h
 *                          AH=4Ch), so the hook returned without running the
 *                          guest's post-INT code. The exit code is available
 *                          from hydra_machine_exit_code().
 *
 *                          The machine is spent: the host must not run the
 *                          guest again, and cannot reuse the machine for
 *                          another one. A guest that terminated inside a
 *                          function reached through a callstub leaves the
 *                          caller's execution context waiting for a return
 *                          that never comes (the callstub waits on a
 *                          condition only the resume path signals), so the
 *                          context stays live and eligible for a later
 *                          resume. Running more than one guest in a process
 *                          is issue #95 Tier 2's job, and it needs an
 *                          explicit teardown for the contexts a terminating
 *                          guest leaves behind. */
#define HYDRA_EXEC_RESUME      0
#define HYDRA_EXEC_REDIRECT    1
#define HYDRA_EXEC_RESTORE     2
#define HYDRA_EXEC_TERMINATED  3

HYDRA_MACHINE_INIT_FUNC(hydra_machine_init);
HYDRA_MACHINE_EXEC_FUNC(hydra_machine_exec);
HYDRA_MACHINE_NOTIFY_FUNC(hydra_machine_notify);
HYDRA_MACHINE_STEP_HOOK_FUNC(hydra_machine_step_hook);

/* Return values of the optional host interrupt hook (`hydra_user_int`, the
 * dlsym seam the core resolves; see hydra/src/internal.h for the pointer and
 * hydra/src/machine.c for the call site). Only consulted by hosts that have
 * no guest CPU able to execute a raw `int` instruction.
 *
 *   0                        the interrupt was serviced; the guest continues
 *   HYDRA_USER_INT_TERMINATE the guest asked to terminate (exit code in AL).
 *                            The core stops the hook at the interrupt and
 *                            reports the termination to the host driver
 *                            (HYDRA_EXEC_TERMINATED), because the guest's
 *                            post-INT code must not run: INT 21h AH=4Ch does
 *                            not return.
 *   < 0                      not handled by this host */
#define HYDRA_USER_INT_TERMINATE 1

/* The exit code of the guest that terminated, or -1 if no guest has
 * terminated yet. Only meaningful after hydra_machine_exec() returned
 * HYDRA_EXEC_TERMINATED, and only until the next hydra_machine_exec()
 * call: the exec layer clears it on every run, so a driver that reads it
 * after a non-terminating run sees -1 rather than an earlier guest's code. */
int hydra_machine_exit_code(void);

/* What the machine did since the process started. The counters advance
 * whether or not a trace sink is open, so a driver can report them without
 * recording a trace. They are process-wide, not per run: a driver that
 * needs a per-run figure takes the difference, the way the dosemu2 host
 * driver does with the hook dispatch count. */
typedef struct hydra_run_stats {
  uint64_t dispatches;      /* hooks dispatched */
  uint64_t calls;           /* guest call boundaries seen by the callstack model */
  uint64_t returns;         /* guest return boundaries seen by the callstack model */
  uint64_t native_calls;    /* native->guest calls issued at a call seam */
  uint64_t native_returns;  /* native->guest calls that returned a value */
  uint64_t steps;           /* guest instructions observed one at a time */
  uint64_t interrupts;      /* interrupts serviced */
} hydra_run_stats_t;

/* Fill *_out with the run statistics. Returns 0 on success. */
int hydra_machine_stats(hydra_run_stats_t *_out);

/* Number of hook functions dispatched since startup (run_begin, not
 * resumed). Public so a host outside the Hydra core can read the progress
 * signal the dosemu2 host driver already consumes. */
size_t hydra_exec_hook_dispatch_count(void);

typedef HYDRA_MACHINE_INIT_FUNC((*hydra_machine_init_fn_t));
typedef HYDRA_MACHINE_EXEC_FUNC((*hydra_machine_exec_fn_t));
typedef HYDRA_MACHINE_NOTIFY_FUNC((*hydra_machine_notify_fn_t));
typedef HYDRA_MACHINE_STEP_HOOK_FUNC((*hydra_machine_step_hook_fn_t));
