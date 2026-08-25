/*
 * test_driver_ovl.c - Phase 7 Item E integration test: overlay support
 * (VROOMM-style page-in stub + decompiled hook takeover).
 *
 * Drives a real dosemu2 instance running testprog_ovl.com:
 *   1. waits until the guest program is loaded (CS:0100 matches the .COM),
 *   2. registers two Hydra hooks located by signature scan:
 *        - h_myfunc at hooked_fn (regular hook, RETURN_NEAR),
 *        - h_ovlhook at the int-3f overlay stub (OVERLAY flag, RETURN_FAR,
 *          AX=0xBEEF - distinguishable from the native body's 0x0A77),
 *   3. releases the guest and host_run()s it until the guest-side overlay
 *      call counter reaches the target,
 *   4. verifies the full page-in cycle (pure-lazy arming):
 *        - call #1 into the stub is fully NATIVE: the guest's own fake
 *          VROOMM pager copies the body to OVSEG:0 and patches the stub to
 *          "jmp far"; the first overlay result is the native body's 0x0A77
 *          (no breakpoint is ever planted on the unpaged stub - a bp that
 *          fired at an address cannot safely be re-executed natively there,
 *          simx86 keeps serving its stale translation),
 *        - at the next regular-hook stop the driver lazily arms: registers
 *          OVSEG with the core (hydra_overlay_segment_set) and plants the
 *          stub's one and only breakpoint,
 *        - subsequent stub calls dispatch through run_begin()'s redirect:
 *          the decompiled h_ovlhook runs (AX=0xBEEF) and RETURN_FAR pops
 *          the far-call frame -> results 2..N are 0xBEEF.
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

/* testprog_ovl.asm layout (org 100h) */
#define GOFLAG_OFF   0x102
#define HOOKRES_OFF  0x103
#define OVLCNT_OFF   0x105
#define OVLRES_OFF   0x107     /* 8-word result ring */
#define CODE_LOAD    0x0080

#define RAW_CODE_LINEAR 0xF000

/* The synthetic overlay segment chosen by the guest program (linear
 * 0x30000): away from the image (< com_seg<<4+file_len, checked below) and
 * from the raw-code region (linear 0xF0000..0xF2000). */
#define OVSEG_EXPECTED 0x3000

/* Unique signatures in testprog_ovl.asm:
 *   hooked_fn: mov ax,0x1111; ret
 *   STUB:      CD 3F <dw BODY_OFF=0> 90 90 90 */
static const uint8_t myfunc_sig[] = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t stub_sig[]   = { 0xCD, 0x3F, 0x00, 0x00, 0x90, 0x90, 0x90 };

/* Number of overlay calls the run covers (ring slots 0..3 used). */
#define TARGET_OVLCALLS 4

/* The callback stop fires right after a stub DISPATCH - the guest has not
 * resumed yet, so the result of that call is not stored yet. Run ONE extra
 * iteration so all TARGET_OVLCALLS ring slots are committed at exit. */
#define RUN_TARGET (TARGET_OVLCALLS + 1)

static uint32_t g_ovlcnt_phys;
static uint16_t g_hookedfn_off;
static uint16_t g_stub_off;
static int      g_fails;

/* Dispatch counters incremented inside the hooks (the exec threads are
 * joined before host_run returns, so plain reads after it are exact). */
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

/* ------------------------------------------------------------------ */
/* the hooks                                                          */
/* ------------------------------------------------------------------ */

HYDRA_FUNC(H_myfunc)
{
  g_myfunc_runs++;
  m->registers->ax = 0x600D;
  RETURN_NEAR();
}

/* The overlay hook: runs INSTEAD of the paged body once the core redirects
 * into the overlay address space. RETURN_FAR completes the guest's far-call
 * frame pushed by `call far [stubptr]`. */
