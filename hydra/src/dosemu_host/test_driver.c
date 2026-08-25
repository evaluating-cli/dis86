/*
 * test_driver.c - Phase 4/5/6/7 integration test.
 *
 * Drives a real dosemu2 instance running testprog.com:
 *   1. waits until the guest program is loaded (CS:0100 matches the .COM),
 *   2. registers three Hydra hooks, each located by its unique
 *      `mov ax,imm16` signature in the loaded .COM,
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
 *
 * Phase 7 Item B adds user-metadata plumbing via dlopen (conf key lib=):
 *   - myfunc is registered BY NAME (HYDRA_REGISTER) and resolved through
 *     the metadata table injected from test_userlib (hydra_user.so),
 *   - hydra_function_name()/hydra_function_addr() resolve through the same
 *     injected table,
 *   - a fake-machine probe exercises the injected callstack confs
 *     (JUMPRET -> deferred leave -> unexpected-return warning).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <signal.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

#ifndef TESTPROG_COM_PATH
#define TESTPROG_COM_PATH "/tmp/opencode/testprog.com"
#endif

#ifndef TEST_USERLIB_PATH
#define TEST_USERLIB_PATH ""
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

/* myfunc is registered BY NAME via HYDRA_REGISTER(myfunc): hooks.c resolves
 * "F_myfunc" through the user metadata injected from test_userlib. */
#define IS_OVERLAY_ENTRY_F_myfunc 0

/* hook1: exercise the guest-opcode execution path (CLI/STI/INT/INB/OUTB) */
HYDRA_FUNC(H_myfunc)
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
  if (sz < 0 || sz > 32768) { fclose(f); return -1; }
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

