/*
 * dosdebug.c - dosemu2 debugger FIFO protocol client.
 *
 * Talks to the stock dosemu2 binary via its debugger FIFOs
 * ($XDG_RUNTIME_DIR/dosemu2/dosemu.dbgin.<pid> and dosemu.dbgout.<pid>).
 *
 * CRITICAL: the dbgin writer fd must be kept open for the whole session.
 * Every close() of the write end sends EOF to dosemu2 which makes it
 * auto-continue the machine. We open it once in connect() and only close
 * it in disconnect().
 *
 * The protocol is line-oriented text. Responses are free-form (no framing),
 * so all reads are bounded by poll() timeouts.
 */

#define _POSIX_C_SOURCE 200809L

#include "dosdebug.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define BUF_SIZE 8192
#define QUIET_MS 80          /* silence threshold: response considered done */
#define DEFAULT_TIMEOUT_MS 5000

/* Fallback when XDG_RUNTIME_DIR is not set (Phase 0 default). */
#define DEFAULT_XDG_RUNTIME "/tmp/opencode/runtime"

struct dosdebug {
    pid_t pid;          /* dosemu2 process id */
    int in_fd;          /* dbgin  writer fd (kept open!) */
    int out_fd;         /* dbgout reader fd */
    bool connected;
    char buf[BUF_SIZE + 1];   /* read buffer (+1 for NUL terminator) */
    size_t bpos;              /* first unconsumed byte */
    size_t blen;              /* one past last valid byte */
};

/* ---------------- low-level helpers ---------------- */

