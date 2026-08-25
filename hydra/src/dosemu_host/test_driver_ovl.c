/*
 * test_driver_ovl.c - Phase 7 Item E integration test: overlay support.
 *
 * This fixture intentionally removes the two crutches that previously made
 * lazy arming look reliable: there is no ordinary hook between repeated
 * overlay calls and the guest never rewrites the stub merely to invalidate
 * simx86 translations. The host must observe INT 3F page-in itself.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

#ifndef TESTPROG_OVL_PATH
#define TESTPROG_OVL_PATH "/tmp/opencode/testprog_ovl.com"
#endif

#define GOFLAG_OFF   0x102
#define HOOKRES_OFF  0x103
#define OVLCNT_OFF   0x105
#define OVLRES_OFF   0x107
#define CODE_LOAD    0x0080
#define RAW_CODE_LINEAR 0xF000
#define OVSEG_EXPECTED 0x3000
#define OVL_LOGICAL_NUM 3
#define BODY_OFF 0

static const uint8_t myfunc_sig[] = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t stub_sig[]   = { 0xCD, 0x3F, 0x00, 0x00, 0x90, 0x90, 0x90 };

#define TARGET_OVLCALLS 4
#define RUN_TARGET (TARGET_OVLCALLS + 1)

static uint32_t g_ovlcnt_phys;
static uint16_t g_hookedfn_off;
static uint16_t g_stub_off;
static int      g_fails;
static unsigned g_myfunc_runs;
static unsigned g_ovlhook_runs;

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

HYDRA_FUNC(H_myfunc)
{
  g_myfunc_runs++;
  m->registers->ax = 0x600D;
  RETURN_NEAR();
}

HYDRA_FUNC(h_ovlhook)
{
  g_ovlhook_runs++;
  m->registers->ax = 0xBEEF;
  RETURN_FAR();
}

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
    uint8_t m32[16];
    int got = dosdebug_read_mem(ctx->db, r.cs, 0x100, m32, (int)check_len);
    if (got == (int)check_len && memcmp(m32, file, check_len) == 0)
      return r.cs;
    if (go_sleep_stop(ctx, 250) != 0) return 0;
  }
  return 0;
}

static int stop_when_ovlcount(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                              const host_run_stats_t *stats, void *user)
{
  (void)regs;
  (void)stats;
  size_t target = *(size_t *)user;
  return lowmem_read16(ctx->lm, g_ovlcnt_phys) >= target;
}

static size_t find_sig(const uint8_t *file, size_t file_len,
                       const uint8_t *sig, size_t sig_len)
{
  for (size_t i = 0; i + sig_len <= file_len; i++)
    if (memcmp(file + i, sig, sig_len) == 0)
      return i;
  return (size_t)-1;
}

int main(int argc, char **argv)
{
  setvbuf(stdout, NULL, _IOLBF, 0);
  signal(SIGPIPE, SIG_IGN);

  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *com_path = (argc > 2) ? argv[2] : TESTPROG_OVL_PATH;

  printf("=== Phase 7 Item E integration test (overlay) ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(com_path, &file, &file_len) != 0) {
    printf("cannot read .COM: %s\n", com_path);
    return 2;
  }
  printf("guest program: %s (%zu bytes)\n", com_path, file_len);

  char conf[512];
  snprintf(conf, sizeof(conf),
           "dosemu|pid=%ld|code_load=0x80|data_seg=0x0|overlays=armed",
           (long)pid);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;
  printf("connected to dosemu2 pid=%ld\n", (long)host_pid(ctx));

  printf("waiting for the guest program to load...\n");
  uint16_t com_seg = wait_for_com(ctx, file, file_len);
  CHECK(com_seg != 0, "guest program loaded (segment %04x)", com_seg);
  if (com_seg == 0) { host_disconnect(ctx); free(file); return 1; }

  uint32_t com_phys = (uint32_t)com_seg << 4;
  printf("guest CS=%04x (linear %06x), raw-code linear %05x, overlay segment %04x\n",
         com_seg, com_phys, RAW_CODE_LINEAR, OVSEG_EXPECTED);

  {
    uint32_t image_top = com_phys + (uint32_t)file_len;
    CHECK(image_top < RAW_CODE_LINEAR && OVSEG_EXPECTED > 0xF20u,
          "image top %06x below raw-code region; overlay linear %06x clash-free",
          image_top, (unsigned)OVSEG_EXPECTED << 4);
  }

  CHECK(host_reserve_raw_code(ctx, RAW_CODE_LINEAR, 8192) == 0,
        "raw-code reservation registered (linear %x)", RAW_CODE_LINEAR);
  CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");
  host_set_code_load(ctx, com_seg);

  size_t fn_pos   = find_sig(file, file_len, myfunc_sig, sizeof myfunc_sig);
  size_t stub_pos = find_sig(file, file_len, stub_sig, sizeof stub_sig);
  CHECK(fn_pos != (size_t)-1 && stub_pos != (size_t)-1,
        "signatures found (hooked_fn, overlay stub)");
  if (fn_pos == (size_t)-1 || stub_pos == (size_t)-1) {
    host_disconnect(ctx); free(file); return 1;
  }

  g_hookedfn_off = (uint16_t)(0x100 + fn_pos);
  g_stub_off     = (uint16_t)(0x100 + stub_pos);
  printf("hooks: hooked_fn %04x:%04x, stub %04x:%04x -> overlay_%u:%04x\n",
         com_seg, g_hookedfn_off, com_seg, g_stub_off,
         OVL_LOGICAL_NUM, BODY_OFF);

  /* Production generator contract: F_ovlhook is the physical entry stub;
   * F_ovlhook_OVERLAY is the logical paged body. HYDRA_REGISTER(name) resolves
   * the former and carries the OVERLAY flag. Build the same metadata shape. */
  hydra_function_def_t defs[] = {
    { "F_ovlhook",         ADDR_MAKE(0, g_stub_off) },
    { "F_ovlhook_OVERLAY", ADDR_MAKE_EXT(1, OVL_LOGICAL_NUM, BODY_OFF) },
  };
  hydra_function_metadata_t md = { sizeof(defs) / sizeof(defs[0]), defs };
  CHECK(hydra_function_metadata_set(&md) == 0,
        "overlay metadata installed (logical overlay %u)", OVL_LOGICAL_NUM);

  HYDRA_REGISTER_ADDR(H_myfunc, 0, g_hookedfn_off, 0);
  hydra_impl_register("F_ovlhook", h_ovlhook, HYDRA_HOOK_FLAGS_OVERLAY);
  CHECK(host_hook_breakpoint_count(ctx) == 1,
        "1 static hook bp; overlay stub is dynamic (count=%d)",
        host_hook_breakpoint_count(ctx));

  uint32_t stub_phys = com_phys + g_stub_off;
  CHECK(lowmem_read8(ctx->lm, stub_phys) == 0xCD &&
        lowmem_read8(ctx->lm, stub_phys + 1) == 0x3F,
        "stub starts exactly CD 3F");

  /* Fail-closed regression: corrupt the unexecuted stub, invoke host_run, and
   * prove classification rejects it before the guest advances. Restore CD3F
   * afterwards and run the real scenario on the same connection. */
  {
    host_run_stats_t bad_stats = {0};
    uint8_t saved = lowmem_read8(ctx->lm, stub_phys);
    lowmem_write8(ctx->lm, stub_phys, 0x90);
    host_run_stop_reason_t bad = host_run(ctx, &m, NULL, &bad_stats);
    CHECK(bad == HOST_RUN_STOP_ERROR,
          "malformed stub fails closed before execution (reason=%s)",
          reason_name(bad));
    CHECK(bad_stats.stops == 0 && bad_stats.hook_dispatches == 0,
          "malformed stub rejection observes zero guest stops/dispatches");
    lowmem_write8(ctx->lm, stub_phys, saved);
    CHECK(lowmem_read8(ctx->lm, stub_phys) == 0xCD &&
          lowmem_read8(ctx->lm, stub_phys + 1) == 0x3F,
          "stub restored to exact CD 3F after negative probe");
  }

  g_ovlcnt_phys = com_phys + OVLCNT_OFF;
  lowmem_write8(ctx->lm, com_phys + GOFLAG_OFF, 1);
  printf("go flag set; running the guest...\n");

  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = RUN_TARGET;
  opts.timeout_ms = 3000;
  opts.stop_fn = stop_when_ovlcount;
  opts.stop_user = &target;
  opts.verbose = 1;

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);
  printf("host_run ended: %s\n", reason_name(reason));
  printf("  stops=%lu hook_dispatches=%lu raw_code_runs=%lu raw_code_returns=%lu redirects=%lu\n",
         (unsigned long)stats.stops, (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs, (unsigned long)stats.raw_code_returns,
         (unsigned long)stats.redirects);

  uint16_t ovlres[TARGET_OVLCALLS];
  for (int i = 0; i < TARGET_OVLCALLS; i++)
    ovlres[i] = lowmem_read16(ctx->lm,
                              com_phys + OVLRES_OFF + 2 * (uint32_t)i);
  uint16_t ovlcnt  = lowmem_read16(ctx->lm, g_ovlcnt_phys);
  uint16_t hookres = lowmem_read16(ctx->lm, com_phys + HOOKRES_OFF);

  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
  CHECK(ovlcnt >= RUN_TARGET,
        "guest overlay-call counter >= %d (got %u)", RUN_TARGET, ovlcnt);
  CHECK(ovlres[0] == 0x0A77,
        "first overlay body executed natively (ovlres[0]=%04x)", ovlres[0]);
  CHECK(ovlres[1] == 0xBEEF && ovlres[2] == 0xBEEF && ovlres[3] == 0xBEEF,
        "calls 2..4 dispatch decompiled hook (%04x %04x %04x)",
        ovlres[1], ovlres[2], ovlres[3]);
  CHECK(hookres == 0x600D,
        "one regular hook observed before overlay loop (hookres=%04x)", hookres);

  CHECK(hydra_overlay_segment_lookup(OVL_LOGICAL_NUM) == OVSEG_EXPECTED,
        "logical overlay %u maps to %04x (got %04x)",
        OVL_LOGICAL_NUM, OVSEG_EXPECTED,
        hydra_overlay_segment_lookup(OVL_LOGICAL_NUM));

  CHECK(g_myfunc_runs == 1,
        "no ordinary hook is used as a repeated overlay synchronization point (myfunc=%u)",
        g_myfunc_runs);
  CHECK(g_ovlhook_runs >= TARGET_OVLCALLS,
        "decompiled overlay hook handled later calls (runs=%u >= %d)",
        g_ovlhook_runs, TARGET_OVLCALLS);
  CHECK(stats.hook_dispatches >= 1 + TARGET_OVLCALLS,
        "dispatch accounting includes one static + later overlay hooks (got %lu)",
        (unsigned long)stats.hook_dispatches);

  /* host_run cleanup clears the debugger bp, so the shared mapping must expose
   * the pager's clean EA bytes again. No guest self-write was used to get here. */
  CHECK(lowmem_read8(ctx->lm, stub_phys) == 0xEA,
        "stub is clean EA after breakpoint cleanup (byte=%02x)",
        lowmem_read8(ctx->lm, stub_phys));

  CHECK(stats.raw_code_runs == stats.raw_code_returns,
        "every raw code / guest call returned via trace (raw=%lu ret=%lu)",
        (unsigned long)stats.raw_code_runs,
        (unsigned long)stats.raw_code_returns);
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}
