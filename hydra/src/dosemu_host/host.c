/*
 * host.c - dosemu2 host for Hydra's hydra_machine_hardware_t vtable.
 *
 * Memory          -> verified lowmem memfd mapping (raw shared guest memory).
 * Registers/exec  -> dosdebug FIFO protocol (register get/set, breakpoints).
 * I/O             -> guest IN/OUT opcodes via hydra_impl_raw_code.
 */

#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "host.h"
#include "hydra_machine.h"
#include "typedefs.h"
#include "addr.h"
#include "conf.h"
#include "functions.h"
#include "callstack.h"
#include "header.h"

extern char *HYDRA_CMDLINE_CONF;

#define HOST_SNAPSHOT_MAGIC "HYDSNP1"
#define HOST_SNAPSHOT_VERSION 1u
#define HOST_SNAPSHOT_MEM_SIZE 0x110000u
#define HOST_FLAGS_VERIFY_MASK ((uint16_t)~(0x3000u | 0x0002u))

typedef struct host_snapshot_file_header {
    char magic[8];
    uint32_t version;
    uint32_t memory_size;
    dosdebug_regs_t regs;
} host_snapshot_file_header_t;

static long host_conf_pid(const char *conf)
{
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
    return 0;
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

static uint8_t host_io_in8(hydra_machine_ctx_t *ctx, uint16_t port)
{
    (void)ctx; (void)port;
    return 0xff;
}

static uint16_t host_io_in16(hydra_machine_ctx_t *ctx, uint16_t port)
{
    (void)ctx; (void)port;
    return 0xffff;
}

static void host_io_out8(hydra_machine_ctx_t *ctx, uint16_t port, uint8_t val)
{
    (void)ctx; (void)port; (void)val;
}

static void host_io_out16(hydra_machine_ctx_t *ctx, uint16_t port, uint16_t val)
{
    (void)ctx; (void)port; (void)val;
}

static void host_update_registers(hydra_machine_ctx_t *_ctx,
                                  hydra_machine_registers_t *regs)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0)
        return;

    regs->ax = dr.ax;   regs->bx = dr.bx;
    regs->cx = dr.cx;   regs->dx = dr.dx;
    regs->si = dr.si;   regs->di = dr.di;
    regs->bp = dr.bp;   regs->sp = dr.sp;
    regs->ip = dr.ip;   regs->cs = dr.cs;
    regs->ds = dr.ds;   regs->es = dr.es;
    regs->ss = dr.ss;   regs->flags = dr.flags;
}

int host_set_regs(host_ctx_t *ctx, const dosdebug_regs_t *regs)
{
    if (!ctx || !regs)
        return -1;

    if (dosdebug_write_regs(ctx->db, regs) != 0)
        return -1;

    /* dosdebug_write_regs historically ORed IF/IOPL before invoking the FL
     * setter. Re-apply the exact requested guest FLAGS. dosemu's set_FLAGS()
     * stores guest IF through VIF/set_IF()/clear_IF(); get_FLAGS() exposes it
     * again even though the physical vm86 IF remains forced on. */
    (void)dosdebug_write_reg(ctx->db, "FL", regs->flags);

    dosdebug_regs_t now;
    if (dosdebug_read_regs(ctx->db, &now) != 0)
        return -1;
    if ((now.flags & HOST_FLAGS_VERIFY_MASK) !=
        (regs->flags & HOST_FLAGS_VERIFY_MASK)) {
        fprintf(stderr,
                "dosemu host: guest FLAGS restore failed: wanted %04x got %04x\n",
                (unsigned)regs->flags, (unsigned)now.flags);
        return -1;
    }
    return 0;
}

static int host_write_snapshot(host_ctx_t *ctx, const char *path,
                               const dosdebug_regs_t *regs)
{
    if (!path || !path[0] || lowmem_size(ctx->lm) < HOST_SNAPSHOT_MEM_SIZE)
        return -1;

    char tmp[PATH_MAX];
    int n = snprintf(tmp, sizeof(tmp), "%s.tmp.%ld", path, (long)getpid());
    if (n < 0 || (size_t)n >= sizeof(tmp))
        return -1;

    FILE *f = fopen(tmp, "wb");
    if (!f) {
        fprintf(stderr, "dosemu host: snapshot open %s failed: %s\n",
                tmp, strerror(errno));
        return -1;
    }

    host_snapshot_file_header_t h;
    memset(&h, 0, sizeof(h));
    memcpy(h.magic, HOST_SNAPSHOT_MAGIC, sizeof(HOST_SNAPSHOT_MAGIC));
    h.version = HOST_SNAPSHOT_VERSION;
    h.memory_size = HOST_SNAPSHOT_MEM_SIZE;
    h.regs = *regs;

    uint8_t *mem = lowmem_base(ctx->lm);
    int ok = fwrite(&h, 1, sizeof(h), f) == sizeof(h) &&
             fwrite(mem, 1, HOST_SNAPSHOT_MEM_SIZE, f) == HOST_SNAPSHOT_MEM_SIZE &&
             fflush(f) == 0 && fsync(fileno(f)) == 0;
    int saved_errno = errno;
    if (fclose(f) != 0)
        ok = 0;
    if (!ok) {
        unlink(tmp);
        errno = saved_errno;
        fprintf(stderr, "dosemu host: snapshot write %s failed: %s\n",
                tmp, strerror(errno));
        return -1;
    }
    if (rename(tmp, path) != 0) {
        saved_errno = errno;
        unlink(tmp);
        errno = saved_errno;
        fprintf(stderr, "dosemu host: snapshot rename %s -> %s failed: %s\n",
                tmp, path, strerror(errno));
        return -1;
    }
    return 0;
}

