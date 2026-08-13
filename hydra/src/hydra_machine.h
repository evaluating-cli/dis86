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

HYDRA_MACHINE_INIT_FUNC(hydra_machine_init);
HYDRA_MACHINE_EXEC_FUNC(hydra_machine_exec);
HYDRA_MACHINE_NOTIFY_FUNC(hydra_machine_notify);
HYDRA_MACHINE_STEP_HOOK_FUNC(hydra_machine_step_hook);

typedef HYDRA_MACHINE_INIT_FUNC((*hydra_machine_init_fn_t));
typedef HYDRA_MACHINE_EXEC_FUNC((*hydra_machine_exec_fn_t));
typedef HYDRA_MACHINE_NOTIFY_FUNC((*hydra_machine_notify_fn_t));
typedef HYDRA_MACHINE_STEP_HOOK_FUNC((*hydra_machine_step_hook_fn_t));
