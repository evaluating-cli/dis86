/*
 * test_driver.c - Phase 4/5/6 integration test.
 *
 * Drives a real dosemu2 instance running testprog.com:
 *   1. waits until the guest program is loaded (CS:0100 matches the .COM),
 *   2. registers three Hydra hooks (myfunc, func2, callthru), each located
 *      by its unique `mov ax,imm16` signature in the loaded .COM,
 *   3. releases the guest (go flag) and host_run()s it until the guest-side
 *      hook counter reaches the target,
 *   4. verifies:
 *      - multiple simultaneous hooks dispatch independently,
 *      - guest opcodes (cli/sti/int/in/out) ran to completion via trace,
 *      - native->guest CALL_FAR through to unhooked guest functions completes
 *        (helper2: >6 instructions, beyond the old 6-step trace limit),
 *      - a hooked function called from inside such a guest call dispatches
 *        as a nested hook mid-trace,
 *      - guest flags survive the hook roundtrip (CF=1 via pushf),
 *      - the guest program observed the hooks' results.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

#ifndef TESTPROG_COM_PATH
#define TESTPROG_COM_PATH "/tmp/opencode/testprog.com"
#endif

/* testprog.asm layout (org 100h) */
#define GOFLAG_OFF  0x102
#define RESULT_OFF  0x103
#define HOOKCNT_OFF 0x105
#define RESULT2_OFF 0x107
#define RES3_OFF    0x109
#define RES4_OFF    0x10b
#define FLAGRES_OFF 0x10d
#define CODE_LOAD   0x0080

/* Unique function signatures (`mov ax,imm16` bodies, kept unique in the asm) */
static const uint8_t myfunc_sig[]   = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t func2_sig[]    = { 0xB8, 0x22, 0x22, 0xC3 };
static const uint8_t callthru_sig[] = { 0xB8, 0x33, 0x33, 0xC3 };
static const uint8_t helper2_sig[]  = { 0xB8, 0xCE, 0x7A };
static const uint8_t helper_sig[]   = { 0xB8, 0x44, 0x44 };

static uint32_t g_hookcnt_phys;
static uint32_t g_result_phys;
static uint32_t g_res4_phys;
static uint16_t g_call_rel_seg;   /* com_seg - CODE_START_SEG */
static uint16_t g_helper_off;
static uint16_t g_helper2_off;
static int      g_fails;

#define CHECK(cond, ...) do {                                    \
  if (!(cond)) {                                                 \
    printf("  FAIL: "); printf(__VA_ARGS__); printf("\n");       \
    g_fails++;                                                   \
  } else {                                                       \
    printf("  ok:   "); printf(__VA_ARGS__); printf("\n");       \
  }                                                              \
} while (0)

static const char *reason_name(host_run_stop_reason_t r)
{
  switch (r) {
    case HOST_RUN_STOP_NONE:        return "NONE";
    case HOST_RUN_STOP_CALLBACK:    return "CALLBACK";
    case HOST_RUN_STOP_MAXSTEPS:    return "MAXSTEPS";
    case HOST_RUN_STOP_DOSEMU_EXIT: return "DOSEMU_EXIT";
    case HOST_RUN_STOP_TIMEOUT:     return "TIMEOUT";
    case HOST_RUN_STOP_ERROR:       return "ERROR";
  }
  return "?";
}

/* ------------------------------------------------------------------ */
/* the hooks                                                          */
/* ------------------------------------------------------------------ */

/* hook1: exercise the guest-opcode execution path (CLI/STI/INT/INB/OUTB) */
HYDRA_FUNC(h_test_hook)
{
  CLI();
  STI();
  INT(0x28);
  u8 t = INB(0x40);
  OUTB(0x80, t);

  /* record into guest memory (host reads this after host_run) */
  u16 cnt = m->hardware->mem_read16(m->hardware->ctx, g_hookcnt_phys);
  m->hardware->mem_write16(m->hardware->ctx, g_hookcnt_phys, (u16)(cnt + 1));
  m->registers->ax = (u16)(0xBE00u | t);

  RETURN_NEAR();
}

