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
 *                          from hydra_machine_exit_code(). The host must not
 *                          run the guest again. */
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
 * HYDRA_EXEC_TERMINATED. */
int hydra_machine_exit_code(void);

typedef HYDRA_MACHINE_INIT_FUNC((*hydra_machine_init_fn_t));
typedef HYDRA_MACHINE_EXEC_FUNC((*hydra_machine_exec_fn_t));
typedef HYDRA_MACHINE_NOTIFY_FUNC((*hydra_machine_notify_fn_t));
typedef HYDRA_MACHINE_STEP_HOOK_FUNC((*hydra_machine_step_hook_fn_t));
