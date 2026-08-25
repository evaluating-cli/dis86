/*
 * host.c - dosemu2 host for Hydra's hydra_machine_hardware_t vtable.
 *
 * Memory          -> verified lowmem memfd mapping (raw shared guest memory).
 * Registers/exec  -> dosdebug FIFO protocol (register get/set, breakpoints).
 * I/O             -> guest IN/OUT opcodes via hydra_impl_raw_code.
 */

#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "host.h"
#include "hydra_machine.h"
#include "internal.h"

extern char *HYDRA_CMDLINE_CONF;
extern hydra_conf_t HYDRA_CONF[1];

#define HOST_SNAPSHOT_MAGIC "HYDSNP1"
#define HOST_SNAPSHOT_VERSION 1u
#define HOST_SNAPSHOT_MEM_SIZE 0x110000u
#define HOST_FLAGS_VERIFY_MASK ((uint16_t)~(0x3000u | 0x0002u))
#define HOST_RAW_SLOT_SIZE 128u

typedef struct host_snapshot_file_header {
    char magic[8];
    uint32_t version;
    uint32_t memory_size;
    dosdebug_regs_t regs;
} host_snapshot_file_header_t;
/* ------------------------------------------------------------------ */
/* conf string parsing                                                 */
/*                                                                    */
/* Format: "dosemu|pid=<dec>|code_load=<hex>|data_seg=<hex>|lib=<path>" */
/* Any field may be omitted; if pid is absent, $DOSEMU_PID is tried,   */
/* then auto-discovery (pid 0).                                        */
/* ------------------------------------------------------------------ */

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

/* Copy the value of a string-valued conf key ("key=value", up to the next
 * '|' or end of string). Returns 1 if present, 0 if absent. */
static int host_conf_string(const char *conf, const char *key,
                            char *out, size_t out_len)
{
    const char *p = conf ? strstr(conf, key) : NULL;
    if (!p)
        return 0;
    p += strlen(key);
    const char *end = strchr(p, '|');
    size_t len = end ? (size_t)(end - p) : strlen(p);
    if (len == 0 || len >= out_len)
        return 0;
    memcpy(out, p, len);
    out[len] = '\0';
    return 1;
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

    /* dosdebug_write_regs() owns the vm86 IF/VIF translation and performs an
     * architectural read-back. Keep one outer read-back here as a host-vtable
     * invariant check, but do not issue a second FL write with different
     * normalization semantics. */
    if (dosdebug_write_regs(ctx->db, regs) != 0)
        return -1;

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

    /* Memory first: restored CS:IP may point into this image on resume. */
    memcpy(lowmem_base(ctx->lm), mem, HOST_SNAPSHOT_MEM_SIZE);
    free(mem);
    *regs = h.regs;
    return 0;
}

/* ------------------------------------------------------------------ */
/* HYDSNAP v1: full-memory + register state snapshots (Phase 7 Item D) */
/*                                                                    */
/* Layout (little-endian, byte-exact):                                */
/*   0x00  8   magic "HYDSNAP\0"                                      */
/*   0x08  4   version (u32, =1)                                      */
/*   0x0C 28   regs[14] u16: ax bx cx dx si di bp sp ip cs ds es ss   */
/*             flags                                                  */
/*   0x28  2   psp (informational; 0 = unknown)                       */
/*   0x2A  2   code_load_offset (CODE_START_SEG at capture time)      */
/*   0x2C  2   data_section_seg                                       */
/*   0x2E  2   reserved (0)                                           */
/*   0x30  4   reserved (0)                                           */
/*   0x34  4   guest_size (u32, must equal the guest-addressable window) */
/*   0x38  4   crc32 of header (this field zeroed) + lowmem blob      */
/*   0x3C  4   reserved (0)                                           */
/*   0x40 ...  lowmem blob (lowmem_size bytes)                        */
/*                                                                    */
/* NOT captured (documented limitation): simx86 JIT state (harmless   */
/* after a full reload), device/IRQ/PIT state, DOS handles/SFT/JFT    */
/* backing host fds/latches (the guest bytes are restored but the     */
/* underlying host-side file state isn't; acceptable for CI-style     */
/* restore-into-fresh-instance use), memory above the lowmem window   */
/* (HMA top), dosemu debugger breakpoints (replanted by host_run).    */
/* ------------------------------------------------------------------ */