/* hook2: one trivial raw code; target of both direct guest calls and the
 * nested call from inside helper's guest code (mid-trace dispatch). */
HYDRA_FUNC(h_test_hook2)
{
  NOP();
  m->registers->ax = 0xCAFE;
  RETURN_NEAR();
}

/* hook3: native->guest calls through CALL_FAR into unhooked guest code.
 * helper2 is 11 instructions (defeats the old 6-step trace limit);
 * helper near-calls the hooked func2 mid-trace (nested hook dispatch). */
HYDRA_FUNC(h_callthru)
{
  u32 r2 = hydra_impl_call_far(g_call_rel_seg, g_helper2_off);
  m->hardware->mem_write16(m->hardware->ctx, g_res4_phys, (u16)r2);

  u32 r1 = hydra_impl_call_far(g_call_rel_seg, g_helper_off);
  m->registers->ax = (u16)r1;

  RETURN_NEAR();
}

/* ------------------------------------------------------------------ */
/* helpers                                                            */
/* ------------------------------------------------------------------ */

static int read_file(const char *path, uint8_t **out, size_t *out_len)
{
  FILE *f = fopen(path, "rb");
  if (!f) return -1;
  if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return -1; }
  long sz = ftell(f);
  if (sz < 0 || sz > 4096) { fclose(f); return -1; }
  rewind(f);
  uint8_t *buf = malloc((size_t)sz);
  if (!buf || fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
    free(buf); fclose(f); return -1;
  }
  fclose(f);
  *out = buf;
  *out_len = (size_t)sz;
  return 0;
}

/* Run the guest for ms, then stop it. Returns 0 on success. */
static int go_sleep_stop(host_ctx_t *ctx, int ms)
{
  if (dosdebug_go(ctx->db) != 0) return -1;
  usleep((useconds_t)ms * 1000u);
  return dosdebug_stop(ctx->db);
}

/* Poll until the .COM is loaded; returns its segment (0 on failure). */
static uint16_t wait_for_com(host_ctx_t *ctx, const uint8_t *file, size_t file_len)
{
  size_t check_len = file_len < 16 ? file_len : 16;

  for (int i = 0; i < 80; i++) {
    dosdebug_regs_t r;
    if (host_get_regs(ctx, &r) != 0) return 0;

    uint8_t m32[16];
    int got = dosdebug_read_mem(ctx->db, r.cs, 0x100, m32, (int)check_len);
    if (got == (int)check_len && memcmp(m32, file, check_len) == 0)
      return r.cs;

    if (go_sleep_stop(ctx, 250) != 0) return 0;
  }
  return 0;
}

static int stop_when_hooked(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                            const host_run_stats_t *stats, void *user)
{
  (void)regs;
  (void)stats;
  size_t target = *(size_t *)user;
  return lowmem_read16(ctx->lm, g_hookcnt_phys) >= target;
}

/* Find a function's org-100h offset by unique signature. */
static size_t find_sig(const uint8_t *file, size_t file_len,
                       const uint8_t *sig, size_t sig_len)
{
  for (size_t i = 0; i + sig_len <= file_len; i++) {
    if (memcmp(file + i, sig, sig_len) == 0)
      return i;
  }
  return (size_t)-1;
}

