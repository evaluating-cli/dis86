#ifndef HYDRA_DOSEMU_HOST_H
#define HYDRA_DOSEMU_HOST_H

#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>

#include "dosdebug.h"
#include "lowmem.h"

typedef struct host_ctx {
    pid_t           pid;
    dosdebug_t     *db;
    lowmem_t       *lm;

    dosdebug_regs_t initial_regs;
    int             have_initial_regs;

    uint16_t code_load_offset;
    uint16_t data_section_seg;

    /* Raw-code memory is never guessed. The guest/launcher must explicitly
     * reserve a target-owned region and hand it to the host before host_run. */
    uint32_t raw_code_addr;
    size_t   raw_code_size;
    int      raw_code_reserved;
} host_ctx_t;

int host_get_regs(host_ctx_t *ctx, dosdebug_regs_t *regs);
int host_set_reg(host_ctx_t *ctx, const char *name, uint16_t val);
int host_set_regs(host_ctx_t *ctx, const dosdebug_regs_t *regs);

/* Register a region that the guest/launcher has explicitly reserved for Hydra.
 * The region must be 16-byte aligned, lie entirely in the verified shared
 * low-memory backing, and be at least one 128-byte raw-code slot. */
int host_reserve_raw_code(host_ctx_t *ctx, uint32_t addr, size_t size);
bool host_raw_code_ready(const host_ctx_t *ctx);

/* Diff-write: push only registers that differ from *base (the live CPU
 * state); FL is always written. Same verification as host_set_regs. */
int host_set_regs_diff(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                       const dosdebug_regs_t *base);

/* Set a breakpoint at seg:off; returns index >= 0 or -1. */
>>>>>>> b0a5a94 (dosemu_host: diff-writes for register pushes (2.8x test speedup))
int host_set_bp(host_ctx_t *ctx, uint16_t seg, uint16_t off);
int host_clear_bp(host_ctx_t *ctx, int bp_index);
int host_go_and_wait(host_ctx_t *ctx, dosdebug_regs_t *regs, int timeout_ms);
int host_stop(host_ctx_t *ctx);
int host_clear_breakpoints(host_ctx_t *ctx);
pid_t host_pid(host_ctx_t *ctx);
void host_disconnect(host_ctx_t *ctx);

#endif /* HYDRA_DOSEMU_HOST_H */
