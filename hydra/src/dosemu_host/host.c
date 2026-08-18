/*
 * host.c - dosemu2 host for Hydra's hydra_machine_hardware_t vtable.
 *
 * This is Phase 3: the concrete emulator host behind Hydra's
 * emulator-independent core. hydra_user_init() is discovered by Hydra's
 * api_impl.c (dlsym(RTLD_DEFAULT, "hydra_user_init")) and is responsible for
 * filling the vtable with the callbacks below.
 *
 * Memory          -> lowmem memfd mapping (raw shared guest memory).
 * Registers/exec  -> dosdebug FIFO protocol (register get/set, breakpoints).
 * I/O             -> stubs; Phase 4 will drive real guest IN/OUT opcodes.
 *
 * Register direction note: the current Hydra core has NO call sites for
 * update_registers() (checked machine.c, exec.c, callstack.c, api_impl.c).
 * The vtable slot exists (hydra_machine.h:24) but nothing invokes it yet, so
 * its semantics are defined here. We implement update_registers() as a PULL:
 * it refreshes the hydra_machine_registers_t struct from the real CPU. The
 * complementary PUSH path (host -> CPU) is exposed as host_set_regs() and is
 * what state_restore() uses.
 */

#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "host.h"
#include "hydra_machine.h"
#include "typedefs.h"
#include "addr.h"
#include "conf.h"
#include "functions.h"
#include "callstack.h"
#include "header.h"

/* Set by hydra_machine_init() (api_impl.c) before hydra_user_init() runs. */
extern char *HYDRA_CMDLINE_CONF;

/* ------------------------------------------------------------------ */
/* conf string parsing                                                 */
/*                                                                    */
/* Format: "dosemu|pid=<dec>|code_load=<hex>|data_seg=<hex>"           */
/* Any field may be omitted; if pid is absent, $DOSEMU_PID is tried,   */
/* then auto-discovery (pid 0).                                        */
/* ------------------------------------------------------------------ */

static long host_conf_pid(const char *conf)
{
    /* e.g. "dosemu|pid=1234" -> 1234 */
    const char *p = conf ? strstr(conf, "pid=") : NULL;
    if (p) {
        p += strlen("pid=");
        char *end = NULL;
        long v = strtol(p, &end, 10);
        if (end && end != p && v >= 0)
            return v;
    }

    const char *env = getenv("DOSEMU_PID");
    if (env && *env) {
        char *end = NULL;
        long v = strtol(env, &end, 10);
        if (end && end != env && v >= 0)
            return v;
    }

    return 0; /* auto-discover */
}

static uint16_t host_conf_u16(const char *conf, const char *key, uint16_t dflt)
{
    const char *p = conf ? strstr(conf, key) : NULL;
    if (p) {
        p += strlen(key);
        char *end = NULL;
        long v = strtol(p, &end, 16);
        if (end && end != p && v >= 0 && v <= 0xffff)
            return (uint16_t)v;
    }
    return dflt;
}

/* ------------------------------------------------------------------ */
/* vtable: memory (delegate to lowmem)                                 */
/* ------------------------------------------------------------------ */

static uint8_t *host_mem_hostaddr(hydra_machine_ctx_t *_ctx, uint32_t addr)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    return lowmem_hostaddr(ctx->lm, addr);
}

static uint8_t host_mem_read8(hydra_machine_ctx_t *_ctx, uint32_t addr)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    return lowmem_read8(ctx->lm, addr);
}

static uint16_t host_mem_read16(hydra_machine_ctx_t *_ctx, uint32_t addr)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    return lowmem_read16(ctx->lm, addr);
}

static void host_mem_write8(hydra_machine_ctx_t *_ctx, uint32_t addr, uint8_t val)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    lowmem_write8(ctx->lm, addr, val);
}

static void host_mem_write16(hydra_machine_ctx_t *_ctx, uint32_t addr, uint16_t val)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    lowmem_write16(ctx->lm, addr, val);
}

/* ------------------------------------------------------------------ */
/* vtable: I/O (stubs for now; Phase 4 routes through guest opcodes)   */
/* ------------------------------------------------------------------ */

static uint8_t host_io_in8(hydra_machine_ctx_t *ctx, uint16_t port)
{
    (void)ctx; (void)port;
    return 0xff; /* stub */
}

