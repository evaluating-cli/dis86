/*
 * test_driver.c - Phase 4/5 integration test.
 *
 * Drives a real dosemu2 instance running testprog.com:
 *   1. waits until the guest program is loaded (CS:0100 matches the .COM),
 *   2. registers a Hydra hook at myfunc (located by its B8 11 11 C3
 *      signature in the loaded .COM),
 *   3. plants dosemu2 breakpoints at every registered hook,
 *   4. releases the guest (sets the go flag) and host_run()s it until the
 *      guest-side hook counter reaches the target,
 *   5. verifies that hooks were dispatched, guest opcodes (cli/sti/int/in/out)
 *      ran to completion through the return-stub mechanism, and the guest
 *      program observed the hook's result.
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
#define CODE_LOAD   0x0080

/* guest opcodes per hook dispatch: cli, sti, int 0x28, in 0x40, out 0x80 */
#define RAW_CODES_PER_HOOK 5u

static const uint8_t myfunc_sig[4] = { 0xB8, 0x11, 0x11, 0xC3 };

static uint32_t g_hookcnt_phys;
static uint32_t g_result_phys;
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
/* the hook                                                           */
/* ------------------------------------------------------------------ */

HYDRA_FUNC(h_test_hook)
{
  /* exercise the guest-opcode execution path */
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

/* ------------------------------------------------------------------ */
/* main                                                               */
/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *com_path = (argc > 2) ? argv[2] : TESTPROG_COM_PATH;

  printf("=== Phase 4/5 integration test ===\n");

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

  /* 2. locate myfunc by signature within the .COM file */
  size_t sig_off = (size_t)-1;
  for (size_t i = 0; i + sizeof(myfunc_sig) <= file_len; i++) {
    if (memcmp(file + i, myfunc_sig, sizeof(myfunc_sig)) == 0) {
      sig_off = i;
      break;
    }
  }
  CHECK(sig_off != (size_t)-1, "myfunc signature found at file offset %zu", sig_off);
  if (sig_off == (size_t)-1) { host_disconnect(ctx); free(file); return 1; }

  uint16_t myfunc_off = (uint16_t)(0x100 + sig_off);
  uint16_t hook_rel_seg = (uint16_t)(com_seg - CODE_LOAD);
  printf("hook target: myfunc @ %04x:%04x (rel seg %04x)\n",
         com_seg, myfunc_off, hook_rel_seg);

  /* 3. register the hook */
  HYDRA_REGISTER_ADDR(h_test_hook, hook_rel_seg, myfunc_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 1, "hook registered "
        "(breakpoint count = %d)", host_hook_breakpoint_count(ctx));

  g_hookcnt_phys = com_phys + HOOKCNT_OFF;
  g_result_phys  = com_phys + RESULT_OFF;

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
    /* diagnostics: where did the CPU end up? what's in the raw-code slots? */
    dosdebug_regs_t d;
    if (host_get_regs(ctx, &d) == 0)
      printf("  cpu now at %04x:%04x (sp=%04x ss=%04x)\n",
             d.cs, d.ip, d.sp, d.ss);
    uint8_t rc[16];
    if (dosdebug_read_mem(ctx->db, 0x1c0, 0x0000, rc, 16) > 0) {
      printf("  raw slot0 @1c0:0000:");
      for (int i = 0; i < 16; i++) printf(" %02x", rc[i]);
      printf("\n");
    }
    if (dosdebug_read_mem(ctx->db, 0xffff, 0x0000, rc, 8) > 0) {
      printf("  rom @ffff:0000:");
      for (int i = 0; i < 8; i++) printf(" %02x", rc[i]);
      printf("\n");
    }
  }
  printf("  stops=%lu hook_dispatches=%lu raw_code_runs=%lu "
         "stub_hits=%lu redirects=%lu\n",
         (unsigned long)stats.stops, (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs, (unsigned long)stats.stub_hits,
         (unsigned long)stats.redirects);

  /* 6. verify */
  uint16_t hookcnt = lowmem_read16(ctx->lm, g_hookcnt_phys);
  uint16_t result  = lowmem_read16(ctx->lm, g_result_phys);

  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
  CHECK(hookcnt >= target, "guest hook counter >= %lu (got %u)", 
        (unsigned long)target, hookcnt);
  CHECK((result & 0xFF00u) == 0xBE00u,
        "guest observed the hook result (result=%04x)", result);
  CHECK(stats.hook_dispatches >= target, "hook dispatches >= %lu (got %lu)",
        (unsigned long)target, (unsigned long)stats.hook_dispatches);
  CHECK(stats.raw_code_runs >= (uint64_t)target * RAW_CODES_PER_HOOK,
        "raw-code runs >= %lu (got %lu)",
        (unsigned long)(target * RAW_CODES_PER_HOOK),
        (unsigned long)stats.raw_code_runs);
  CHECK(stats.stub_hits == stats.raw_code_runs,
        "every raw code returned via a stub (raw=%lu stub=%lu)",
        (unsigned long)stats.raw_code_runs, (unsigned long)stats.stub_hits);
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  /* 7. read back one hook's I/O byte to prove the INB executed */
  printf("hook result byte = %02x (from guest in 0x40)\n", (unsigned)(result & 0xff));

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}