#define HYDSNAP_MAGIC   "HYDSNAP\0"
#define HYDSNAP_VERSION 1u
#define HYDSNAP_HDR_LEN 0x40u

static uint32_t hydsnap_crc32_update(uint32_t crc, const uint8_t *data, size_t len)
{
    for (size_t i = 0; i < len; i++) {
        crc ^= data[i];
        for (int b = 0; b < 8; b++)
            crc = (crc >> 1) ^ (crc & 1 ? 0xedb88320u : 0);
    }
    return crc;
}

static uint32_t hydsnap_crc32_snapshot(const uint8_t hdr[HYDSNAP_HDR_LEN],
                                       const uint8_t *data, size_t len)
{
    uint8_t hdr_copy[HYDSNAP_HDR_LEN];
    memcpy(hdr_copy, hdr, sizeof(hdr_copy));
    memset(hdr_copy + 0x38, 0, 4);

    uint32_t crc = 0xffffffffu;
    crc = hydsnap_crc32_update(crc, hdr_copy, sizeof(hdr_copy));
    crc = hydsnap_crc32_update(crc, data, len);
    return ~crc;
}

static void hydsnap_put16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8);
}

static void hydsnap_put32(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)v;         p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16); p[3] = (uint8_t)(v >> 24);
}

static uint16_t hydsnap_get16(const uint8_t *p)
{
    return (uint16_t)(p[0] | (p[1] << 8));
}