static int release_go_flag(host_ctx_t *ctx, void *user)
{
  uint32_t goflag_phys = *(const uint32_t *)user;
  lowmem_write8(ctx->lm, goflag_phys, 1);
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
/* Phase 7 Item B: fake-machine probe for injected callstack confs     */
/*                                                                    */
/* Drives hydra_callstack_trigger_enter/notify/track against a local   */
/* RAM image: pushes one CALL frame, then stops at the JUMPRET conf    */
/* address supplied by the user library. The resulting leave pops the  */
/* frame from a mismatched site -> the core's "unexpected return       */
/* location" warning proves the INJECTED metadata was consumed.        */
/* ------------------------------------------------------------------ */

#define OFF_JMPLOOP_PROBE 0x013c

static uint8_t g_probe_ram[0x10000];

static uint8_t probe_mem_read8(hydra_machine_ctx_t *_ctx, uint32_t addr)
{
  (void)_ctx;
  return g_probe_ram[addr & 0xffff];
}

static uint16_t probe_mem_read16(hydra_machine_ctx_t *_ctx, uint32_t addr)
{
  (void)_ctx;
  addr &= 0xffff;
  return (uint16_t)(g_probe_ram[addr] |
                    ((uint16_t)g_probe_ram[(addr + 1) & 0xffff] << 8));
}

static void probe_mem_write8(hydra_machine_ctx_t *_ctx, uint32_t addr, uint8_t v)
{ (void)_ctx; g_probe_ram[addr & 0xffff] = v; }

static void probe_mem_write16(hydra_machine_ctx_t *_ctx, uint32_t addr, uint16_t v)
{
  (void)_ctx;
  addr &= 0xffff;
  g_probe_ram[addr] = (uint8_t)v;
  g_probe_ram[(addr + 1) & 0xffff] = (uint8_t)(v >> 8);
}

static uint8_t probe_io_in8(hydra_machine_ctx_t *_ctx, uint16_t port)
{ (void)_ctx; (void)port; return 0; }

static uint16_t probe_io_in16(hydra_machine_ctx_t *_ctx, uint16_t port)
{ (void)_ctx; (void)port; return 0; }

static void probe_io_out8(hydra_machine_ctx_t *_ctx, uint16_t port, uint8_t v)
{ (void)_ctx; (void)port; (void)v; }

static void probe_io_out16(hydra_machine_ctx_t *_ctx, uint16_t port, uint16_t v)
{ (void)_ctx; (void)port; (void)v; }

static void probe_noop_ctx(hydra_machine_ctx_t *_ctx, const char *label)
{ (void)_ctx; (void)label; }

static void probe_update_registers(hydra_machine_ctx_t *_ctx,
                                   hydra_machine_registers_t *regs)
{ (void)_ctx; (void)regs; }

static void metadata_probe(void)
{
  u16 css = CODE_START_SEG;

  /* jmp short at the JUMPRET site (a non-call/non-ret opcode so the
   * instruction classifier stays out of the way). */
  g_probe_ram[((u32)css << 4 | OFF_JMPLOOP_PROBE) & 0xffff] = 0xEB;

  hydra_machine_t pm = {0};
  pm.hardware->mem_read8         = probe_mem_read8;
  pm.hardware->mem_read16        = probe_mem_read16;
  pm.hardware->mem_write8        = probe_mem_write8;
  pm.hardware->mem_write16       = probe_mem_write16;
  pm.hardware->io_in8            = probe_io_in8;
  pm.hardware->io_in16           = probe_io_in16;
  pm.hardware->io_out8           = probe_io_out8;
  pm.hardware->io_out16          = probe_io_out16;
  pm.hardware->state_save        = probe_noop_ctx;
  pm.hardware->state_restore     = probe_noop_ctx;
  pm.hardware->update_registers  = probe_update_registers;

  /* 1. defer a CALL whose source is the guest entry (raw seg), then let
   *    notify() push it at a neutral CS:IP. */
  hydra_callstack_trigger_enter(css, 0x0100);
  pm.registers->cs = css;
  pm.registers->ip = 0x013d;
  hydra_callstack_notify(&pm);

  /* 2. "stop" at the injected JUMPRET conf -> deferred JMP_RET leave. */
  memset(pm.registers, 0, sizeof(pm.registers));
  pm.registers->cs = css;
  pm.registers->ip = OFF_JMPLOOP_PROBE;
  hydra_callstack_track(&pm, 0);

  /* 3. notify executes the leave; the popped frame expects 0000:0100 but
   *    we are leaving at 0000:013c -> warning emitted to stdout. */
  hydra_callstack_notify(&pm);
}

/* ------------------------------------------------------------------ */
/* main                                                               */
/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
  /* line-buffered: keep every check visible even if we die mid-run */
  setvbuf(stdout, NULL, _IOLBF, 0);
  /* a dead emulator must surface as dosdebug errors (clean FAIL unwind),
   * not as a SIGPIPE instant kill mid-diagnosis */
  signal(SIGPIPE, SIG_IGN);

  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *com_path = (argc > 2) ? argv[2] : TESTPROG_COM_PATH;
  /* optional lib= override (argv[3]): exercises the loader's failure paths
   * without needing a live dosemu (loading fails before the connect). */
  const char *lib_path = (argc > 3) ? argv[3] : TEST_USERLIB_PATH;

  printf("=== Phase 4/5/6 integration test ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(com_path, &file, &file_len) != 0) {
    printf("cannot read .COM: %s\n", com_path);
    return 2;
  }
  printf("guest program: %s (%zu bytes)\n", com_path, file_len);

  char conf[512];
  snprintf(conf, sizeof(conf),
            "dosemu|pid=%ld|code_load=0x80|data_seg=0x0|raw_code=0x1c00"
            "|lib=%s",
            (long)pid, lib_path);

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

  /* The real load segment is only known now: adopt it as CODE_START_SEG so
   * every address - including the metadata tables from the user library,
   * which are 0-based image-relative - is interpreted against the segment
   * the guest actually runs in (same pattern as the MZ/bpload path). */
  host_set_code_load(ctx, com_seg);

  /* 2a. Phase 7 Item B: user library loaded via lib=? Then the injected
   * function metadata must resolve names/addresses right now, before any
   * hook registration relies on it. */
  {
    const char *nm = hydra_function_name(ADDR_MAKE(0, 0x013e));
    CHECK(nm && strcmp(nm, "F_myfunc") == 0,
          "injected metadata: hydra_function_name(myfunc) == F_myfunc (got %s)",
          nm ? nm : "(null)");
    CHECK(hydra_function_name(ADDR_MAKE(0, 0xffee)) == NULL,
          "injected metadata: unknown address resolves to NULL");

    addr_t md_addr;
    CHECK(hydra_function_addr("F_func2", &md_addr) &&
              addr_seg(md_addr) == 0 && addr_off(md_addr) == 0x0142,
          "injected metadata: hydra_function_addr(F_func2) == 0000:0135");
  }

  /* 2b. fake-machine probe through the injected callstack confs */
  {
    /* capture hydra_callstack_dump()'s stdout into memory */
    char *capbuf = NULL;
    size_t caplen = 0;
    FILE *cap = open_memstream(&capbuf, &caplen);
    FILE *saved_stdout = stdout;
    if (cap) {
      stdout = cap;
      metadata_probe();
      fflush(cap);
      stdout = saved_stdout;
      fclose(cap);

      CHECK(strstr(capbuf, "Unexpected return location") != NULL,
            "callstack probe: injected JUMPRET conf drove an unexpected-"
            "return leave");
      printf("  callstack probe output:\n%s", capbuf);
      free(capbuf);
    } else {
      CHECK(0, "callstack probe: could not open capture stream");
    }
  }

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

  uint16_t hook_rel_seg = 0;   /* all addrs are 0-based image-relative */
  uint16_t myfunc_off   = (uint16_t)(0x100 + myfunc_pos);
  uint16_t func2_off    = (uint16_t)(0x100 + func2_pos);
  uint16_t callthru_off = (uint16_t)(0x100 + callthru_pos);
  printf("hooks: myfunc %04x:%04x func2 %04x:%04x callthru %04x:%04x "
         "(rel seg %04x)\n",
         com_seg, myfunc_off, com_seg, func2_off, com_seg, callthru_off,
         hook_rel_seg);

  /* 3. register the hooks. myfunc goes in BY NAME: hydra_impl_register
   * looks up "F_myfunc" through the metadata injected from test_userlib
   * (with no lib= this FAILs with "Cannot find function"). */
  {
    addr_t a;
    CHECK(hydra_function_addr("F_myfunc", &a) && addr_off(a) == myfunc_off,
          "metadata F_myfunc offset %04x matches signature scan", myfunc_off);
    CHECK(hydra_function_addr("F_func2", &a) && addr_off(a) == func2_off,
          "metadata F_func2 offset %04x matches signature scan", func2_off);
    CHECK(hydra_function_addr("F_callthru", &a) && addr_off(a) == callthru_off,
          "metadata F_callthru offset %04x matches signature scan", callthru_off);
  }

  HYDRA_REGISTER(myfunc);
  HYDRA_REGISTER_ADDR(h_test_hook2, hook_rel_seg, func2_off, 0);
  HYDRA_REGISTER_ADDR(h_callthru,   hook_rel_seg, callthru_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 3, "3 hooks registered "
        "(myfunc by NAME; breakpoint count = %d)",
        host_hook_breakpoint_count(ctx));

  g_hookcnt_phys  = com_phys + HOOKCNT_OFF;
  g_result_phys   = com_phys + RESULT_OFF;
  g_res4_phys     = com_phys + RES4_OFF;
  g_call_rel_seg  = hook_rel_seg;
  g_helper_off    = (uint16_t)(0x100 + helper_pos);
  g_helper2_off   = (uint16_t)(0x100 + helper2_pos);

  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = 5;
  uint32_t goflag_phys = com_phys + GOFLAG_OFF;
  opts.timeout_ms = 3000;
  opts.before_go_fn = release_go_flag;
  opts.before_go_user = &goflag_phys;
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
        "name-registered hook1 (F_myfunc) result observed (result=%04x)",
        result);
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