HYDRA_FUNC(h_ovlhook)
{
  g_ovlhook_runs++;
  m->registers->ax = 0xBEEF;
  RETURN_FAR();
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

static int stop_when_ovlcount(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                              const host_run_stats_t *stats, void *user)
{
  (void)regs;
  (void)stats;
  size_t target = *(size_t *)user;
  return lowmem_read16(ctx->lm, g_ovlcnt_phys) >= target;
}

/* Find a byte pattern's org-100h offset within the .COM file. */
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
  /* line-buffered: keep every check visible even if we die mid-run */
  setvbuf(stdout, NULL, _IOLBF, 0);
  /* a dead emulator must surface as dosdebug errors (clean FAIL unwind),
   * not as a SIGPIPE instant kill mid-diagnosis */
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

  /* overlays=armed: OPTION E opt-in. Without it host_run() would reject
   * the overlay hooks exactly as #34 shipped (default contract). */
  char conf[512];
  snprintf(conf, sizeof(conf),
            "dosemu|pid=%ld|code_load=0x80|data_seg=0x0"
            "|overlays=armed",
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
  printf("guest CS=%04x (linear %06x), raw-code linear %05x, "
         "overlay segment %04x\n",
         com_seg, com_phys, RAW_CODE_LINEAR, OVSEG_EXPECTED);

  {
    uint32_t image_top = com_phys + (uint32_t)file_len;
    CHECK(image_top < RAW_CODE_LINEAR && OVSEG_EXPECTED > 0xF20u,
          "image top %06x below raw-code region; overlay linear %06x "
          "clash-free", image_top, (unsigned)OVSEG_EXPECTED << 4);
  }

  /* Hardened host: raw-code execution requires an explicit guest-owned
   * reservation (16-byte aligned, segment >= code_load_offset, >= one
   * 128-byte slot). RAW_CODE_LINEAR sits above the COM image and below
   * the synthetic overlay segment. */
  CHECK(host_reserve_raw_code(ctx, RAW_CODE_LINEAR, 8192) == 0,
        "raw-code reservation registered (linear %x)", RAW_CODE_LINEAR);
  CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");

  /* The real load segment is only known now: adopt it as CODE_START_SEG so
   * every image-relative hook address is interpreted against the segment
   * the guest actually runs in (same pattern as the other drivers). */
  host_set_code_load(ctx, com_seg);

  /* 2. locate the hooked function and the overlay stub by signature */
  size_t fn_pos   = find_sig(file, file_len, myfunc_sig, sizeof myfunc_sig);
  size_t stub_pos = find_sig(file, file_len, stub_sig, sizeof stub_sig);
  CHECK(fn_pos != (size_t)-1 && stub_pos != (size_t)-1,
        "signatures found (hooked_fn, overlay stub)");
  if (fn_pos == (size_t)-1 || stub_pos == (size_t)-1) {
    host_disconnect(ctx); free(file); return 1;
  }

  g_hookedfn_off = (uint16_t)(0x100 + fn_pos);
  g_stub_off     = (uint16_t)(0x100 + stub_pos);
  printf("hooks: hooked_fn %04x:%04x (near), stub %04x:%04x (overlay)\n",
         com_seg, g_hookedfn_off, com_seg, g_stub_off);

  /* 3. register the hooks: regular near-return hook + OVERLAY-flagged
   * far-return hook on the page-in stub. Only the regular hook gets a
   * static breakpoint (host_hook_breakpoint_count); the stub is armed
   * lazily once paged in. */
  HYDRA_REGISTER_ADDR(H_myfunc, 0, g_hookedfn_off, 0);
  HYDRA_REGISTER_ADDR(h_ovlhook, 0, g_stub_off, HYDRA_HOOK_FLAGS_OVERLAY);
  CHECK(host_hook_breakpoint_count(ctx) == 1, "1 static hook bp; overlay "
        "stub lazy-armed later (breakpoint count = %d)",
        host_hook_breakpoint_count(ctx));

  g_ovlcnt_phys = com_phys + OVLCNT_OFF;

  /* 4. release the guest (host_run plants/clears breakpoints itself) */
  lowmem_write8(ctx->lm, com_phys + GOFLAG_OFF, 1);
  printf("go flag set; running the guest...\n");

  /* 5. run the driver loop until the guest overlay counter reaches target */
  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = RUN_TARGET;
  opts.timeout_ms = 3000;
  opts.stop_fn = stop_when_ovlcount;
  opts.stop_user = &target;
  opts.verbose = 1;

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);

  printf("host_run ended: %s\n", reason_name(reason));
  {
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
  uint16_t ovlres[TARGET_OVLCALLS];
  for (int i = 0; i < TARGET_OVLCALLS; i++)
    ovlres[i] = lowmem_read16(ctx->lm,
                              com_phys + OVLRES_OFF + 2 * (uint32_t)i);
  uint16_t ovlcnt  = lowmem_read16(ctx->lm, g_ovlcnt_phys);
  uint16_t hookres = lowmem_read16(ctx->lm, com_phys + HOOKRES_OFF);

  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
  CHECK(ovlcnt >= RUN_TARGET,
        "guest overlay-call counter >= %d (got %u)",
        RUN_TARGET, ovlcnt);

  /* (1) FIRST overlay result comes from the NATIVE paged-in body (page-in
   * ran exactly once); results 2..N come from the decompiled hook. */
  CHECK(ovlres[0] == 0x0A77,
        "first overlay result is the native body's AX (ovlres[0]=%04x)",
        ovlres[0]);
  CHECK(ovlres[1] == 0xBEEF && ovlres[2] == 0xBEEF && ovlres[3] == 0xBEEF,
        "overlay results 2..4 took the decompiled hook path "
        "(%04x %04x %04x)", ovlres[1], ovlres[2], ovlres[3]);
  CHECK(hookres == 0x600D,
        "regular hook observed by the guest (hookres=%04x)", hookres);

  /* (2) the core got fed the overlay segment mapping. */
  CHECK(hydra_overlay_segment_lookup(0) == OVSEG_EXPECTED,
        "hydra_overlay_segment_lookup(0) == %04x (got %04x)",
        OVSEG_EXPECTED, hydra_overlay_segment_lookup(0));

  /* (3) dispatch accounting: (RUN_TARGET) myfunc + (RUN_TARGET-1) stub EA
   * redirects; the unpaged stub itself is never intercepted, so there is
   * no RESUME dispatch - the page-in is observed through ovlres[0]. */
  CHECK(stats.hook_dispatches == 2 * (uint64_t)RUN_TARGET - 1,
        "myfunc+EA dispatches == %d (got %lu)",
        2 * RUN_TARGET - 1, (unsigned long)stats.hook_dispatches);
  CHECK(g_myfunc_runs == RUN_TARGET && g_ovlhook_runs >= TARGET_OVLCALLS,
        "hook-side counters: myfunc=%u ovlhook=%u (>=%d EA redirects)",
        g_myfunc_runs, g_ovlhook_runs, TARGET_OVLCALLS);

  /* (4) raw/returns consistent (no raw-code requests in this workload). */
  CHECK(stats.raw_code_runs == stats.raw_code_returns,
        "every raw code / guest call returned via trace (raw=%lu ret=%lu)",
        (unsigned long)stats.raw_code_runs,
        (unsigned long)stats.raw_code_returns);

  /* (5) dosemu alive */
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}