static uint32_t hydsnap_get32(const uint8_t *p)
{
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
           ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void hydsnap_fill_regrange(uint8_t *hdr, const dosdebug_regs_t *dr)
{
    uint16_t vals[14] = { dr->ax, dr->bx, dr->cx, dr->dx,
                          dr->si, dr->di, dr->bp, dr->sp,
                          dr->ip, dr->cs, dr->ds, dr->es, dr->ss,
                          dr->flags };
    for (int i = 0; i < 14; i++)
        hydsnap_put16(hdr + 0x0C + 2 * i, vals[i]);
}

static void hydsnap_read_regrange(const uint8_t *hdr, dosdebug_regs_t *dr)
{
    dr->ax = hydsnap_get16(hdr + 0x0C);      dr->bx = hydsnap_get16(hdr + 0x0E);
    dr->cx = hydsnap_get16(hdr + 0x10);      dr->dx = hydsnap_get16(hdr + 0x12);
    dr->si = hydsnap_get16(hdr + 0x14);      dr->di = hydsnap_get16(hdr + 0x16);
    dr->bp = hydsnap_get16(hdr + 0x18);      dr->sp = hydsnap_get16(hdr + 0x1A);
    dr->ip = hydsnap_get16(hdr + 0x1C);      dr->cs = hydsnap_get16(hdr + 0x1E);
    dr->ds = hydsnap_get16(hdr + 0x20);      dr->es = hydsnap_get16(hdr + 0x22);
    dr->ss = hydsnap_get16(hdr + 0x24);      dr->flags = hydsnap_get16(hdr + 0x26);
}

/* Write a full snapshot (regs + the whole guest-addressable window) to
 * path. NOTE: lowmem_size() is the raw memfd mapping length, which dosemu
 * pads to hundreds of MB; only lowmem_guest_size() (lowmem + HMA) holds
 * guest-visible state and must be captured. */
static int host_snapshot_write_file(host_ctx_t *ctx, const char *path,
                                    const dosdebug_regs_t *dr)
{
    size_t blob = lowmem_guest_size();
    if (blob > lowmem_size(ctx->lm))
        return -1; /* mapping smaller than the guest window: cannot capture */
    uint8_t hdr[HYDSNAP_HDR_LEN] = {0};

    memcpy(hdr, HYDSNAP_MAGIC, 8);
    hydsnap_put32(hdr + 0x08, HYDSNAP_VERSION);
    hydsnap_fill_regrange(hdr, dr);
    hydsnap_put16(hdr + 0x28, ctx->mz_psp);
    hydsnap_put16(hdr + 0x2A, ctx->code_load_offset);
    hydsnap_put16(hdr + 0x2C, ctx->data_section_seg);
    hydsnap_put32(hdr + 0x34, (uint32_t)blob);

    /* Copy the guest window once and compute the CRC over the exact header
     * and private memory image that will be written. The live mapping keeps
     * mutating (BDA ticks et al.), so no second pass over it is allowed. */
    uint8_t *snap = malloc(blob);
    if (!snap)
        return -1;
    memcpy(snap, lowmem_base(ctx->lm), blob);
    hydsnap_put32(hdr + 0x38, hydsnap_crc32_snapshot(hdr, snap, blob));

    FILE *f = fopen(path, "wb");
    if (!f) {
        free(snap);
        return -1;
    }
    int ok = fwrite(hdr, 1, sizeof(hdr), f) == sizeof(hdr) &&
             fwrite(snap, 1, blob, f) == blob;
    if (fclose(f) != 0)
        ok = 0;
    free(snap);
    return ok ? 0 : -1;
}

/* Read + validate a snapshot file. Allocates the blob; caller frees. */
static int host_snapshot_read_file(const char *path, uint8_t hdr[HYDSNAP_HDR_LEN],
                                   uint8_t **blob_out, size_t *blob_size_out)
{
    FILE *f = fopen(path, "rb");
    if (!f)
        return -1;
    if (fread(hdr, 1, HYDSNAP_HDR_LEN, f) != HYDSNAP_HDR_LEN) {
        fclose(f);
        return -1;
    }
    if (memcmp(hdr, HYDSNAP_MAGIC, 8) != 0 ||
        hydsnap_get32(hdr + 0x08) != HYDSNAP_VERSION) {
        fclose(f);
        return -1;
    }
    uint32_t blob = hydsnap_get32(hdr + 0x34);
    if (blob == 0 || blob > 2u * 1024u * 1024u) {   /* sanity bound */
        fclose(f);
        return -1;
    }
    uint8_t *mem = malloc(blob);
    if (!mem) {
        fclose(f);
        return -1;
    }
    int ok = fread(mem, 1, blob, f) == blob;
    fclose(f);
    if (ok && hydsnap_crc32_snapshot(hdr, mem, blob) ==
                  hydsnap_get32(hdr + 0x38)) {
        *blob_out = mem;
        *blob_size_out = blob;
        return 0;
    }
    free(mem);
    return -1;
}

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
        FAIL("dosemu host: failed to read CPU state for %s",
             label ? label : "(null)");

    /* CAPTURE mode (HYDRA core): persist a full-memory HYDSNAP snapshot to
     * state_path (passed as label). NOTE: the xhost core then exit(0)s; on
     * this external host the caller drives the shutdown. */
    if (HYDRA_MODE->mode == HYDRA_MODE_CAPTURE) {
        if (host_snapshot_write_file(ctx, label, &dr) != 0)
            FAIL("dosemu host: failed to write HYDSNAP state '%s'", label);
        return;
    }

    if (host_write_snapshot(ctx, label, &dr) != 0) {
        /* The hardware vtable callback is void and capture exits immediately
         * after it returns. Logging and returning would therefore turn a
         * failed checkpoint into exit(0). Fail hard instead. */
        FAIL("dosemu host: state_save failed for %s",
             label ? label : "(null)");
    }

    host_snapshot_t *s = host_snapshot_find(ctx, label, 1);
    if (s)
        s->regs = dr;
}