/* ------------------------------------------------------------------ */
/* main                                                               */
/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *com_path = (argc > 2) ? argv[2] : TESTPROG_COM_PATH;

  printf("=== Phase 4/5/6 integration test ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(com_path, &file, &file_len) != 0) {
    printf("cannot read .COM: %s\n", com_path);
    return 2;
  }
  printf("guest program: %s (%zu bytes)\n", com_path, file_len);

  char conf[160];
  snprintf(conf, sizeof(conf),
            "dosemu|pid=%ld|code_load=0x80|data_seg=0x0|raw_code=0x1c00",
           (long)pid);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;

  printf("connected to dosemu2 pid=%ld\n", (long)host_pid(ctx));

  /* 1. wait for the guest program to be loaded */
  printf("waiting for the guest program to load...\n");
  uint16_t com_seg = wait_for_com(ctx, file, file_len);
  CHECK(com_seg != 0, "guest program loaded (segment %04x)", com_seg);
  if (com_seg == 0) { host_disconnect(ctx); free(file); return 1; }

  uint32_t com_phys = (uint32_t)com_seg << 4;
  printf("guest CS=%04x (linear %06x), raw-code region 0x1c00\n",
         com_seg, com_phys);

  /* 2. locate every function by signature */
  size_t myfunc_pos   = find_sig(file, file_len, myfunc_sig, sizeof myfunc_sig);
  size_t func2_pos    = find_sig(file, file_len, func2_sig, sizeof func2_sig);
  size_t callthru_pos = find_sig(file, file_len, callthru_sig, sizeof callthru_sig);
  size_t helper2_pos  = find_sig(file, file_len, helper2_sig, sizeof helper2_sig);
  size_t helper_pos   = find_sig(file, file_len, helper_sig, sizeof helper_sig);
  CHECK(myfunc_pos != (size_t)-1 && func2_pos != (size_t)-1 &&
        callthru_pos != (size_t)-1, "hook signatures found");
  CHECK(helper2_pos != (size_t)-1 && helper_pos != (size_t)-1,
        "helper signatures found");
  if (myfunc_pos == (size_t)-1 || func2_pos == (size_t)-1 ||
      callthru_pos == (size_t)-1 || helper2_pos == (size_t)-1 ||
      helper_pos == (size_t)-1) {
    host_disconnect(ctx); free(file); return 1;
  }

  uint16_t hook_rel_seg = (uint16_t)(com_seg - CODE_LOAD);
  uint16_t myfunc_off   = (uint16_t)(0x100 + myfunc_pos);
  uint16_t func2_off    = (uint16_t)(0x100 + func2_pos);
  uint16_t callthru_off = (uint16_t)(0x100 + callthru_pos);
  printf("hooks: myfunc %04x:%04x func2 %04x:%04x callthru %04x:%04x "
         "(rel seg %04x)\n",
         com_seg, myfunc_off, com_seg, func2_off, com_seg, callthru_off,
         hook_rel_seg);

  /* 3. register the hooks */
  HYDRA_REGISTER_ADDR(h_test_hook,  hook_rel_seg, myfunc_off, 0);
  HYDRA_REGISTER_ADDR(h_test_hook2, hook_rel_seg, func2_off, 0);
  HYDRA_REGISTER_ADDR(h_callthru,   hook_rel_seg, callthru_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 3, "3 hooks registered "
        "(breakpoint count = %d)", host_hook_breakpoint_count(ctx));

  g_hookcnt_phys  = com_phys + HOOKCNT_OFF;
  g_result_phys   = com_phys + RESULT_OFF;
  g_res4_phys     = com_phys + RES4_OFF;
  g_call_rel_seg  = hook_rel_seg;
  g_helper_off    = (uint16_t)(0x100 + helper_pos);
  g_helper2_off   = (uint16_t)(0x100 + helper2_pos);

  /* 4. release the guest (host_run plants/clears breakpoints itself) */
  lowmem_write8(ctx->lm, com_phys + GOFLAG_OFF, 1);
  printf("go flag set; running the guest...\n");

  /* 5. run the driver loop until the guest hook counter reaches target */
  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = 5;
  opts.timeout_ms = 3000;
  opts.stop_fn = stop_when_hooked;
  opts.stop_user = &target;
  opts.verbose = 1;

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);

  printf("host_run ended: %s\n", reason_name(reason));

  {
    /* diagnostics: where did the CPU end up? */
    dosdebug_regs_t d;
    if (host_get_regs(ctx, &d) == 0)
      printf("  cpu now at %04x:%04x (sp=%04x ss=%04x)\n",
             d.cs, d.ip, d.sp, d.ss);
  }
  printf("  stops=%lu hook_dispatches=%lu raw_code_runs=%lu "
         "raw_code_returns=%lu redirects=%lu\n",
         (unsigned long)stats.stops, (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs, (unsigned long)stats.raw_code_returns,
         (unsigned long)stats.redirects);

  /* 6. verify */
  uint16_t hookcnt = lowmem_read16(ctx->lm, g_hookcnt_phys);
  uint16_t result  = lowmem_read16(ctx->lm, com_phys + RESULT_OFF);
  uint16_t result2 = lowmem_read16(ctx->lm, com_phys + RESULT2_OFF);
  uint16_t res3    = lowmem_read16(ctx->lm, com_phys + RES3_OFF);
  uint16_t res4    = lowmem_read16(ctx->lm, com_phys + RES4_OFF);
  uint16_t flagres = lowmem_read16(ctx->lm, com_phys + FLAGRES_OFF);

  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
  CHECK(hookcnt >= target, "guest hook counter >= %lu (got %u)",
        (unsigned long)target, hookcnt);
  CHECK((result & 0xFF00u) == 0xBE00u,
        "hook1 result observed (result=%04x)", result);
  CHECK(result2 == 0xCAFE,
        "hook2 result observed (result2=%04x)", result2);
  CHECK(res4 == 0x7ACE,
        "native->guest callthrough completed (helper2 13-insn, res4=%04x)",
        res4);
  CHECK(res3 == 0xCAFE || res3 == 0x2222,
        "nested callthrough returned (res3=%04x, %s)", res3,
        res3 == 0xCAFE ? "nested hook fired during trace"
                       : "bp did NOT fire during trace");
  CHECK((flagres & 0x0001u) == 0x0001u,
        "flags preserved across hook3 (CF=1, flagres=%04x)", flagres);
  CHECK(stats.raw_code_returns == stats.raw_code_runs,
        "every raw code / guest call returned via trace (raw=%lu ret=%lu)",
        (unsigned long)stats.raw_code_runs, (unsigned long)stats.raw_code_returns);

  /* Deterministic dispatch accounting: hookcnt hits `target` exactly at
   * hook1 dispatch #target, so we stop with target hook1 dispatches and
   * (target-1) of each of hook2, hook3, and the nested hook2 dispatch:
   *   dispatches = target + 3*(target-1) = 5+12 = 17
   *   raw runs   = 5*target + (1)*(target-1) + (2+1)*(target-1) = 25+4+12 = 41
   *   redirects  = raw runs + dispatches (each dispatch redirects once when
   *                it begins before completing) */
  {
    uint64_t want_dispatches = (uint64_t)target + 3 * (target - 1);
    uint64_t want_raws = 5 * (uint64_t)target + 4 * (target - 1);
    CHECK(stats.hook_dispatches == want_dispatches,
          "hook dispatches == %lu (got %lu)", (unsigned long)want_dispatches,
          (unsigned long)stats.hook_dispatches);
    CHECK(stats.raw_code_runs == want_raws,
          "raw code runs == %lu (got %lu)", (unsigned long)want_raws,
          (unsigned long)stats.raw_code_runs);
    CHECK(stats.redirects == stats.raw_code_runs + stats.hook_dispatches,
          "redirects == raws+dispatches (%lu == %lu)",
          (unsigned long)stats.redirects,
          (unsigned long)(stats.raw_code_runs + stats.hook_dispatches));
  }
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  printf("  hook1 result byte = %02x (from guest in 0x40)\n",
         (unsigned)(result & 0xff));

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}
