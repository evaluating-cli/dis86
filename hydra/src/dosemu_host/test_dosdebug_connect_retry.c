/*
 * test_dosdebug_connect_retry.c - dosemu2-free regression for the connect race
 * fixed in #146.
 *
 * Before the fix, dosdebug_connect() made exactly one attempt:
 *
 *   - discover_pid() does a single readdir pass and requires *both*
 *     dosemu.dbgin.<pid> and dosemu.dbgout.<pid> to exist already, so it
 *     returns -1 if dosemu2 has not published the pair yet;
 *   - open(dbgin, O_WRONLY | O_NONBLOCK) returns ENXIO until dosemu2 has
 *     opened its read end.
 *
 * dosemu2 publishes the pair early in startup and then removes and recreates
 * it ("removing stale files: .../dosemu.dbgin.<pid>"), so a connector that
 * looks into that window failed ~1 s into the Phase 7 lane.
 *
 * Each case stands up a fake dosemu2 peer - a forked child that creates the
 * two FIFOs named after its own pid and answers the stop/r0 handshake - and
 * then calls dosdebug_connect() against it. No dosemu2 installation and no
 * legacy build step: only the FIFO endpoint and its line protocol.
 *
 * Cases:
 *   1. late publication   - the pair appears only after the first attempt
 *   2. reader lag (ENXIO) - the pair exists but dbgin has no reader yet
 *   3. never published    - connect must be bounded, not instant and not hang
 *   4. silent peer        - a handshake failure stays terminal (not retried)
 */

#define _POSIX_C_SOURCE 200809L

#include "dosdebug.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int g_failures;

#define CHECK(cond, label)                                              \
    do {                                                                \
        if (cond) {                                                     \
            printf("PASS: %s\n", label);                                \
        } else {                                                        \
            printf("FAIL: %s\n", label);                                \
            g_failures++;                                               \
        }                                                               \
    } while (0)

static char g_dir[1024];   /* <XDG_RUNTIME_DIR>/dosemu2 */

static long now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static void sleep_ms(int ms)
{
    struct timespec ts;
    if (ms <= 0)
        return;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    while (nanosleep(&ts, &ts) != 0 && errno == EINTR)
        continue;
}

static void fifo_paths(pid_t p, char *in, size_t nin, char *out, size_t nout)
{
    snprintf(in, nin, "%s/dosemu.dbgin.%ld", g_dir, (long)p);
    snprintf(out, nout, "%s/dosemu.dbgout.%ld", g_dir, (long)p);
}

static int write_all(int fd, const char *s)
{
    size_t len = strlen(s), off = 0;
    while (off < len) {
        ssize_t r = write(fd, s + off, len - off);
        if (r < 0) {
            if (errno == EINTR)
                continue;
            return -1;
        }
        off += (size_t)r;
    }
    return 0;
}

/*
 * A complete real-mode dump in the shape parse_regs_from_buffer() expects:
 * eight GPRs, the segment/flag line, CS:IP, SS:SP, then the disassembly line
 * that terminates a 'r0' response.
 */
static const char DUMP_TEXT[] =
    "AX=1234 BX=2345 CX=3456 DX=4567 SI=5678 DI=6789 SP=0000 BP=0000\n"
    "DS=1111 ES=2222 FS=3333 GS=4444 FL=00000246\n"
    "CS:IP=0A00:0100\n"
    "SS:SP=0B00:0000\n"
    "0A00:0100 90          nop\n";

typedef struct {
    int publish_delay_ms;   /* before the first (dbgin) fifo appears */
    int pair_gap_ms;        /* between dbgin and dbgout appearing */
    int reader_delay_ms;    /* after the pair, before dbgin gains a reader */
    int silent;             /* 1 = publish the pair but never answer */
    int notify_fd;          /* write one byte once the pair exists, -1 = none */
} peer_opts_t;