static void host_state_restore(hydra_machine_ctx_t *_ctx, const char *label)
{
    dosdebug_regs_t dr;

    host_ctx_t *ctx = (host_ctx_t *)_ctx;
    /* RESTORE mode (HYDRA core): load a HYDSNAP file. Memory FIRST, then the
     * register state (full paced write with read-back verify; diff-writes
     * are unsafe here because the live CPU state is unknown). Also adopt the
     * snapshot's code/data offsets so image-relative hooks stay valid. */
    if (HYDRA_MODE->mode == HYDRA_MODE_RESTORE) {
        uint8_t hdr[HYDSNAP_HDR_LEN];
        uint8_t *blob = NULL;
        size_t blob_size = 0;
        if (host_snapshot_read_file(label, hdr, &blob, &blob_size) != 0)
            FAIL("dosemu host: failed to load HYDSNAP state '%s'", label);
        if (blob_size != lowmem_guest_size())
            FAIL("dosemu host: HYDSNAP blob size %zu != guest window %zu",
                 blob_size, lowmem_guest_size());

        memcpy(lowmem_base(ctx->lm), blob, blob_size);
        free(blob);

        hydsnap_read_regrange(hdr, &dr);
        if (dosdebug_write_regs(ctx->db, &dr) != 0)
            FAIL("dosemu host: failed to write restored registers");

        uint16_t snap_psp = hydsnap_get16(hdr + 0x28);
        ctx->mz_psp = snap_psp;
        if (snap_psp && dr.ds != snap_psp)
            fprintf(stderr, "dosemu host: WARNING: HYDSNAP psp=%04x but "
                    "restored DS=%04x\n", snap_psp, dr.ds);

        /* data_section_seg is part of the snapshot layout. Restore it before
         * code_load_offset so host_set_code_load() recomputes the datasection
         * base pointer using the captured value, and synchronize the core
         * configuration at the same time. */
        ctx->data_section_seg = hydsnap_get16(hdr + 0x2C);
        HYDRA_CONF->data_section_seg = ctx->data_section_seg;
        host_set_code_load(ctx, hydsnap_get16(hdr + 0x2A));
        return;
    }

    if (host_read_snapshot(ctx, label, &dr) != 0 ||
        host_set_regs(ctx, &dr) != 0) {
        /* A failed restore must not let the core switch back to NORMAL with
         * partially replaced memory/registers. The callback cannot return an
         * error, so terminate rather than report a false successful restore. */
        FAIL("dosemu host: state_restore failed for %s",
             label ? label : "(null)");
    }

    host_snapshot_t *s = host_snapshot_find(ctx, label, 0);
    if (!s)
        return;

    host_set_regs(ctx, &s->regs);
}

int host_get_regs(host_ctx_t *ctx, dosdebug_regs_t *regs)
{
    return dosdebug_read_regs(ctx->db, regs);
}

int host_set_reg(host_ctx_t *ctx, const char *name, uint16_t val)
{
    return dosdebug_write_reg(ctx->db, name, val);
}

int host_reserve_raw_code(host_ctx_t *ctx, uint32_t addr, size_t size)
{
    if (!ctx || !ctx->lm || size < HOST_RAW_SLOT_SIZE || size > UINT32_MAX)
        return -1;
    if ((addr & 0x0fu) != 0)
        return -1;
    if ((uint64_t)addr + size > lowmem_size(ctx->lm))
        return -1;
    if ((addr >> 4) < ctx->code_load_offset)
        return -1;

    ctx->raw_code_addr = addr;
    ctx->raw_code_size = size;
    ctx->raw_code_reserved = 1;
    HYDRA_CONF->raw_code_offset = addr;
    HYDRA_CONF->raw_code_size = (uint32_t)size;
    return 0;
}

bool host_raw_code_ready(const host_ctx_t *ctx)
{
    return ctx && ctx->raw_code_reserved &&
           ctx->raw_code_size >= HOST_RAW_SLOT_SIZE;
}

