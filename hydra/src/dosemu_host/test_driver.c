/*
 * Real dosemu2 integration test for the external Hydra host.
 *
 * Covers: 3 simultaneous hooks, raw guest opcodes, native->guest callthrough,
 * nested hooks, CF preservation, IF=0 preservation, and an explicitly
 * guest-owned raw-code reservation whose bytes are restored after execution.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

#ifndef TESTPROG_COM_PATH
#define TESTPROG_COM_PATH "/tmp/opencode/testprog.com"
#endif

#define GOFLAG_OFF   0x102
#define RESULT_OFF   0x103
#define HOOKCNT_OFF  0x105
#define RESULT2_OFF  0x107
#define RES3_OFF     0x109
#define RES4_OFF     0x10b
#define FLAGRES_OFF  0x10d
#define IFFLAGRES_OFF 0x10f
#define CODE_LOAD    0x0080
#define RAW_SCRATCH_SIZE 8192u

static const uint8_t myfunc_sig[]   = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t func2_sig[]    = { 0xB8, 0x22, 0x22, 0xC3 };
static const uint8_t callthru_sig[] = { 0xB8, 0x33, 0x33, 0xC3 };
static const uint8_t helper2_sig[]  = { 0xB8, 0xCE, 0x7A };
static const uint8_t helper_sig[]   = { 0xB8, 0x44, 0x44 };
static const uint8_t raw_marker[]   = "HYDRA_RAW_SLOT!!"; /* 16 bytes + C NUL */

static uint32_t g_hookcnt_phys;
static uint32_t g_res4_phys;
static uint16_t g_call_rel_seg;
static uint16_t g_helper_off;
static uint16_t g_helper2_off;
static int g_fails;

#define CHECK(cond, ...) do {                                      \
  if (!(cond)) {                                                   \
    printf("  FAIL: "); printf(__VA_ARGS__); printf("\n");         \
    g_fails++;                                                     \
  } else {                                                         \
    printf("  ok:   "); printf(__VA_ARGS__); printf("\n");         \
  }                                                                \
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

HYDRA_FUNC(h_test_hook)
{
  CLI();
  STI();
  INT(0x28);
  u8 t = INB(0x40);
  OUTB(0x80, t);

  u16 cnt = m->hardware->mem_read16(m->hardware->ctx, g_hookcnt_phys);
  m->hardware->mem_write16(m->hardware->ctx, g_hookcnt_phys, (u16)(cnt + 1));
  m->registers->ax = (u16)(0xBE00u | t);
  RETURN_NEAR();
}

HYDRA_FUNC(h_test_hook2)
{
  NOP();
  m->registers->ax = 0xCAFE;
  RETURN_NEAR();
}

HYDRA_FUNC(h_callthru)
{
  u32 r2 = hydra_impl_call_far(g_call_rel_seg, g_helper2_off);
  m->hardware->mem_write16(m->hardware->ctx, g_res4_phys, (u16)r2);

  u32 r1 = hydra_impl_call_far(g_call_rel_seg, g_helper_off);
  m->registers->ax = (u16)r1;
  RETURN_NEAR();
}

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

static int go_sleep_stop(host_ctx_t *ctx, int ms)
{
  if (dosdebug_go(ctx->db) != 0) return -1;
  usleep((useconds_t)ms * 1000u);
  return dosdebug_stop(ctx->db);
}

