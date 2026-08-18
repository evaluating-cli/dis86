/*
 * test_host.c - Phase 3 integration test: dosemu2 host wired into Hydra's
 * hydra_machine_hardware_t vtable.
 *
 * Usage: test_host [pid]
 *   pid  optional dosemu2 pid; if omitted the host auto-discovers.
 *
 * Requires a running headless dosemu2 instance (see the launch script in
 * /tmp/opencode). Steps exercised:
 *   1. hydra_machine_init() (api_impl path -> dlsyms hydra_user_init,
 *      hydra_user_functions, hydra_user_callstack from libhydra_dosemu)
 *   2. update_registers (PULL) via the vtable
 *   3. mem_hostaddr / mem_read8/16 via the vtable (IVT + BDA)
 *   4. register write (host_set_reg) + read-back via update_registers
 *   5. state_save / state_restore
 *   6. breakpoint / go / wait-stop
 *   7. hydra_machine_exec sanity (no hooks -> returns 0 / RESUME)
 *   8. host_disconnect
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hydra_machine.h"
#include "host.h"

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
           "SI=%04x DI=%04x SP=%04x BP=%04x DS=%04x ES=%04x SS=%04x FL=%04x\n",
           tag, r->cs, r->ip, r->ax, r->bx, r->cx, r->dx,
           r->si, r->di, r->sp, r->bp, r->ds, r->es, r->ss, r->flags);
}

int main(int argc, char **argv)
{
    char conf[128] = "dosemu";
    if (argc > 1) {
        long pid = strtol(argv[1], NULL, 0);
        if (pid <= 0) {
            fprintf(stderr, "invalid pid: %s\n", argv[1]);
            return 2;
        }
        snprintf(conf, sizeof(conf), "dosemu|pid=%ld", pid);
    }

    /* --- 1. hydra_machine_init --- */
    hydra_machine_t m = {0};
    hydra_machine_audio_t audio = {0};
    hydra_machine_init(m.hardware, &audio, conf);
    printf("hydra_machine_init() returned; vtable ctx=%p\n", (void *)m.hardware->ctx);

    host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;
    CHECK(ctx != NULL, "hw->ctx is set (host_ctx_t)");
    if (!ctx) {
        fprintf(stderr, "no host context; aborting\n");
        return 1;
    }

    CHECK(host_pid(ctx) != 0, "host connected to a dosemu2 pid");
    printf("dosemu2 pid = %ld\n", (long)host_pid(ctx));
    if (argc > 1 && host_pid(ctx) != (pid_t)strtol(argv[1], NULL, 0))
        CHECK(0, "connected pid matches requested pid");

    CHECK(ctx->have_initial_regs, "initial register state read at connect");
    print_regs("init", &ctx->initial_regs);

    /* --- 2. update_registers (PULL) via the vtable --- */
    m.hardware->update_registers(m.hardware->ctx, m.registers);
    printf("update_registers CS:IP=%04x:%04x  AX=%04x BX=%04x CX=%04x DX=%04x "
           "SI=%04x DI=%04x SP=%04x BP=%04x\n",
           m.registers->cs, m.registers->ip, m.registers->ax, m.registers->bx,
           m.registers->cx, m.registers->dx, m.registers->si, m.registers->di,
           m.registers->sp, m.registers->bp);
    CHECK(m.registers->cs != 0 || m.registers->ip != 0,
          "update_registers populated registers");
    printf("  (CS:IP=%04x:%04x)\n", m.registers->cs, m.registers->ip);

    /* --- 3. guest memory via the vtable --- */
    uint8_t *ivt = m.hardware->mem_hostaddr(m.hardware->ctx, 0x0000);
    CHECK(ivt != NULL, "mem_hostaddr(0x0000) (IVT) mapped");
    if (ivt) {
        uint16_t v0 = m.hardware->mem_read16(m.hardware->ctx, 0x0000);
        printf("IVT[0] = %04x (int 0x00 handler seg:off)\n", v0);
        CHECK(v0 != 0, "IVT vector 0 is non-zero");
    }

    uint8_t *bda = m.hardware->mem_hostaddr(m.hardware->ctx, 0x0400);
    CHECK(bda != NULL, "mem_hostaddr(0x0400) (BDA) mapped");
    if (bda) {
        uint8_t eq = m.hardware->mem_read8(m.hardware->ctx, 0x0410);
        printf("BDA equipment word low byte @ 0x410 = 0x%02x\n", eq);
        CHECK(eq != 0xff, "BDA equipment byte is not 0xff");
    }

    /* write/read via vtable at a scratch location (0x5000 is above DOS) */
    const uint32_t scratch = 0x50000;
    uint8_t orig = m.hardware->mem_read8(m.hardware->ctx, scratch);
    m.hardware->mem_write8(m.hardware->ctx, scratch, 0x5a);
    CHECK(m.hardware->mem_read8(m.hardware->ctx, scratch) == 0x5a,
          "mem_write8/mem_read8 round-trip at 0x50000");
    m.hardware->mem_write8(m.hardware->ctx, scratch, orig);

    m.hardware->mem_write16(m.hardware->ctx, scratch, 0xbeef);
    CHECK(m.hardware->mem_read16(m.hardware->ctx, scratch) == 0xbeef,
          "mem_write16/mem_read16 round-trip at 0x50000");
    m.hardware->mem_write8(m.hardware->ctx, scratch, orig);

    /* --- 4. register write + read-back --- */
    dosdebug_regs_t cur;
    CHECK(host_get_regs(ctx, &cur) == 0, "host_get_regs");
    uint16_t orig_ax = cur.ax;
    CHECK(host_set_reg(ctx, "AX", 0x1234) == 0, "host_set_reg(AX, 0x1234)");
    m.hardware->update_registers(m.hardware->ctx, m.registers);
    CHECK(m.registers->ax == 0x1234,
          "register write verified via update_registers (AX=0x1234)");

    /* --- 5. state_save / state_restore --- */
    m.hardware->state_save(m.hardware->ctx, "t1");
    CHECK(host_set_reg(ctx, "AX", 0x5555) == 0, "set AX=0x5555 after save");
    CHECK(host_get_regs(ctx, &cur) == 0 && cur.ax == 0x5555,
          "AX changed to 0x5555");
    m.hardware->state_restore(m.hardware->ctx, "t1");
    CHECK(host_get_regs(ctx, &cur) == 0 && cur.ax == 0x1234,
          "state_restore restored AX=0x1234");

    /* --- 6. breakpoint / go / wait-stop --- */
    uint16_t cs = cur.cs;
    uint16_t ip = cur.ip;

    uint8_t walkloop[8] = { 0xEB, 0x04, 0x90, 0x90, 0x90, 0x90, 0xEB, 0xFA };
    for (int i = 0; i < 8; i++)
        m.hardware->mem_write8(m.hardware->ctx, (uint32_t)cs * 16 + 0x100 + i,
                               walkloop[i]);
    CHECK(host_set_reg(ctx, "IP", 0x0102) == 0, "set IP=0102");

    const uint16_t bp_off = 0x0104;
    int bpi = host_set_bp(ctx, cs, bp_off);
    CHECK(bpi >= 0, "host_set_bp at CS:0104");
    printf("breakpoint index = %d\n", bpi);

    dosdebug_regs_t stopregs;
    memset(&stopregs, 0, sizeof(stopregs));
    CHECK(host_go_and_wait(ctx, &stopregs, 4000) == 0, "host_go_and_wait");
    CHECK(stopregs.cs == cs && stopregs.ip == bp_off,
          "stopped at breakpoint CS:0104");
    print_regs("after bp hit", &stopregs);

    CHECK(bpi >= 0 && host_clear_bp(ctx, bpi) == 0, "host_clear_bp");
    CHECK(host_stop(ctx) == 0, "host_stop");

    /* restore the walk-loop region + registers */
    for (int i = 0; i < 8; i++)
        m.hardware->mem_write8(m.hardware->ctx, (uint32_t)cs * 16 + 0x100 + i,
                               0x90);
    CHECK(host_set_reg(ctx, "IP", ip) == 0, "restore IP");
    CHECK(host_set_reg(ctx, "AX", orig_ax) == 0, "restore AX");

    /* --- 7. hydra_machine_exec sanity --- */
    m.hardware->update_registers(m.hardware->ctx, m.registers);
    int ret = hydra_machine_exec(&m, 0);
    printf("hydra_machine_exec(interrupt_count=0) = %d\n", ret);
    CHECK(ret == 0, "hydra_machine_exec returns 0 with no hook (RESUME)");

    /* --- 8. disconnect --- */
    CHECK(host_pid(ctx) != 0, "host_pid before disconnect");
    host_disconnect(ctx);
    printf("disconnected\n");

    if (g_failures == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("%d TEST(S) FAILED\n", g_failures);
    return 1;
}