int host_set_regs_diff(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                       const dosdebug_regs_t *base)
{
    return dosdebug_write_regs_diff(ctx->db, regs, base);
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

void host_set_code_load(host_ctx_t *ctx, uint16_t seg)
{
    ctx->code_load_offset = seg;
    HYDRA_CONF->code_load_offset = seg;
    HYDRA_CONF->data_section_seg = ctx->data_section_seg;

    /* Keep the datasection basepointer coherent with the host-owned layout. */
    u16 dseg = (u16)(seg + ctx->data_section_seg);
    hydra_datasection_baseptr_set(lowmem_hostaddr(ctx->lm,
                                                 (uint32_t)dseg << 4));
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
    if (HYDRA_CONF->raw_code_offset == ctx->raw_code_addr) {
        HYDRA_CONF->raw_code_offset = 0;
        HYDRA_CONF->raw_code_size = 0;
    }
    ctx->db = NULL;
    ctx->lm = NULL;
    free(ctx);
}

/* ------------------------------------------------------------------ */
/* Hydra user metadata (required by functions.c / callstack.c)         */
/*                                                                    */
/* Without a user library these are empty stubs: they exist so that    */
/* api_impl.c's dlsym(RTLD_DEFAULT) binding of hydra_user_functions/   */
/* hydra_user_callstack always succeeds on this platform. When the     */
/* conf string carries lib=/path/user.so (see user_init), the real     */
/* providers are resolved from that library BY HANDLE and pushed into  */
/* the core via hydra_function_metadata_set()/                         */
/* hydra_callstack_metadata_set(), which take precedence over the      */
/* stubs cached at core init time.                                     */
/*                                                                    */
/* WHY BY-HANDLE RESOLUTION IS MANDATORY: dlsym(RTLD_DEFAULT, ...)     */
/* searches the default lookup scope, where this host object's own     */
/* stub symbols sit ahead of anything dlopened later — a user library  */
/* exporting the same names would ALWAYS be shadowed.                  */
/*                                                                    */
/* RULE: the user .so must NOT export hydra_user_init. The host owns   */
/* that symbol on this platform; api_impl binds THIS object's copy, so */
/* a hydra_user_init inside the user .so would never run (and any      */
/* one-time setup it attempted would silently not happen).             */
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

typedef const hydra_function_metadata_t *(*user_functions_fn_t)(void);
typedef const hydra_callstack_metadata_t *(*user_callstack_fn_t)(void);

/* Load the user metadata library (conf key "lib=<path>") and inject its
 * provider tables into the core. Both providers MUST be present: a library
 * that loads but is missing either symbol is a hard failure, never a
 * silent fallback to the empty stubs above.
 *
 * The dlopen handle is kept for the process lifetime (stored in ctx): the
 * injected tables point into the loaded object, so dlclose() would leave
 * the core holding dangling pointers. Leaking one handle at exit is the
 * deliberate trade. */
static void host_load_user_library(host_ctx_t *ctx, const char *path)
{
    ctx->user_lib = dlopen(path, RTLD_NOW | RTLD_GLOBAL);
    if (!ctx->user_lib)
        FAIL("dosemu host: failed to load user metadata library '%s': %s",
             path, dlerror());

    user_functions_fn_t ufunctions = NULL;
    *(void **)&ufunctions = dlsym(ctx->user_lib, "hydra_user_functions");
    if (!ufunctions)
        FAIL("dosemu host: user library '%s' does not export "
             "hydra_user_functions(): %s", path, dlerror());

    user_callstack_fn_t ucallstack = NULL;
    *(void **)&ucallstack = dlsym(ctx->user_lib, "hydra_user_callstack");
    if (!ucallstack)
        FAIL("dosemu host: user library '%s' does not export "
             "hydra_user_callstack(): %s", path, dlerror());

    const hydra_function_metadata_t *fmd = ufunctions();
    if (!fmd)
        FAIL("dosemu host: hydra_user_functions() from '%s' returned NULL",
             path);

    const hydra_callstack_metadata_t *cmd = ucallstack();
    if (!cmd)
        FAIL("dosemu host: hydra_user_callstack() from '%s' returned NULL",
             path);

    hydra_function_metadata_set(fmd);
    hydra_callstack_metadata_set(cmd);
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

    const char *confstr = HYDRA_CMDLINE_CONF ? HYDRA_CMDLINE_CONF : "";
    host_ctx_t *ctx = calloc(1, sizeof(*ctx));
    if (!ctx)
        FAIL("dosemu host: out of memory");

    ctx->code_load_offset = host_conf_u16(confstr, "code_load=", 0);
    ctx->data_section_seg = host_conf_u16(confstr, "data_seg=", 0);
    /* No implicit raw-code address. A guest/launcher-owned reservation must be
     * supplied with host_reserve_raw_code() after the target is loaded. */
    conf->raw_code_offset = 0;
    conf->raw_code_size = 0;

    /* Optional user metadata library ("lib=/path/user.so"). Loaded before
     * the emulator connection so a broken library fails fast. See the
     * metadata block above for by-handle resolution + the no-hydra_user_init
     * rule. */
    {
        char lib_path[4096];
        if (host_conf_string(confstr, "lib=", lib_path, sizeof(lib_path)))
            host_load_user_library(ctx, lib_path);
    }

    long pid = host_conf_pid(confstr);
    ctx->db = dosdebug_connect((pid_t)pid);
    if (!ctx->db)
        FAIL("dosemu host: failed to connect to dosemu2 (pid=%ld)", pid);
    ctx->pid = dosdebug_get_pid(ctx->db);

    /* mapmshm creates multiple identically named memfds. Cross-check two
     * independent dosdebug reads and require one unique matching backing. */
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