static uint16_t host_io_in16(hydra_machine_ctx_t *ctx, uint16_t port)
{
    (void)ctx; (void)port;
    return 0xffff; /* stub */
}

static void host_io_out8(hydra_machine_ctx_t *ctx, uint16_t port, uint8_t val)
{
    (void)ctx; (void)port; (void)val; /* stub */
}

static void host_io_out16(hydra_machine_ctx_t *ctx, uint16_t port, uint16_t val)
{
    (void)ctx; (void)port; (void)val; /* stub */
}

/* ------------------------------------------------------------------ */
/* vtable: register synchronization                                    */
/*                                                                    */
/* update_registers() is a PULL: read the CPU state into regs.         */
/* ------------------------------------------------------------------ */

static void host_update_registers(hydra_machine_ctx_t *_ctx,
                                  hydra_machine_registers_t *regs)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0)
        return; /* leave the struct untouched on failure */

    regs->ax = dr.ax;   regs->bx = dr.bx;
    regs->cx = dr.cx;   regs->dx = dr.dx;
    regs->si = dr.si;   regs->di = dr.di;
    regs->bp = dr.bp;   regs->sp = dr.sp;
    regs->ip = dr.ip;   regs->cs = dr.cs;
    regs->ds = dr.ds;   regs->es = dr.es;
    regs->ss = dr.ss;   regs->flags = dr.flags;
}

/* ------------------------------------------------------------------ */
/* vtable: state save / restore (register snapshots keyed by label)    */
/* ------------------------------------------------------------------ */

static host_snapshot_t *host_snapshot_find(host_ctx_t *ctx, const char *label,
                                           int alloc)
{
    host_snapshot_t *free_slot = NULL;

    for (size_t i = 0; i < HOST_MAX_SNAPSHOTS; i++) {
        host_snapshot_t *s = &ctx->snapshots[i];
        if (s->used && strcmp(s->label, label) == 0)
            return s;
        if (!s->used && !free_slot)
            free_slot = s;
    }

    if (!alloc)
        return NULL;

    /* No existing slot: reuse the first (oldest) if none are free. */
    if (!free_slot)
        free_slot = &ctx->snapshots[0];

    strncpy(free_slot->label, label, HOST_SNAPSHOT_LABEL_MAX - 1);
    free_slot->label[HOST_SNAPSHOT_LABEL_MAX - 1] = '\0';
    free_slot->used = 1;
    return free_slot;
}

static void host_state_save(hydra_machine_ctx_t *_ctx, const char *label)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0)
        return;

    host_snapshot_t *s = host_snapshot_find(ctx, label, 1);
    if (s)
        s->regs = dr;
}

static void host_state_restore(hydra_machine_ctx_t *_ctx, const char *label)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    host_snapshot_t *s = host_snapshot_find(ctx, label, 0);
    if (!s)
        return;

    dosdebug_write_regs(ctx->db, &s->regs);
}

/* ------------------------------------------------------------------ */
/* host driver helpers (host.h)                                        */
/* ------------------------------------------------------------------ */

int host_get_regs(host_ctx_t *ctx, dosdebug_regs_t *regs)
{
    return dosdebug_read_regs(ctx->db, regs);
}

int host_set_reg(host_ctx_t *ctx, const char *name, uint16_t val)
{
    return dosdebug_write_reg(ctx->db, name, val);
}

int host_set_regs(host_ctx_t *ctx, const dosdebug_regs_t *regs)
{
    return dosdebug_write_regs(ctx->db, regs);
}

int host_set_bp(host_ctx_t *ctx, uint16_t seg, uint16_t off)
{
    return dosdebug_set_bp(ctx->db, seg, off);
}

int host_clear_bp(host_ctx_t *ctx, int bp_index)
{
    return dosdebug_clear_bp(ctx->db, bp_index);
}

int host_go_and_wait(host_ctx_t *ctx, dosdebug_regs_t *regs, int timeout_ms)
{
    if (dosdebug_go(ctx->db) != 0)
        return -1;
    return dosdebug_wait_stop(ctx->db, regs, timeout_ms);
}

int host_stop(host_ctx_t *ctx)
{
    return dosdebug_stop(ctx->db);
}