static int hexval(int c)
{
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

static bool ishex(int c)
{
    return hexval(c) >= 0;
}

static void buf_nul_terminate(dosdebug_t *db)
{
    db->buf[db->blen] = '\0';
}

/* Move unconsumed data to the front of the buffer. */
static void buf_compact(dosdebug_t *db)
{
    if (db->bpos == 0) return;
    if (db->bpos < db->blen)
        memmove(db->buf, db->buf + db->bpos, db->blen - db->bpos);
    db->blen -= db->bpos;
    db->bpos = 0;
    buf_nul_terminate(db);
}

/*
 * poll() the dbgout fd and read whatever is available into the buffer.
 * Returns bytes read (>0), 0 on timeout, -1 on error/EOF.
 */
static int recv_more(dosdebug_t *db, int timeout_ms)
{
    if (db->blen == BUF_SIZE) {
        buf_compact(db);
        if (db->blen == BUF_SIZE)
            return -1; /* buffer overflow: protocol broken */
    }
    if (db->bpos > 0)
        buf_compact(db);

    struct pollfd pfd;
    pfd.fd = db->out_fd;
    pfd.events = POLLIN;
    pfd.revents = 0;

    int rc = poll(&pfd, 1, timeout_ms);
    if (rc < 0) {
        if (errno == EINTR)
            return 0;
        return -1;
    }
    if (rc == 0)
        return 0; /* timeout */
    if (!(pfd.revents & POLLIN)) {
        if (pfd.revents & (POLLHUP | POLLERR | POLLNVAL))
            return -1;
        return 0;
    }

    ssize_t r = read(db->out_fd, db->buf + db->blen, BUF_SIZE - db->blen);
    if (r < 0) {
        if (errno == EINTR)
            return 0;
        return -1;
    }
    if (r == 0)
        return -1; /* EOF: dosemu2 closed the debugger fifo */
    db->blen += (size_t)r;
    buf_nul_terminate(db);
    return (int)r;
}

/* Write a whole command line to dbgin. */
static int send_cmd(dosdebug_t *db, const char *cmd)
{
    size_t len = strlen(cmd);
    size_t off = 0;
    while (off < len) {
        ssize_t r = write(db->in_fd, cmd + off, len - off);
        if (r < 0) {
            if (errno == EINTR)
                continue;
            return -1;
        }
        off += (size_t)r;
    }
    return 0;
}

static bool txt_has(const char *s, size_t n, const char *needle)
{
    size_t nl = strlen(needle);
    if (nl == 0) return true;
    if (n < nl) return false;
    for (size_t i = 0; i + nl <= n; i++) {
        if (memcmp(s + i, needle, nl) == 0)
            return true;
    }
    return false;
}

/*
 * Read until the fifo has been quiet for QUIET_MS. Used to fully consume a
 * command response whose length is not known in advance.
 */
static int read_until_quiet(dosdebug_t *db, int first_wait_ms)
{
    int n = recv_more(db, first_wait_ms);
    if (n < 0)
        return -1;
    if (n > 0) {
        for (;;) {
            n = recv_more(db, QUIET_MS);
            if (n < 0)
                return -1;
            if (n == 0)
                break;
        }
    }
    return 0;
}

/* ---------------- register dump parsing ---------------- */

/* Parse exactly 4 hex digits into *out. Returns 0 on failure. */
static int parse_hex4(const char *p, uint16_t *out)
{
    int v = 0;
    for (int i = 0; i < 4; i++) {
        int h = hexval(p[i]);
        if (h < 0)
            return 0;
        v = (v << 4) | h;
    }
    if (ishex((unsigned char)p[4]))
        return 0; /* more than 4 digits: not a 16-bit field */
    *out = (uint16_t)v;
    return 1;
}

/* Parse up to 8 hex digits into *out (low 16 bits used). */
static int parse_hex8(const char *p, uint16_t *out)
{
    int v = 0;
    for (int i = 0; i < 8; i++) {
        int h = hexval(p[i]);
        if (h < 0)
            return 0;
        v = (v << 4) | h;
    }
    if (ishex((unsigned char)p[8]))
        return 0;
    *out = (uint16_t)(v & 0xffff);
    return 1;
}

/* Forward search helper (memmem-style, avoids GNU dependency). */
static const char *find_sub(const char *s, size_t n, const char *needle)
{
    size_t nl = strlen(needle);
    if (n < nl)
        return NULL;
    for (size_t i = 0; i + nl <= n; i++) {
        if (memcmp(s + i, needle, nl) == 0)
            return s + i;
    }
    return NULL;
}

/* Skip horizontal whitespace (register dump fields are "NAME=xxxx  NAME=..."). */
static const char *skip_spaces(const char *cur, const char *end)
{
    while (cur < end && (*cur == ' ' || *cur == '\t'))
        cur++;
    return cur;
}

/*
 * Try to parse a complete register dump from the current buffer.
 * Returns 1 (parsed + consumed), 0 (incomplete, need more data),
 * -1 (buffer holds garbage that can never form a dump).
 */
static int parse_regs_from_buffer(dosdebug_t *db, dosdebug_regs_t *regs)
{
    const char *s = db->buf + db->bpos;
    size_t n = db->blen - db->bpos;

    /* The real-mode dump always has an "AX=" line. */
    const char *p = find_sub(s, n, "AX=");
    if (!p)
        return 0;

    const char *cur = p;
    const char *end = s + n;

    static const char *gnames[8] = { "AX", "BX", "CX", "DX",
                                     "SI", "DI", "SP", "BP" };
    uint16_t gpr[8];
    for (int i = 0; i < 8; i++) {
        if ((size_t)(end - cur) < 3 || strncmp(cur, gnames[i], 2) != 0 || cur[2] != '=')
            return 0;
        cur += 3;
        if (!parse_hex4(cur, &gpr[i]))
            return 0;
        cur = skip_spaces(cur + 4, end);
    }

    /* Segment line: DS= ES= FS= GS= FL= */
    const char *q = find_sub(cur, (size_t)(end - cur), "DS=");
    if (!q)
        return 0;
    cur = q;
    uint16_t ds, es, fs, gs, fl;
    (void)fs;
    (void)gs;
    if ((size_t)(end - cur) < 3 || strncmp(cur, "DS=", 3) != 0)
        return 0;
    cur += 3;
    if (!parse_hex4(cur, &ds))
        return 0;
    cur = skip_spaces(cur + 4, end);
    if ((size_t)(end - cur) < 3 || strncmp(cur, "ES=", 3) != 0)
        return 0;
    cur += 3;
    if (!parse_hex4(cur, &es))
        return 0;
    cur = skip_spaces(cur + 4, end);
    if ((size_t)(end - cur) < 3 || strncmp(cur, "FS=", 3) != 0)
        return 0;
    cur += 3;
    if (!parse_hex4(cur, &fs))
        return 0;
    cur = skip_spaces(cur + 4, end);
    if ((size_t)(end - cur) < 3 || strncmp(cur, "GS=", 3) != 0)
        return 0;
    cur += 3;
    if (!parse_hex4(cur, &gs))
        return 0;
    cur = skip_spaces(cur + 4, end);
    if ((size_t)(end - cur) < 3 || strncmp(cur, "FL=", 3) != 0)
        return 0;
    cur += 3;
    if (!parse_hex8(cur, &fl))
        return 0;
    cur += 8;

    /* CS:IP= line */
    const char *r = find_sub(cur, (size_t)(end - cur), "CS:IP=");
    if (!r)
        return 0;
    cur = r + 6;
    uint16_t cs, ip, ss, sp;
    if (!parse_hex4(cur, &cs))
        return 0;
    cur += 4;
    if (cur >= end || *cur != ':')
        return 0;
    cur++;
    if (!parse_hex4(cur, &ip))
        return 0;
    cur += 4;

    ss = 0;
    sp = 0;
    const char *spos = find_sub(cur, (size_t)(end - cur), "SS:SP=");
    if (spos) {
        cur = spos + 6;
        if (parse_hex4(cur, &ss))
            cur += 4;
        if (cur < end && *cur == ':')
            cur++;
        if (parse_hex4(cur, &sp))
            cur += 4;
    }

    regs->ax = gpr[0]; regs->bx = gpr[1]; regs->cx = gpr[2]; regs->dx = gpr[3];
    regs->si = gpr[4]; regs->di = gpr[5]; regs->sp = gpr[6]; regs->bp = gpr[7];
    regs->ds = ds; regs->es = es; regs->ss = ss;
    regs->flags = fl;
    regs->cs = cs; regs->ip = ip;

    /*
     * Consume through the CS:IP= line plus one following line (the
     * disassembly emitted by the 'u' command that terminates a dump).
     */
    const char *csip_start = r;
    const char *line_end = find_sub(csip_start, (size_t)(end - csip_start), "\n");
    if (!line_end)
        return 0; /* dump not fully arrived yet */
    size_t consumed = (size_t)(line_end - s) + 1;
    if (consumed < n) {
        const char *dasm_end = find_sub(line_end + 1, (size_t)(end - (line_end + 1)), "\n");
        if (dasm_end)
            consumed = (size_t)(dasm_end - s) + 1;
    }
    db->bpos += consumed;
    buf_nul_terminate(db);
    return 1;
}

/*
 * Read until a complete register dump has been parsed or the timeout
 * expires. Returns 0 on success (regs filled), -1 on timeout/EOF.
 */
static int read_dump_block(dosdebug_t *db, dosdebug_regs_t *regs, int timeout_ms)
{
    struct timespec t0;
    clock_gettime(CLOCK_MONOTONIC, &t0);

    for (;;) {
        int r = parse_regs_from_buffer(db, regs);
        if (r == 1) {
            /* Consume everything else that was buffered with this response so
             * the next command starts from a clean buffer. */
            db->bpos = db->blen = 0;
            buf_nul_terminate(db);
            return 0;
        }
        if (r == -1)
            return -1;

        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        long elapsed_ms = (long)(now.tv_sec - t0.tv_sec) * 1000
                        + (now.tv_nsec - t0.tv_nsec) / 1000000;
        long remaining = (long)timeout_ms - elapsed_ms;
        if (remaining <= 0)
            return -1;

        int n = recv_more(db, remaining > 30000 ? 30000 : (int)remaining);
        if (n < 0)
            return -1;
    }
}

/* ---------------- memory dump parsing ---------------- */

/*
 * Parse the hex byte columns of a 'd' response into out[0..maxlen).
 *
 * Each row is:  <addr>  HH HH HH ... HH  <ascii trailer>
 * The address token is skipped; then at most 16 two-digit hex bytes are
 * parsed per row. Rows are capped at 16 bytes so the ASCII trailer (which
 * may itself contain hex-looking characters) is never misread.
 */
static int parse_dump_hex(const char *s, size_t n, uint8_t *out, int maxlen)
{
    int got = 0;
    const char *p = s;
    const char *end = s + n;

    while (p < end && got < maxlen) {
        /* skip leading whitespace / blank lines */
        while (p < end && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r'))
            p++;
        if (p >= end)
            break;

        /* skip the address token (e.g. "ffff:0000" or "#00b7:0100") */
        while (p < end && !(*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r'))
            p++;

        /* parse up to 16 bytes from this row */
        for (int k = 0; k < 16 && got < maxlen; k++) {
            while (p < end && (*p == ' ' || *p == '\t'))
                p++;
            if (p + 1 >= end)
                break;
            int h0 = hexval((unsigned char)p[0]);
            int h1 = hexval((unsigned char)p[1]);
            if (h0 < 0 || h1 < 0)
                break; /* end of byte columns / ASCII trailer */
            out[got++] = (uint8_t)((h0 << 4) | h1);
            p += 2;
        }

        /* skip the rest of this line (ASCII trailer, if any) */
        while (p < end && *p != '\n')
            p++;
    }
    return got;
}

/* Parse "N: <linear>" lines from a 'bl' response; return matching index. */
static int parse_bl_index(const char *s, size_t n, uint32_t linear)
{
    const char *p = s;
    const char *end = s + n;
    while (p < end) {
        const char *eol = find_sub(p, (size_t)(end - p), "\n");
        size_t llen = eol ? (size_t)(eol - p) : (size_t)(end - p);
        char line[64];
        if (llen >= sizeof(line))
            llen = sizeof(line) - 1;
        memcpy(line, p, llen);
        line[llen] = '\0';

        int idx;
        unsigned long addr;
        if (sscanf(line, "%d: %lx", &idx, &addr) == 2) {
            if ((uint32_t)addr == linear)
                return idx;
        }
        p = eol ? eol + 1 : end;
    }
    return -1;
}

/* ---------------- discovery / connection ---------------- */

static const char *xdg_runtime_dir(void)
{
    const char *e = getenv("XDG_RUNTIME_DIR");
    if (e && e[0])
        return e;
    return DEFAULT_XDG_RUNTIME;
}

/*
 * Locate a dosemu2 instance. If wanted != 0, require that exact pid.
 * Otherwise pick the first live instance found.
 */
static pid_t discover_pid(pid_t wanted)
{
    char dir[8192];
    snprintf(dir, sizeof(dir), "%s/dosemu2", xdg_runtime_dir());

    DIR *d = opendir(dir);
    if (!d)
        return -1;

    pid_t found = -1;
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        const char *nn = ent->d_name;
        if (strncmp(nn, "dosemu.dbgin.", 13) != 0)
            continue;
        long p = atol(nn + 13);
        if (p <= 0)
            continue;
        if (wanted != 0 && p != wanted)
            continue;

        char in_path[8192], out_path[8192];
        if (snprintf(in_path, sizeof(in_path), "%s/%s", dir, nn) >= (int)sizeof(in_path))
            continue;
        if (snprintf(out_path, sizeof(out_path), "%s/dosemu.dbgout.%ld",
                     dir, p) >= (int)sizeof(out_path))
            continue;
        if (access(in_path, F_OK) != 0 || access(out_path, F_OK) != 0)
            continue;
        if (kill((pid_t)p, 0) != 0)
            continue; /* stale fifo, process gone */

        found = (pid_t)p;
        break;
    }
    closedir(d);
    return found;
}

dosdebug_t *dosdebug_connect(pid_t pid)
{
    pid_t p = discover_pid(pid);
    if (p < 0)
        return NULL;

    char in_path[8192], out_path[8192];
    snprintf(in_path, sizeof(in_path), "%s/dosemu2/dosemu.dbgin.%ld",
             xdg_runtime_dir(), (long)p);
    snprintf(out_path, sizeof(out_path), "%s/dosemu2/dosemu.dbgout.%ld",
             xdg_runtime_dir(), (long)p);

    dosdebug_t *db = calloc(1, sizeof(*db));
    if (!db)
        return NULL;
    db->pid = p;
    db->in_fd = -1;
    db->out_fd = -1;

    /* O_NONBLOCK so open() never blocks; cleared right after. */
    db->out_fd = open(out_path, O_RDONLY | O_NONBLOCK);
    if (db->out_fd < 0)
        goto fail;
    db->in_fd = open(in_path, O_WRONLY | O_NONBLOCK);
    if (db->in_fd < 0)
        goto fail;
    fcntl(db->out_fd, F_SETFL, 0);
    fcntl(db->in_fd, F_SETFL, 0);

    /* Handshake: make sure the machine is stopped, then grab a register dump.
     * The 'stop' response (which may be a dump if the machine was running)
     * is fully drained so the following 'r0' dump is a clean, parseable one.
     */
    if (send_cmd(db, "stop\n") != 0)
        goto fail;
    if (read_until_quiet(db, 800) != 0)
        goto fail;
    if (send_cmd(db, "r0\n") != 0)
        goto fail;

    dosdebug_regs_t r;
    if (read_dump_block(db, &r, 2000) != 0)
        goto fail;

    db->connected = true;
    return db;

fail:
    dosdebug_disconnect(db);
    return NULL;
}

void dosdebug_disconnect(dosdebug_t *db)
{
    if (!db)
        return;
    if (db->in_fd >= 0) {
        close(db->in_fd); /* EOF here makes dosemu2 auto-continue; expected */
        db->in_fd = -1;
    }
    if (db->out_fd >= 0) {
        close(db->out_fd);
        db->out_fd = -1;
    }
    db->connected = false;
    free(db);
}

pid_t dosdebug_get_pid(dosdebug_t *db)
{
    if (!db || !db->connected)
        return 0;
    return db->pid;
}

/* ---------------- public API ---------------- */

/* Issue 'stop'; drains its response. Idempotent: safe whether running/stopped.
 * The buffer is reset so no stale 'stop' text leaks into the next command. */
static int ensure_stopped(dosdebug_t *db)
{
    if (send_cmd(db, "stop\n") != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return 0;
}

int dosdebug_read_regs(dosdebug_t *db, dosdebug_regs_t *regs)
{
    if (!db || !db->connected || !regs)
        return -1;
    if (ensure_stopped(db) != 0)
        return -1;
    if (send_cmd(db, "r0\n") != 0)
        return -1;
    if (read_dump_block(db, regs, 2000) != 0)
        return -1;
    return 0;
}

int dosdebug_write_reg(dosdebug_t *db, const char *reg_name, uint16_t val)
{
    if (!db || !db->connected || !reg_name)
        return -1;
    if (ensure_stopped(db) != 0)
        return -1;

    char cmd[64];
    /* dosemu's number parser is decimal by default; '0x' is required for hex. */
    snprintf(cmd, sizeof(cmd), "r %s 0x%04X\n", reg_name, (unsigned)val & 0xffffu);
    if (send_cmd(db, cmd) != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;

    size_t n = db->blen - db->bpos;
    int ok = txt_has(db->buf + db->bpos, n, "changed to");
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return ok ? 0 : -1;
}

int dosdebug_write_regs(dosdebug_t *db, const dosdebug_regs_t *regs)
{
    if (!db || !regs)
        return -1;

    struct {
        const char *name;
        uint16_t val;
    } all[] = {
        { "AX", regs->ax }, { "BX", regs->bx }, { "CX", regs->cx },
        { "DX", regs->dx }, { "SI", regs->si }, { "DI", regs->di },
        { "BP", regs->bp }, { "SP", regs->sp }, { "IP", regs->ip },
        { "CS", regs->cs }, { "DS", regs->ds }, { "ES", regs->es },
        { "SS", regs->ss }, { "FL", regs->flags },
    };
    for (size_t i = 0; i < sizeof(all) / sizeof(all[0]); i++) {
        uint16_t v = all[i].val;
        if (strcmp(all[i].name, "FL") == 0) {
            /*
             * dosemu's set_FLAGS() forces IF (0x200) and IOPL on, then
             * verifies; a write succeeds only for compatible bit patterns.
             * Getting FL exactly right is a dosemu limitation, so a FL
             * failure does not fail the whole batch.
             */
            if (dosdebug_write_reg(db, all[i].name, v) != 0)
                continue;
            continue;
        }
        if (dosdebug_write_reg(db, all[i].name, v) != 0)
            return -1;
    }
    return 0;
}

int dosdebug_set_bp(dosdebug_t *db, uint16_t seg, uint16_t off)
{
    if (!db || !db->connected)
        return -1;
    if (ensure_stopped(db) != 0)
        return -1;

    char cmd[64];
    snprintf(cmd, sizeof(cmd), "bp %04X:%04X\n", (unsigned)seg, (unsigned)off);
    if (send_cmd(db, cmd) != 0)
        return -1;
    if (read_until_quiet(db, 500) != 0)
        return -1;
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);

    /* 'bp' prints nothing on success; query the list for the index. */
    if (send_cmd(db, "bl\n") != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;

    uint32_t linear = ((uint32_t)seg << 4) + (uint32_t)off;
    int idx = parse_bl_index(db->buf + db->bpos, db->blen - db->bpos, linear);
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return idx;
}

int dosdebug_clear_bp(dosdebug_t *db, int bp_index)
{
    if (!db || !db->connected || bp_index < 0)
        return -1;
    if (ensure_stopped(db) != 0)
        return -1;

    char cmd[64];
    snprintf(cmd, sizeof(cmd), "bc %d\n", bp_index);
    if (send_cmd(db, cmd) != 0)
        return -1;
    if (read_until_quiet(db, 500) != 0)
        return -1;

    size_t n = db->blen - db->bpos;
    int ok = !txt_has(db->buf + db->bpos, n, "Invalid")
          && !txt_has(db->buf + db->bpos, n, "No breakpoint");
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return ok ? 0 : -1;
}

int dosdebug_go(dosdebug_t *db)
{
    if (!db || !db->connected)
        return -1;
    if (send_cmd(db, "g\n") != 0)
        return -1;
    /*
     * No immediate response expected; drain whatever is there. IMPORTANT:
     * the buffer is deliberately NOT reset here - a stop notification that
     * arrives quickly (e.g. a breakpoint at the current CS:IP) may land in
     * the buffer and must be visible to a subsequent dosdebug_wait_stop().
     */
    if (read_until_quiet(db, 400) != 0)
        return -1;
    return 0;
}

int dosdebug_stop(dosdebug_t *db)
{
    if (!db || !db->connected)
        return -1;
    if (send_cmd(db, "stop\n") != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return 0;
}

int dosdebug_wait_stop(dosdebug_t *db, dosdebug_regs_t *regs, int timeout_ms)
{
    if (!db || !db->connected)
        return -1;
    int total = timeout_ms > 0 ? timeout_ms : DEFAULT_TIMEOUT_MS;

    struct timespec t0;
    clock_gettime(CLOCK_MONOTONIC, &t0);

    for (;;) {
        int r = parse_regs_from_buffer(db, regs);
        if (r == 1)
            return 0;
        if (r == -1)
            return -1;

        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        long elapsed_ms = (long)(now.tv_sec - t0.tv_sec) * 1000
                        + (now.tv_nsec - t0.tv_nsec) / 1000000;
        long remaining = (long)total - elapsed_ms;
        if (remaining <= 0)
            return -1;

        int n = recv_more(db, remaining > 30000 ? 30000 : (int)remaining);
        if (n < 0)
            return -1;
    }
}

int dosdebug_read_mem(dosdebug_t *db, uint16_t seg, uint16_t off,
                      uint8_t *buf, int len)
{
    if (!db || !db->connected || !buf || len <= 0)
        return -1;
    if (len > 256)
        len = 256;
    if (ensure_stopped(db) != 0)
        return -1;

    char cmd[64];
    snprintf(cmd, sizeof(cmd), "d %04X:%04X %d\n",
             (unsigned)seg, (unsigned)off, len);
    if (send_cmd(db, cmd) != 0)
        return -1;
    if (read_until_quiet(db, 800) != 0)
        return -1;

    int got = parse_dump_hex(db->buf + db->bpos, db->blen - db->bpos, buf, len);
    db->bpos = db->blen = 0;
    buf_nul_terminate(db);
    return got > 0 ? got : -1;
}

int dosdebug_write_mem(dosdebug_t *db, uint16_t seg, uint16_t off,
                       const uint8_t *buf, int len)
{
    if (!db || !db->connected || !buf || len < 0)
        return -1;
    if (len == 0)
        return 0;
    if (ensure_stopped(db) != 0)
        return -1;

    /*
     * dosemu's debugger parses each command line into at most MAXARG=16
     * tokens (mhpdbgc.c) and its argparse() breaks on the 15th token
     * WITHOUT null-terminating it, so the 13th value gets the rest of the
     * line glued on and fails with "Value invalid". To stay clean, write
     * at most 12 bytes per command.
     */
    const int chunk = 12;
    int pos = 0;
    while (pos < len) {
        int c = len - pos;
        if (c > chunk)
            c = chunk;

        /* " 0x%02X" is 5 chars per byte; size generously. */
        char cmd[32 + 5 * chunk];
        int n = snprintf(cmd, sizeof(cmd), "m %04X:%04X",
                         (unsigned)seg, (unsigned)(off + pos));
        for (int i = 0; i < c; i++) {
            n += snprintf(cmd + n, sizeof(cmd) - (size_t)n, " 0x%02X",
                          buf[pos + i]);
        }
        n += snprintf(cmd + n, sizeof(cmd) - (size_t)n, "\n");

        if (send_cmd(db, cmd) != 0)
            return -1;
        if (read_until_quiet(db, 800) != 0)
            return -1;

        size_t rn = db->blen - db->bpos;
        int ok = txt_has(db->buf + db->bpos, rn, "Modified")
              && !txt_has(db->buf + db->bpos, rn, "Invalid");
        db->bpos = db->blen = 0;
        buf_nul_terminate(db);
        if (!ok)
            return -1;

        pos += c;
    }
    return 0;
}

bool dosdebug_is_alive(dosdebug_t *db)
{
    if (!db || !db->connected)
        return false;
    if (db->in_fd < 0 || db->out_fd < 0)
        return false;
    if (kill(db->pid, 0) != 0)
        return false;
    return true;
}