static int host_read_snapshot(host_ctx_t *ctx, const char *path,
                              dosdebug_regs_t *regs)
{
    if (!path || !path[0] || lowmem_size(ctx->lm) < HOST_SNAPSHOT_MEM_SIZE)
        return -1;

    FILE *f = fopen(path, "rb");
    if (!f) {
        fprintf(stderr, "dosemu host: snapshot open %s failed: %s\n",
                path, strerror(errno));
        return -1;
    }

    host_snapshot_file_header_t h;
    uint8_t *mem = malloc(HOST_SNAPSHOT_MEM_SIZE);
    if (!mem) {
        fclose(f);
        return -1;
    }

    int ok = fread(&h, 1, sizeof(h), f) == sizeof(h) &&
             memcmp(h.magic, HOST_SNAPSHOT_MAGIC, sizeof(HOST_SNAPSHOT_MAGIC)) == 0 &&
             h.version == HOST_SNAPSHOT_VERSION &&
             h.memory_size == HOST_SNAPSHOT_MEM_SIZE &&
             fread(mem, 1, HOST_SNAPSHOT_MEM_SIZE, f) == HOST_SNAPSHOT_MEM_SIZE;
    if (fclose(f) != 0)
        ok = 0;
    if (!ok) {
        fprintf(stderr, "dosemu host: invalid or truncated snapshot: %s\n", path);
        free(mem);
        return -1;
    }

    memcpy(lowmem_base(ctx->lm), mem, HOST_SNAPSHOT_MEM_SIZE);
    free(mem);
    *regs = h.regs;
    return 0;
}

static void host_state_save(hydra_machine_ctx_t *_ctx, const char *label)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    dosdebug_regs_t dr;
    if (dosdebug_read_regs(ctx->db, &dr) != 0 ||
        host_write_snapshot(ctx, label, &dr) != 0) {
        fprintf(stderr, "dosemu host: state_save failed for %s\n",
                label ? label : "(null)");
    }
}

static void host_state_restore(hydra_machine_ctx_t *_ctx, const char *label)
{
    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    dosdebug_regs_t dr;
    if (host_read_snapshot(ctx, label, &dr) != 0 ||
        host_set_regs(ctx, &dr) != 0) {
        fprintf(stderr, "dosemu host: state_restore failed for %s\n",
                label ? label : "(null)");
    }
}

int host_get_regs(host_ctx_t *ctx, dosdebug_regs_t *regs)
{
    return dosdebug_read_regs(ctx->db, regs);
}

int host_set_reg(host_ctx_t *ctx, const char *name, uint16_t val)
{
    return dosdebug_write_reg(ctx->db, name, val);
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

void hydra_user_init(hydra_conf_t *conf,
                     hydra_machine_hardware_t *hw,
                     hydra_machine_audio_t *audio)
{
    (void)audio;

    const char *confstr = HYDRA_CMDLINE_CONF ? HYDRA_CMDLINE_CONF : "";
    host_ctx_t *ctx = calloc(1, sizeof(*ctx));
    if (!ctx)
        FAIL("dosemu host: out of memory");

    ctx->code_load_offset = host_conf_u16(confstr, "code_load=", 0);
    ctx->data_section_seg = host_conf_u16(confstr, "data_seg=", 0);
    conf->raw_code_offset = (uint32_t)host_conf_u16(confstr, "raw_code=", 0x1c00);

    long pid = host_conf_pid(confstr);
    ctx->db = dosdebug_connect((pid_t)pid);
    if (!ctx->db)
        FAIL("dosemu host: failed to connect to dosemu2 (pid=%ld)", pid);
    ctx->pid = dosdebug_get_pid(ctx->db);

    /* The memfd name is not unique: mapmshm may create several dosemu_<pid>
     * objects. Read two stable low-memory regions independently through
     * dosdebug, then require exactly one candidate backing to match both. */
    uint8_t ivt_probe[16];
    uint8_t bda_probe[16];
    if (dosdebug_read_mem(ctx->db, 0, 0x0000, ivt_probe, sizeof(ivt_probe)) !=
            (int)sizeof(ivt_probe) ||
        dosdebug_read_mem(ctx->db, 0, 0x0400, bda_probe, sizeof(bda_probe)) !=
            (int)sizeof(bda_probe)) {
        FAIL("dosemu host: unable to read independent lowmem verification probes");
    }
    const lowmem_probe_t probes[] = {
        {0x0000u, ivt_probe, sizeof(ivt_probe)},
        {0x0400u, bda_probe, sizeof(bda_probe)},
    };
    ctx->lm = lowmem_connect_verified(ctx->pid, probes,
                                      sizeof(probes) / sizeof(probes[0]));
    if (!ctx->lm)
        FAIL("dosemu host: failed to identify a unique lowmem backing for pid %ld",
             (long)ctx->pid);

    ctx->have_initial_regs =
        (dosdebug_read_regs(ctx->db, &ctx->initial_regs) == 0);
    if (!ctx->have_initial_regs)
        FAIL("dosemu host: initial register read failed for pid %ld",
             (long)ctx->pid);

    conf->code_load_offset = ctx->code_load_offset;
    conf->data_section_seg = ctx->data_section_seg;

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
