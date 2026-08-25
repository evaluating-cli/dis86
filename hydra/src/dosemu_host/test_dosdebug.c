/*
 * test_dosdebug.c - exercises the dosdebug protocol client API against a
 * running dosemu2 instance.
 *
 * Usage: test_dosdebug [pid]
 *   pid  optional dosemu2 pid; defaults to auto-discovery.
 *
 * Tested functions: connect, read_regs, write_reg, read_mem, write_mem,
 * set_bp, clear_bp, go, wait_stop, is_alive, disconnect.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "dosdebug.h"

static int g_failures = 0;

#define CHECK(cond, label)                                              \
    do {                                                                \
        if (cond) {                                                     \
            printf("PASS: %s\n", label);                                \
        } else {                                                        \
            printf("FAIL: %s\n", label);                                \
            g_failures++;                                               \
        }                                                               \
    } while (0)

static void print_regs(const char *tag, const dosdebug_regs_t *r)
{
    printf("%s CS:IP=%04x:%04x  AX=%04x BX=%04x CX=%04x DX=%04x "
           "SI=%04x DI=%04x SP=%04x BP=%04x\n",
           tag, r->cs, r->ip, r->ax, r->bx, r->cx, r->dx,
           r->si, r->di, r->sp, r->bp);
    printf("%s DS=%04x ES=%04x SS=%04x FL=%04x\n",
           tag, r->ds, r->es, r->ss, r->flags);
}

static void print_bytes(const char *tag, const uint8_t *b, int n)
{
    printf("%s (%d bytes):", tag, n);
    for (int i = 0; i < n; i++)
        printf(" %02x", b[i]);
    printf("\n");
}

int main(int argc, char **argv)
{
    pid_t pid = 0;
    if (argc > 1)
        pid = (pid_t)strtol(argv[1], NULL, 0);

    dosdebug_t *db = dosdebug_connect(pid);
    CHECK(db != NULL, "dosdebug_connect");
    if (!db) {
        fprintf(stderr, "unable to connect to dosemu2 (pid=%ld)\n", (long)pid);
        return 1;
    }
    printf("connected to dosemu2\n");

    /* --- read registers --- */
    dosdebug_regs_t regs, check;
    CHECK(dosdebug_read_regs(db, &regs) == 0, "dosdebug_read_regs");
    print_regs("init", &regs);
    uint16_t orig_ax = regs.ax;
    uint16_t cs = regs.cs;
    uint16_t ip = regs.ip;

    /* --- write a register, read back to verify --- */
    CHECK(dosdebug_write_reg(db, "AX", 0x1234) == 0, "dosdebug_write_reg AX=0x1234");
    CHECK(dosdebug_read_regs(db, &check) == 0, "dosdebug_read_regs (after write)");
    CHECK(check.ax == 0x1234, "register write verified (AX==0x1234)");
    print_regs("after AX write", &check);

    /* --- read memory at CS:0100 --- */
    uint8_t mem[16];
    uint8_t orig_mem[16];
    int have_orig = 0;
    int got = dosdebug_read_mem(db, cs, 0x100, mem, 16);
    CHECK(got == 16, "dosdebug_read_mem 16 bytes @ CS:0100");
    if (got > 0) {
        memcpy(orig_mem, mem, (size_t)got);
        have_orig = (got == 16);
        print_bytes("mem @ CS:0100", mem, got);
    }

    /* --- write memory (NOP; JMP -2), read back to verify --- */
    uint8_t loop3[3] = { 0x90, 0xEB, 0xFE };
    uint8_t back[3];
    CHECK(dosdebug_write_mem(db, cs, 0x100, loop3, 3) == 0,
          "dosdebug_write_mem 3 bytes @ CS:0100");
    got = dosdebug_read_mem(db, cs, 0x100, back, 3);
    CHECK(got == 3 && memcmp(back, loop3, 3) == 0,
          "memory write verified (90 EB FE read back)");
    if (got > 0)
        print_bytes("mem after write", back, got);

    /*
     * --- breakpoint / go / wait_stop / clear_bp ---
     * Plant a short walk-loop at CS:0100 so the machine has somewhere to go
     * after 'g' (the testloop JMP at CS:0101 is a self-loop; a breakpoint at
     * the *current* IP never fires because dosemu only traps when the CS:IP
     * changes). The loop visits 0102,0103,0104,...,0106,0102,... so a
     * breakpoint at 0104 is hit on the first pass.
     *
     *   0100: EB 04       jmp 0106
     *   0102: 90          nop
     *   0103: 90          nop
     *   0104: 90          nop      <- breakpoint here (byte becomes CC)
     *   0105: 90          nop
     *   0106: EB FA       jmp 0102
     */
    uint8_t walkloop[8] = { 0xEB, 0x04, 0x90, 0x90, 0x90, 0x90, 0xEB, 0xFA };
    CHECK(dosdebug_write_mem(db, cs, 0x100, walkloop, 8) == 0,
          "plant walk-loop @ CS:0100");
    CHECK(dosdebug_write_reg(db, "IP", 0x0102) == 0, "set IP=0102");

    const uint16_t bp_off = 0x0104;
    int bpi = dosdebug_set_bp(db, cs, bp_off);
    CHECK(bpi >= 0, "dosdebug_set_bp at CS:0104");
    printf("breakpoint index = %d\n", bpi);

    CHECK(dosdebug_go(db) == 0, "dosdebug_go");

    dosdebug_regs_t stopregs;
    memset(&stopregs, 0, sizeof(stopregs));
    CHECK(dosdebug_wait_stop(db, &stopregs, 3000) == 0, "dosdebug_wait_stop");
    CHECK(stopregs.cs == cs && stopregs.ip == bp_off,
          "stopped at breakpoint CS:0104");
    print_regs("after bp hit", &stopregs);

    CHECK(bpi >= 0 && dosdebug_clear_bp(db, bpi) == 0, "dosdebug_clear_bp");
    CHECK(dosdebug_stop(db) == 0, "dosdebug_stop");

    /* --- restore machine state --- */
    CHECK(dosdebug_write_reg(db, "IP", ip) == 0, "restore IP");
    CHECK(dosdebug_write_reg(db, "AX", orig_ax) == 0, "restore AX");
    if (have_orig) {
        CHECK(dosdebug_write_mem(db, cs, 0x100, orig_mem, 16) == 0,
              "restore memory @ CS:0100");
        dosdebug_read_mem(db, cs, 0x100, back, 3);
        CHECK(memcmp(back, orig_mem, 3) == 0, "restore verified");
    }

    /* --- alive check + cleanup --- */
    CHECK(dosdebug_is_alive(db), "dosdebug_is_alive");

    dosdebug_disconnect(db);
    printf("disconnected\n");

    if (g_failures == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("%d TEST(S) FAILED\n", g_failures);
    return 1;
}