int host_clear_breakpoints(host_ctx_t *ctx)
{
    /* The dosdebug protocol has no "clear all"; nothing to do here.
     * Drivers track and clear individual breakpoints via host_clear_bp(). */
    (void)ctx;
    return 0;
}

pid_t host_pid(host_ctx_t *ctx)
{
    return ctx->pid;
}

void host_disconnect(host_ctx_t *ctx)
{
    if (!ctx)
        return;
    if (ctx->db)
        dosdebug_disconnect(ctx->db);
    if (ctx->lm)
        lowmem_disconnect(ctx->lm);
    ctx->db = NULL;
    ctx->lm = NULL;
    free(ctx);
}

/* ------------------------------------------------------------------ */
/* Hydra user metadata (required by functions.c / callstack.c)         */
/* ------------------------------------------------------------------ */

const hydra_function_metadata_t *hydra_user_functions(void)
{
    static const hydra_function_metadata_t md = { 0, NULL };
    return &md;
}

const hydra_callstack_metadata_t *hydra_user_callstack(void)
{
    static const hydra_callstack_metadata_t md = { 0, NULL };
    return &md;
}

/* ------------------------------------------------------------------ */
/* hydra_user_init: the entry point Hydra discovers via dlsym.         */
/*                                                                    */
/* Note: this codebase's signature is                                 */
/*   void hydra_user_init(hydra_conf_t *conf,                         */
/*                        hydra_machine_hardware_t *hw,               */
/*                        hydra_machine_audio_t *audio)               */
/* (api_impl.c:18) - not the (hw, audio, conf) order.                 */
/* ------------------------------------------------------------------ */

void hydra_user_init(hydra_conf_t *conf,
                     hydra_machine_hardware_t *hw,
                     hydra_machine_audio_t *audio)
{
    (void)audio;

    /* The config string was passed to hydra_machine_init(); it is stashed in
     * HYDRA_CMDLINE_CONF before this function is invoked (api_impl.c:11,25). */
    const char *confstr = HYDRA_CMDLINE_CONF;
    if (!confstr)
        confstr = "";

    host_ctx_t *ctx = calloc(1, sizeof(*ctx));
    if (!ctx)
        FAIL("dosemu host: out of memory");

    ctx->code_load_offset = host_conf_u16(confstr, "code_load=", 0);
    ctx->data_section_seg = host_conf_u16(confstr, "data_seg=", 0);
    conf->raw_code_offset = (uint32_t)host_conf_u16(confstr, "raw_code=", 0x1c00);

    long pid = host_conf_pid(confstr);

    ctx->db = dosdebug_connect((pid_t)pid);
    if (!ctx->db)
        FAIL("dosemu host: failed to connect to dosemu2 (pid=%ld). Is a "
             "dosemu2 instance running with the debugger enabled?", pid);

    ctx->pid = dosdebug_get_pid(ctx->db);

    ctx->lm = lowmem_connect(ctx->pid);
    if (!ctx->lm)
        FAIL("dosemu host: failed to map lowmem for pid %ld "
             "(is $_mapping = \"mapmshm\" set?)", (long)ctx->pid);

    /* Read the initial register state; this also verifies the connection. */
    ctx->have_initial_regs =
        (dosdebug_read_regs(ctx->db, &ctx->initial_regs) == 0);
    if (!ctx->have_initial_regs)
        FAIL("dosemu host: connection to pid %ld is not alive "
             "(register read failed)", (long)ctx->pid);

    /* Mandatory config (api_impl.c:27-28 asserts these were set). */
    conf->code_load_offset = ctx->code_load_offset;
    conf->data_section_seg = ctx->data_section_seg;

    /* Fill the vtable. */
    hw->ctx = (hydra_machine_ctx_t *)ctx;
    hw->mem_hostaddr = host_mem_hostaddr;
    hw->mem_read8    = host_mem_read8;
    hw->mem_read16   = host_mem_read16;
    hw->mem_write8   = host_mem_write8;
    hw->mem_write16  = host_mem_write16;
    hw->io_in8       = host_io_in8;
    hw->io_in16      = host_io_in16;
    hw->io_out8      = host_io_out8;
    hw->io_out16     = host_io_out16;
    hw->state_save   = host_state_save;
    hw->state_restore = host_state_restore;
    hw->update_registers = host_update_registers;
}