static void peer_serve(int rfd, int wfd, int silent)
{
    char line[256];
    size_t len = 0;
    long deadline = now_ms() + 20000;   /* never orphan the test run */

    while (now_ms() < deadline) {
        struct pollfd pfd;
        pfd.fd = rfd;
        pfd.events = POLLIN;
        pfd.revents = 0;

        int pr = poll(&pfd, 1, 200);
        if (pr < 0) {
            if (errno == EINTR)
                continue;
            break;
        }
        if (pr == 0)
            continue;

        char chunk[256];
        ssize_t n = read(rfd, chunk, sizeof(chunk));
        if (n <= 0)
            break;          /* client closed the write end: session over */

        for (ssize_t i = 0; i < n; i++) {
            if (chunk[i] == '\n') {
                line[len] = '\0';
                len = 0;
                if (silent)
                    continue;
                if (strcmp(line, "stop") == 0) {
                    /* Real dosemu2 acknowledges; keep it free of "AX=" so the
                     * following r0 dump cannot be confused with it. */
                    if (write_all(wfd, "stopped\n") != 0)
                        return;
                } else if (strcmp(line, "r0") == 0) {
                    if (write_all(wfd, DUMP_TEXT) != 0)
                        return;
                }
            } else if (len + 1 < sizeof(line)) {
                line[len++] = chunk[i];
            }
        }
    }
}

static void peer_run(pid_t me, const peer_opts_t *o)
{
    char in_path[4096], out_path[4096];
    fifo_paths(me, in_path, sizeof(in_path), out_path, sizeof(out_path));

    sleep_ms(o->publish_delay_ms);
    if (mkfifo(in_path, 0600) != 0)
        _exit(20);
    sleep_ms(o->pair_gap_ms);
    if (mkfifo(out_path, 0600) != 0)
        _exit(21);

    if (o->notify_fd >= 0) {
        char c = 'P';
        (void)!write(o->notify_fd, &c, 1);   /* pair is on disk: let the test go */
        close(o->notify_fd);
    }

    sleep_ms(o->reader_delay_ms);

    /* Becoming the reader is what closes the ENXIO window: the client's
     * non-blocking O_WRONLY open fails until this open is pending. */
    int rfd = open(in_path, O_RDONLY);
    if (rfd < 0)
        _exit(22);

    /* The client holds dbgout's read end for the whole session, so this
     * succeeds; retry anyway so the two pids cannot deadlock on ordering. */
    int wfd = -1;
    long deadline = now_ms() + 5000;
    while (now_ms() < deadline) {
        wfd = open(out_path, O_WRONLY | O_NONBLOCK);
        if (wfd >= 0)
            break;
        if (errno != ENXIO && errno != ENOENT && errno != EINTR)
            _exit(23);
        sleep_ms(10);
    }
    if (wfd < 0)
        _exit(24);
    (void)fcntl(wfd, F_SETFL, 0);

    peer_serve(rfd, wfd, o->silent);
    _exit(0);
}

static pid_t peer_start(const peer_opts_t *o)
{
    pid_t pid = fork();
    if (pid != 0)
        return pid;
    peer_run(getpid(), o);
    _exit(0);
}

static void peer_stop(pid_t peer)
{
    char in_path[4096], out_path[4096];
    int st;

    if (peer <= 0)
        return;
    fifo_paths(peer, in_path, sizeof(in_path), out_path, sizeof(out_path));
    unlink(in_path);
    unlink(out_path);
    kill(peer, SIGKILL);
    while (waitpid(peer, &st, 0) < 0 && errno == EINTR)
        continue;
}

/* Wait for the peer's one-byte "pair is published" notice. */
static void wait_for_pair(int fd)
{
    char c;
    while (read(fd, &c, 1) < 0 && errno == EINTR)
        continue;
    close(fd);
}

