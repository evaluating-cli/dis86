/* Focused live MZ/FBOV overlay dispatch smoke test. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"
#include "addr.h"
#include "overlay.h"

#ifndef TESTPROG_MZ_OVL_PATH
#define TESTPROG_MZ_OVL_PATH "/tmp/opencode/TESTPROG.EXE"
#endif
#ifndef TEST_MZ_OVL_USERLIB_PATH
#define TEST_MZ_OVL_USERLIB_PATH "/tmp/test_mz_overlay_user.so"
#endif

#define BDA_SEG 0x40
#define LAUNCH_GO 0xF4
#define LAUNCH_UP 0xF5
#define LAUNCH_STAT 0xF6
#define LAUNCH_PSP 0xF8
#define CALL_COUNT_OFF 37
#define LAST_RESULT_OFF 39
#define FIRST_RESULT_OFF 41
#define STUB_OFF 0x20
#define OVSEG_EXPECTED 0x3000
#define TARGET_CALLS 4

static int failures;
static unsigned overlay_runs;

#define CHECK(c, ...) do { \
  if (c) printf("  ok:   "); else { printf("  FAIL: "); failures++; } \
  printf(__VA_ARGS__); printf("\n"); fflush(stdout); \
} while (0)

HYDRA_FUNC(h_mz_ovlhook)
{
  overlay_runs++;
  m->registers->ax = 0xBEEF;
  RETURN_FAR();
}

static int stop_after_calls(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                            const host_run_stats_t *stats, void *user)
{
  (void)regs; (void)stats; (void)user;
  uint32_t base = (uint32_t)ctx->code_load_offset << 4;
  return lowmem_read16(ctx->lm, base + CALL_COUNT_OFF) >= TARGET_CALLS;
}

static int read_mz(const char *path, uint8_t *head, size_t n)
{
  FILE *f = fopen(path, "rb");
  if (!f) return -1;
  int ok = fread(head, 1, n, f) == n;
  fclose(f);
  return ok ? 0 : -1;
}

static int has_fbov_record(const char *path, const uint8_t head[0x20])
{
  uint16_t cblp = (uint16_t)(head[2] | (head[3] << 8));
  uint16_t cp = (uint16_t)(head[4] | (head[5] << 8));
  uint32_t mz_size = cblp ? ((uint32_t)(cp - 1) * 512u + cblp)
                          : ((uint32_t)cp * 512u);
  uint8_t fbov[16] = {0}, seginfo[8] = {0}, overlay[4] = {0};
  FILE *f = fopen(path, "rb");
  if (!f) return 0;
  int ok = fseek(f, (long)mz_size, SEEK_SET) == 0 &&
           fread(fbov, 1, sizeof(fbov), f) == sizeof(fbov);
  uint32_t exeinfo = (uint32_t)fbov[8] | ((uint32_t)fbov[9] << 8) |
                     ((uint32_t)fbov[10] << 16) | ((uint32_t)fbov[11] << 24);
  ok = ok && fseek(f, (long)exeinfo, SEEK_SET) == 0 &&
       fread(seginfo, 1, sizeof(seginfo), f) == sizeof(seginfo) &&
       fseek(f, (long)mz_size + sizeof(fbov), SEEK_SET) == 0 &&
       fread(overlay, 1, sizeof(overlay), f) == sizeof(overlay);
  fclose(f);
  return ok && memcmp(fbov, "FBOV", 4) == 0 &&
         fbov[4] == 4 && fbov[5] == 0 && fbov[6] == 0 && fbov[7] == 0 &&
         seginfo[0] == 0 && seginfo[1] == 0 &&
         seginfo[2] == 37 && seginfo[3] == 0 &&
         seginfo[4] == 3 && seginfo[5] == 0 &&
         memcmp(overlay, "\xb8\x77\x0a\xcb", 4) == 0;
}

int main(int argc, char **argv)
{
  pid_t pid = argc > 1 ? (pid_t)strtol(argv[1], NULL, 0) : 0;
  const char *exe = argc > 2 ? argv[2] : TESTPROG_MZ_OVL_PATH;
  uint8_t head[0x20] = {0};
  CHECK(read_mz(exe, head, sizeof(head)) == 0 && head[0] == 'M' && head[1] == 'Z',
        "synthetic guest is an MZ executable");
  CHECK(has_fbov_record(exe, head), "MZ carries the expected FBOV/STUB record");
  if (failures) return 2;

  uint16_t hdr = (uint16_t)((head[8] | (head[9] << 8)) * 16);
  uint16_t e_ip = (uint16_t)(head[0x14] | (head[0x15] << 8));
  uint16_t e_cs = (uint16_t)(head[0x16] | (head[0x17] << 8));
  uint16_t e_ss = (uint16_t)(head[0x0e] | (head[0x0f] << 8));
  uint16_t e_sp = (uint16_t)(head[0x10] | (head[0x11] << 8));
  CHECK(hdr == 0x20 && e_cs == 0 && e_ip >= 47 && e_ip < 0x100,
        "MZ entry %04x:%04x and header size %u", e_cs, e_ip, hdr);

  char conf[2048];
  snprintf(conf, sizeof(conf),
           "dosemu|pid=%ld|code_load=0|data_seg=0|overlays=armed|lib=%s", (long)pid,
           TEST_MZ_OVL_USERLIB_PATH);
  hydra_machine_t machine = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(machine.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)machine.hardware->ctx;

  addr_t stub, body;
  CHECK(hydra_function_addr("F_mz_ovlhook", &stub) &&
        !addr_is_overlay(stub) && addr_seg(stub) == 0 && addr_off(stub) == STUB_OFF,
        "generated metadata resolves the MZ stub at 0000:0020");
  CHECK(hydra_function_addr("F_mz_ovlhook_OVERLAY", &body) &&
        addr_is_overlay(body) && addr_overlay_num(body) == 0 && addr_off(body) == 0,
        "generated metadata resolves body at overlay_0000:0000");
  hydra_impl_register("F_mz_ovlhook", h_mz_ovlhook, HYDRA_HOOK_FLAGS_OVERLAY);

  if (dosdebug_go(ctx->db) != 0) {
    printf("FAIL: cannot start dosemu2\n"); host_disconnect(ctx); return 1;
  }
  uint32_t bda = (uint32_t)BDA_SEG << 4;
  int up = 0;
  for (int i = 0; i < 600 && !up; i++) {
    usleep(100000);
    up = lowmem_read8(ctx->lm, bda + LAUNCH_UP) == 0xA5;
  }
  CHECK(up, "launch.com reached its resident wait point");
  if (!up) { host_disconnect(ctx); return 1; }

  dosdebug_regs_t parked;
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot park launcher\n"); host_disconnect(ctx); return 1;
  }
  lowmem_write8(ctx->lm, bda + LAUNCH_GO, 1);
  if (dosdebug_go(ctx->db) != 0) {
    printf("FAIL: cannot release launcher\n"); host_disconnect(ctx); return 1;
  }
  uint16_t psp = 0;
  int loaded = 0;
  for (int i = 0; i < 100 && !loaded; i++) {
    usleep(100000);
    if (lowmem_read8(ctx->lm, bda + LAUNCH_STAT) == 0x5A) {
      psp = lowmem_read16(ctx->lm, bda + LAUNCH_PSP); loaded = 1;
    }
  }
  CHECK(loaded, "launcher loaded MZ guest and published PSP");
  if (!loaded) { host_disconnect(ctx); return 1; }
  CHECK(host_reserve_raw_code(ctx, 0xF000, 8192) == 0 &&
        host_raw_code_ready(ctx), "raw-code reservation registered");
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot park loaded launcher\n"); host_disconnect(ctx); return 1;
  }
  uint16_t load_seg = (uint16_t)(psp + 0x10);
  uint32_t load_phys = (uint32_t)load_seg << 4;
  CHECK(lowmem_read8(ctx->lm, load_phys) == 0xCD &&
        lowmem_read8(ctx->lm, load_phys + 1) == 0x3F &&
        lowmem_read8(ctx->lm, load_phys + STUB_OFF) == 0xCD &&
        lowmem_read8(ctx->lm, load_phys + STUB_OFF + 1) == 0x3F,
        "MZ load preserves overlay header and annotated CD 3F stub");
  dosdebug_regs_t entry = {0};
  entry.cs = (uint16_t)(load_seg + e_cs); entry.ip = e_ip;
  entry.ds = psp; entry.es = psp;
  entry.ss = (uint16_t)(load_seg + e_ss); entry.sp = e_sp;
  entry.flags = 0x3202;
  if (host_set_regs(ctx, &entry) != 0) {
    printf("FAIL: cannot set MZ entry state\n"); host_disconnect(ctx); return 1;
  }

  host_run_options_t opts = {0};
  host_run_stats_t stats;
  opts.timeout_ms = 3000; opts.stop_fn = stop_after_calls; opts.verbose = 1;
  opts.mz_load = 1; opts.mz_parked_at_entry = 1;
  opts.mz_entry_cs = e_cs; opts.mz_entry_ip = e_ip;
  opts.mz_timeout_ms = 60000;
  host_run_stop_reason_t reason = host_run(ctx, &machine, &opts, &stats);
  uint32_t base = (uint32_t)(stats.mz_psp + 0x10) << 4;
  uint16_t count = lowmem_read16(ctx->lm, base + CALL_COUNT_OFF);
  uint16_t first = lowmem_read16(ctx->lm, base + FIRST_RESULT_OFF);
  uint16_t last = lowmem_read16(ctx->lm, base + LAST_RESULT_OFF);
  CHECK(reason == HOST_RUN_STOP_CALLBACK, "host run stopped on smoke callback");
  CHECK(count >= TARGET_CALLS, "guest completed at least %u calls (got %u)", TARGET_CALLS, count);
  CHECK(first == 0x0A77, "first call paged and ran native overlay body (%04x)", first);
  CHECK(last == 0xBEEF && overlay_runs >= 2,
        "later call dispatched generated overlay hook (result=%04x runs=%u)", last, overlay_runs);
  CHECK(lowmem_read8(ctx->lm, base + STUB_OFF) == 0xEA,
        "guest page-in patched MZ stub to far jump");
  CHECK(hydra_overlay_segment_lookup(0) == OVSEG_EXPECTED,
        "logical overlay 0 mapped to guest segment %04x", hydra_overlay_segment_lookup(0));
  CHECK(stats.hook_dispatches >= 2, "overlay dispatches observed by host (%llu)",
        (unsigned long long)stats.hook_dispatches);

  host_disconnect(ctx);
  return failures ? 1 : 0;
}