static uint16_t wait_for_com(host_ctx_t *ctx, const uint8_t *file, size_t file_len)
{
  size_t check_len = file_len < 16 ? file_len : 16;
  for (int i = 0; i < 80; i++) {
    dosdebug_regs_t r;
    if (host_get_regs(ctx, &r) != 0) return 0;
    uint8_t m16[16];
    int got = dosdebug_read_mem(ctx->db, r.cs, 0x100, m16, (int)check_len);
    if (got == (int)check_len && memcmp(m16, file, check_len) == 0)
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

static size_t find_sig(const uint8_t *file, size_t file_len,
                       const uint8_t *sig, size_t sig_len)
{
  for (size_t i = 0; i + sig_len <= file_len; i++) {
    if (memcmp(file + i, sig, sig_len) == 0)
      return i;
  }
  return (size_t)-1;
}

int main(int argc, char **argv)
{
  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *com_path = (argc > 2) ? argv[2] : TESTPROG_COM_PATH;

  printf("=== Hydra dosemu2 integration test ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(com_path, &file, &file_len) != 0) {
    printf("cannot read .COM: %s\n", com_path);
    return 2;
  }

  char conf[128];
  snprintf(conf, sizeof(conf), "dosemu|pid=%ld|code_load=0x80|data_seg=0x0",
           (long)pid);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;

  uint16_t com_seg = wait_for_com(ctx, file, file_len);
  CHECK(com_seg != 0, "guest program loaded (segment %04x)", com_seg);
  if (!com_seg) { host_disconnect(ctx); free(file); return 1; }
  uint32_t com_phys = (uint32_t)com_seg << 4;

  size_t myfunc_pos   = find_sig(file, file_len, myfunc_sig, sizeof myfunc_sig);
  size_t func2_pos    = find_sig(file, file_len, func2_sig, sizeof func2_sig);
  size_t callthru_pos = find_sig(file, file_len, callthru_sig, sizeof callthru_sig);
  size_t helper2_pos  = find_sig(file, file_len, helper2_sig, sizeof helper2_sig);
  size_t helper_pos   = find_sig(file, file_len, helper_sig, sizeof helper_sig);
  size_t marker_pos   = find_sig(file, file_len, raw_marker, 16);

  CHECK(myfunc_pos != (size_t)-1 && func2_pos != (size_t)-1 &&
        callthru_pos != (size_t)-1 && helper2_pos != (size_t)-1 &&
        helper_pos != (size_t)-1, "hook/helper signatures found");
  CHECK(marker_pos != (size_t)-1, "guest-owned raw-code marker found");
  if (myfunc_pos == (size_t)-1 || func2_pos == (size_t)-1 ||
      callthru_pos == (size_t)-1 || helper2_pos == (size_t)-1 ||
      helper_pos == (size_t)-1 || marker_pos == (size_t)-1) {
    host_disconnect(ctx); free(file); return 1;
  }

  size_t scratch_pos = marker_pos + 16;
  CHECK(scratch_pos + RAW_SCRATCH_SIZE <= file_len,
        "raw-code reservation lies inside guest image");
  uint32_t scratch_phys = com_phys + 0x100u + (uint32_t)scratch_pos;
  CHECK((scratch_phys & 0x0fu) == 0,
        "raw-code reservation paragraph aligned (%06x)", scratch_phys);
  CHECK(host_reserve_raw_code(ctx, scratch_phys, RAW_SCRATCH_SIZE) == 0,
        "registered 8 KiB guest-owned raw-code reservation");
  CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");

  uint16_t hook_rel_seg = (uint16_t)(com_seg - CODE_LOAD);
  uint16_t myfunc_off   = (uint16_t)(0x100 + myfunc_pos);
  uint16_t func2_off    = (uint16_t)(0x100 + func2_pos);
  uint16_t callthru_off = (uint16_t)(0x100 + callthru_pos);

  HYDRA_REGISTER_ADDR(h_test_hook,  hook_rel_seg, myfunc_off, 0);
  HYDRA_REGISTER_ADDR(h_test_hook2, hook_rel_seg, func2_off, 0);
  HYDRA_REGISTER_ADDR(h_callthru,   hook_rel_seg, callthru_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 3, "3 static hooks registered");

  g_hookcnt_phys = com_phys + HOOKCNT_OFF;
  g_res4_phys = com_phys + RES4_OFF;
  g_call_rel_seg = hook_rel_seg;
  g_helper_off = (uint16_t)(0x100 + helper_pos);
  g_helper2_off = (uint16_t)(0x100 + helper2_pos);

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

  uint16_t hookcnt  = lowmem_read16(ctx->lm, g_hookcnt_phys);
  uint16_t result   = lowmem_read16(ctx->lm, com_phys + RESULT_OFF);
  uint16_t result2  = lowmem_read16(ctx->lm, com_phys + RESULT2_OFF);
  uint16_t res3     = lowmem_read16(ctx->lm, com_phys + RES3_OFF);
  uint16_t res4     = lowmem_read16(ctx->lm, com_phys + RES4_OFF);
  uint16_t flagres  = lowmem_read16(ctx->lm, com_phys + FLAGRES_OFF);
  uint16_t ifflagres = lowmem_read16(ctx->lm, com_phys + IFFLAGRES_OFF);

  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
  CHECK(hookcnt >= target, "guest hook counter >= %lu (got %u)",
        (unsigned long)target, hookcnt);
  CHECK((result & 0xFF00u) == 0xBE00u, "hook1 result observed (%04x)", result);
  CHECK(result2 == 0xCAFE, "hook2 result observed (%04x)", result2);
  CHECK(res4 == 0x7ACE, "native->guest helper2 returned (%04x)", res4);
  CHECK(res3 == 0xCAFE,
        "nested hook fired during guest callthrough (%04x)", res3);
  CHECK((flagres & 0x0001u) != 0,
        "CF preserved across hook roundtrip (FLAGS=%04x)", flagres);
  CHECK((ifflagres & 0x0200u) == 0,
        "IF=0 preserved across complete hook roundtrip (FLAGS=%04x)", ifflagres);
  CHECK(stats.raw_code_returns == stats.raw_code_runs,
        "every raw-code/guest call returned (%lu/%lu)",
        (unsigned long)stats.raw_code_returns,
        (unsigned long)stats.raw_code_runs);

  /* Four complete iterations precede the fifth hook1 stop. Each complete
   * iteration now dispatches hook1 + two direct hook2 calls + hook3 + nested
   * hook2 = 5 hooks. */
  {
    uint64_t want_dispatches = 1 + 5 * (target - 1); /* 21 for target=5 */
    uint64_t want_raws = 5 * target + 5 * (target - 1); /* 45 */
    CHECK(stats.hook_dispatches == want_dispatches,
          "hook dispatches == %lu (got %lu)",
          (unsigned long)want_dispatches,
          (unsigned long)stats.hook_dispatches);
    CHECK(stats.raw_code_runs == want_raws,
          "raw/guest-call runs == %lu (got %lu)",
          (unsigned long)want_raws,
          (unsigned long)stats.raw_code_runs);
    CHECK(stats.redirects == stats.raw_code_runs + stats.hook_dispatches,
          "redirect accounting consistent");
  }

  int scratch_ok = 1;
  for (size_t i = 0; i < RAW_SCRATCH_SIZE; i++) {
    if (lowmem_read8(ctx->lm, scratch_phys + (uint32_t)i) != file[scratch_pos + i]) {
      scratch_ok = 0;
      break;
    }
  }
  CHECK(scratch_ok, "raw-code reservation restored byte-for-byte");
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}
