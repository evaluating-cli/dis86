#ifndef HYDRA_DOSEMU_HOST_H
#define HYDRA_DOSEMU_HOST_H

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
    pid_t           pid;
    dosdebug_t     *db;
    lowmem_t       *lm;

    dosdebug_regs_t initial_regs;
    int             have_initial_regs;

    /* state_save / state_restore snapshots, keyed by label */
    host_snapshot_t snapshots[HOST_MAX_SNAPSHOTS];

    /* PSP segment of the last MZ guest loaded via host_run (0 = none). */
    uint16_t mz_psp;

    uint16_t code_load_offset;
    uint16_t data_section_seg;

    /* Raw-code memory is never guessed. The guest/launcher must explicitly
     * reserve a target-owned region and hand it to the host before host_run. */
    uint32_t raw_code_addr;
    size_t   raw_code_size;
    int      raw_code_reserved;

    /* dlopen handle of the user metadata library (conf key "lib=...").
     * NULL when no lib= was given. Ownership note in host.c: the handle is
     * intentionally kept mapped for the process lifetime because the core
     * holds metadata pointers that live inside the loaded object. */
    void *user_lib;

    /* OPTION E opt-in (conf key "overlays=armed"). When zero, overlay-typed
     * hooks fail host_run() before guest execution exactly as #34 shipped;
     * when set, overlay stubs arm lazily once paged in — see
     * docs/dosemu2/OPTION_E_DESIGN.md. */
    int overlays_armed;
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
int host_set_bp(host_ctx_t *ctx, uint16_t seg, uint16_t off);
int host_clear_bp(host_ctx_t *ctx, int bp_index);
int host_go_and_wait(host_ctx_t *ctx, dosdebug_regs_t *regs, int timeout_ms);
int host_stop(host_ctx_t *ctx);
int host_clear_breakpoints(host_ctx_t *ctx);

/* Point code_load_offset (CODE_START_SEG) at a newly discovered load
 * segment at runtime — e.g. PSP+0x10 after an MZ bpload. Updates the host
 * ctx, the Hydra core conf (HYDRA_CONF->code_load_offset, i.e. everything
 * that resolves image-relative hook addresses) and the datasection
 * baseptr derived from it. */
void host_set_code_load(host_ctx_t *ctx, uint16_t seg);

/* Connected pid, or 0. */
pid_t host_pid(host_ctx_t *ctx);
void host_disconnect(host_ctx_t *ctx);

#endif /* HYDRA_DOSEMU_HOST_H */
