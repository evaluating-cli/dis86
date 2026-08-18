#ifndef HYDRA_DOSEMU_HOST_H
#define HYDRA_DOSEMU_HOST_H

/*
 * host.h - dosemu2 host for the Hydra hydra_machine_hardware_t vtable.
 *
 * host.c implements hydra_user_init() (discovered by Hydra's api_impl.c via
 * dlsym) and the vtable callbacks. This header exposes the concrete host
 * context so the integration test (and future Hydra drivers) can reach the
 * dosdebug/lowmem handles directly when the vtable has no fitting callback.
 */

#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>

#include "dosdebug.h"
#include "lowmem.h"

#define HOST_MAX_SNAPSHOTS 8
#define HOST_SNAPSHOT_LABEL_MAX 64

typedef struct host_snapshot {
    char label[HOST_SNAPSHOT_LABEL_MAX];
    int  used;
    dosdebug_regs_t regs;
} host_snapshot_t;

typedef struct host_ctx {
    pid_t           pid;        /* dosemu2 pid (0 until connected) */
    dosdebug_t     *db;         /* dosdebug FIFO client */
    lowmem_t       *lm;         /* lowmem memfd mapping */

    dosdebug_regs_t initial_regs;   /* register state read at connect time */
    int             have_initial_regs;

    /* state_save / state_restore snapshots, keyed by label */
    host_snapshot_t snapshots[HOST_MAX_SNAPSHOTS];

    uint16_t code_load_offset;  /* conf->code_load_offset (CODE_START_SEG) */
    uint16_t data_section_seg;  /* conf->data_section_seg */
} host_ctx_t;

/* ------------------------------------------------------------------ */
/* Host driver helpers (beyond the vtable, used by test_host + driver) */
/* ------------------------------------------------------------------ */

/* Read the full register state from the CPU. */
int host_get_regs(host_ctx_t *ctx, dosdebug_regs_t *regs);

/* Write one register by name ("AX","IP","FL",...). */
int host_set_reg(host_ctx_t *ctx, const char *name, uint16_t val);

/* Write the full register state into the CPU. */
int host_set_regs(host_ctx_t *ctx, const dosdebug_regs_t *regs);

/* Set a breakpoint at seg:off; returns index >= 0 or -1. */
int host_set_bp(host_ctx_t *ctx, uint16_t seg, uint16_t off);

/* Clear a breakpoint by index. */
int host_clear_bp(host_ctx_t *ctx, int bp_index);

/* Continue execution, then wait for a stop (breakpoint/exception). */
int host_go_and_wait(host_ctx_t *ctx, dosdebug_regs_t *regs, int timeout_ms);

/* Request a stop of the emulated machine. */
int host_stop(host_ctx_t *ctx);

/* Clear any breakpoints we may have planted. */
int host_clear_breakpoints(host_ctx_t *ctx);

/* Connected pid, or 0. */
pid_t host_pid(host_ctx_t *ctx);

/* Tear down the dosdebug + lowmem connections and free the context. */
void host_disconnect(host_ctx_t *ctx);

#endif /* HYDRA_DOSEMU_HOST_H */