/* 1. The FIFO pair appears only after connect() has already looked once. */
static void test_late_publication(void)
{
    peer_opts_t o = { 250, 50, 0, 0, -1 };
    pid_t peer = peer_start(&o);
    long t0 = now_ms();
    dosdebug_t *db = dosdebug_connect(peer);
    long dt = now_ms() - t0;

    CHECK(db != NULL, "late publication: connect waits for the fifo pair");
    if (db) {
        CHECK(dosdebug_get_pid(db) == peer,
              "late publication: bound to the peer pid");
        dosdebug_regs_t r;
        CHECK(dosdebug_read_regs(db, &r) == 0,
              "late publication: handshake produced a usable connection");
        dosdebug_disconnect(db);
    }
    CHECK(dt >= 200, "late publication: connect actually waited (~300 ms)");
    peer_stop(peer);
}

/* 2. Both FIFOs exist, but dbgin has no reader yet -> ENXIO on the open. */
static void test_reader_lag(void)
{
    int sync[2];
    CHECK(pipe(sync) == 0, "reader lag: sync pipe");

    peer_opts_t o = { 0, 0, 400, 0, sync[1] };
    pid_t peer = peer_start(&o);
    close(sync[1]);
    wait_for_pair(sync[0]);   /* pair is definitely on disk, reader is not */

    long t0 = now_ms();
    dosdebug_t *db = dosdebug_connect(peer);
    long dt = now_ms() - t0;

    CHECK(db != NULL, "reader lag: connect retries through ENXIO");
    if (db)
        dosdebug_disconnect(db);
    CHECK(dt >= 200, "reader lag: connect waited for the reader (~400 ms)");
    peer_stop(peer);
}

/* 3. A live pid with no FIFOs at all: the exact wire shape of the #146 failure.
 *    The wait must be bounded, and it must not be instant - "instant" was the
 *    bug, since a single look is what the old code did. */
static void test_never_published(void)
{
    peer_opts_t o = { 60000, 0, 0, 0, -1 };
    pid_t peer = peer_start(&o);

    long t0 = now_ms();
    dosdebug_t *db = dosdebug_connect(peer);
    long dt = now_ms() - t0;

    CHECK(db == NULL, "never published: connect gives up instead of hanging");
    CHECK(dt >= 1200, "never published: retried for the full budget (>= 1.2 s)");
    CHECK(dt <= 15000, "never published: gave up promptly (<= 15 s)");
    peer_stop(peer);
}

/* 4. The pair is up but the peer never answers: a handshake timeout is a
 *    protocol error, so it must fail once rather than re-run stop/r0 20 times. */
static void test_silent_peer(void)
{
    int sync[2];
    CHECK(pipe(sync) == 0, "silent peer: sync pipe");

    peer_opts_t o = { 0, 0, 0, 1, sync[1] };
    pid_t peer = peer_start(&o);
    close(sync[1]);
    wait_for_pair(sync[0]);

    long t0 = now_ms();
    dosdebug_t *db = dosdebug_connect(peer);
    long dt = now_ms() - t0;

    CHECK(db == NULL, "silent peer: a failed handshake is terminal");
    /* stop-drain (800 ms) + dump timeout (2000 ms) once; a retried handshake
     * would cost that twenty times over. */
    CHECK(dt >= 2000, "silent peer: waited out the handshake timeouts");
    CHECK(dt <= 15000, "silent peer: handshake was not retried");
    peer_stop(peer);
}

int main(void)
{
    char tmpl[] = "/tmp/dosdebug-connect-retry-XXXXXX";
    char *root = mkdtemp(tmpl);
    if (!root) {
        perror("mkdtemp");
        return 2;
    }
    snprintf(g_dir, sizeof(g_dir), "%s/dosemu2", root);
    if (mkdir(g_dir, 0700) != 0) {
        perror("mkdir");
        return 2;
    }
    /* The client resolves the endpoint through XDG_RUNTIME_DIR on every call. */
    if (setenv("XDG_RUNTIME_DIR", root, 1) != 0) {
        perror("setenv");
        return 2;
    }
    printf("runtime dir: %s\n", g_dir);

    test_late_publication();
    test_reader_lag();
    test_never_published();
    test_silent_peer();

    rmdir(g_dir);
    rmdir(root);

    if (g_failures == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("%d TEST(S) FAILED\n", g_failures);
    return 1;
}
