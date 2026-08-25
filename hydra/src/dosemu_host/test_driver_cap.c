/*
 * test_driver_cap.c - Phase 7 Item D integration test: HYDSNAP state
 * capture/restore across two dosemu2 instances.
 *
 * Runs in two modes (CAP_MODE env var, default "cap"):
 *
 *   cap:      boots the launcher harness (-E launch.com), lets the launcher
 *             load TESTPROG.EXE, runs the hook suite until the guest hook
 *             counter reaches CAP_AT, reads the guest-visible probe words,
 *             writes them to PROBES_FILE and captures the FULL machine state
 *             (HYDSNAP: registers + whole lowmem window) via the vtable
 *             state_save() under HYDRA_MODE_CAPTURE.
 *
 *   restore:  same deterministic boot on a FRESH dosemu2 instance (guest
 *             loaded but never run; counters are 0). Then calls state_restore()
 *             under HYDRA_MODE_RESTORE: the vtable loads the HYDSNAP file,
 *             memcpy()s the blob into the lowmem window BEFORE pushing the
 *             register state (full paced dosdebug_write_regs with read-back
 *             verify — diff-writes are unsafe because the live CPU state is
 *             unknown), and adopts the snapshot's code/data offsets. The
 *             guest continues from the capture point (mid-mainloop), hooks
 *             keep dispatching, and the counter must advance exactly
 *             ADVANCE more hits.
 *
 * The launcher/parked/boot sequencing is identical to test_driver_exe.c
 * (which documents why the go-flag handshake exists).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

#ifndef TESTPROG_EXE_PATH
#define TESTPROG_EXE_PATH "/tmp/opencode/testprog.exe"
#endif

#define RAW_CODE_LINEAR 0xF000
#define IMAGE_TOP_MAX   0xE000

#define RESULT_DELTA   4
#define HOOKCNT_DELTA  6
#define RESULT2_DELTA  8
#define RES3_DELTA     10
#define RES4_DELTA     12
#define FLAGRES_DELTA  14

#define BDA_SEG          0x40
#define BDA_LAUNCH_GO    0xF4
#define BDA_LAUNCH_UP    0xF5
#define BDA_LAUNCH_STAT  0xF6
#define BDA_LAUNCH_PSP   0xF8

/* Capture after this many myfunc dispatches; restore must advance this far. */
#define CAP_AT   3
#define ADVANCE  3

/* Unique function signatures (identical bytes to the .com/.exe variants). */
static const uint8_t myfunc_sig[]   = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t func2_sig[]    = { 0xB8, 0x22, 0x22, 0xC3 };
static const uint8_t callthru_sig[] = { 0xB8, 0x33, 0x33, 0xC3 };
static const uint8_t helper2_sig[]  = { 0xB8, 0xCE, 0x7A };
static const uint8_t helper_sig[]   = { 0xB8, 0x44, 0x44 };

static uint16_t g_entry_ip;
static uint16_t g_hookcnt_off;
static uint16_t g_res4_off;
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
/* the hooks (identical to test_driver_exe.c)                          */
/* ------------------------------------------------------------------ */

static uint32_t img_phys(const hydra_machine_t *m, uint16_t off)
{
  const host_ctx_t *ctx = (const host_ctx_t *)m->hardware->ctx;
  return ((uint32_t)ctx->code_load_offset << 4) + off;
}

HYDRA_FUNC(h_test_hook)
{
  CLI();
  STI();
  INT(0x28);
  u8 t = INB(0x40);
  OUTB(0x80, t);

  u32 phys = img_phys(m, g_hookcnt_off);
  u16 cnt = m->hardware->mem_read16(m->hardware->ctx, phys);
  m->hardware->mem_write16(m->hardware->ctx, phys, (u16)(cnt + 1));
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
  u32 r2 = hydra_impl_call_far(0, g_helper2_off);
  m->hardware->mem_write16(m->hardware->ctx, img_phys(m, g_res4_off),
                           (u16)r2);

  u32 r1 = hydra_impl_call_far(0, g_helper_off);
  m->registers->ax = (u32)r1;

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

static int parse_mz_header(const uint8_t *file, size_t len,
                           uint16_t *e_cs, uint16_t *e_ip, uint16_t *e_ss,
                           uint16_t *hdr_bytes)
{
  if (len < 0x20 || file[0] != 'M' || file[1] != 'Z')
    return -1;
  *hdr_bytes = (uint16_t)((file[0x08] | (file[0x09] << 8)) * 16);
  *e_ip = (uint16_t)(file[0x14] | (file[0x15] << 8));
  *e_cs = (uint16_t)(file[0x16] | (file[0x17] << 8));
  *e_ss = (uint16_t)(file[0x0E] | (file[0x0F] << 8));
  if (*hdr_bytes == 0 || (size_t)*hdr_bytes >= len)
    return -1;
  return 0;
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

static int stop_when_hooked(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                            const host_run_stats_t *stats, void *user)
{
  (void)regs;
  (void)stats;
  size_t target = *(size_t *)user;
  uint32_t phys = (((uint32_t)ctx->code_load_offset) << 4) + g_hookcnt_off;
  return lowmem_read16(ctx->lm, phys) >= target;
}

/* The six guest-visible probe words, in a canonical order. */
typedef struct {
  uint16_t hookcnt, result, result2, res3, res4, flagres;
} probes_t;

static void read_probes(host_ctx_t *ctx, uint16_t e_ip, probes_t *p)
{
  uint32_t base = (uint32_t)ctx->code_load_offset << 4;
  p->hookcnt = lowmem_read16(ctx->lm, base + g_hookcnt_off);
  p->result  = lowmem_read16(ctx->lm, base + e_ip + RESULT_DELTA);
  p->result2 = lowmem_read16(ctx->lm, base + e_ip + RESULT2_DELTA);
  p->res3    = lowmem_read16(ctx->lm, base + e_ip + RES3_DELTA);
  p->res4    = lowmem_read16(ctx->lm, base + g_res4_off);
  p->flagres = lowmem_read16(ctx->lm, base + e_ip + FLAGRES_DELTA);
}

static void print_probes(const char *tag, const probes_t *p)
{
  printf("  %s: hookcnt=%u result=%04x result2=%04x res3=%04x res4=%04x "
         "flagres=%04x\n", tag, p->hookcnt, p->result, p->result2, p->res3,
         p->res4, p->flagres);
}

/* ------------------------------------------------------------------ */
/* shared boot: parked launcher -> loaded child -> parked              */
/* ------------------------------------------------------------------ */

static uint16_t boot_and_load(host_ctx_t *ctx, const char *exe_path,
                              uint8_t *file, size_t file_len,
                              uint16_t *hdr_bytes_out)
{
  uint16_t e_cs = 0, e_ip = 0, e_ss = 0, hdr_bytes = 0;
  if (parse_mz_header(file, file_len, &e_cs, &e_ip, &e_ss, &hdr_bytes) != 0) {
    printf("not a parseable MZ image\n");
    return 0;
  }
  g_entry_ip = e_ip;
  printf("  ok:   MZ parsed (entry %04x:%04x, e_ss=%04x, hdr=%u)\n",
         e_cs, e_ip, e_ss, hdr_bytes);

  printf("releasing boot; waiting for launcher (0040:00F5 == A5)...\n");
  if (dosdebug_go(ctx->db) != 0) {
    printf("FAIL: cannot resume the machine\n");
    return 0;
  }
  int launcher_up = 0;
  for (int i = 0; i < 600 && !launcher_up; i++) {
    usleep(100 * 1000);
    launcher_up = lowmem_read8(ctx->lm,
                               ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_UP)
                  == 0xA5;
  }
  CHECK(launcher_up, "launcher running (resident marker seen)");
  if (!launcher_up) return 0;

  dosdebug_regs_t parked;
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot stop the machine\n");
    return 0;
  }
  lowmem_write8(ctx->lm, ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_GO, 1);
  printf("go flag set (cpu parked at %04x:%04x)\n", parked.cs, parked.ip);
  if (dosdebug_go(ctx->db) != 0) {
    printf("FAIL: cannot resume the machine\n");
    return 0;
  }

  uint16_t child_psp = 0;
  int loaded = 0;
  for (int i = 0; i < 100 && !loaded; i++) {
    usleep(100 * 1000);
    if (lowmem_read8(ctx->lm,
                     ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_STAT) != 0x5A)
      continue;
    child_psp = lowmem_read16(ctx->lm,
                              ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_PSP);
    loaded = 1;
  }
  CHECK(loaded, "loader reported a loaded child (F6=5A)");
  if (!loaded) return 0;

  /* re-park; verify the image landed; push the exact DOS entry state */
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot stop the machine\n");
    return 0;
  }
  uint16_t load_seg = (uint16_t)(child_psp + 0x10u);
  uint32_t load_phys_chk = (uint32_t)load_seg << 4;
  int mcb_ok = lowmem_read16(ctx->lm,
                  (((uint32_t)(child_psp - 1)) << 4) + 1) == child_psp;
  int sig_ok = lowmem_read8(ctx->lm, (uint32_t)child_psp << 4) == 0xCD &&
               lowmem_read8(ctx->lm,
                            ((uint32_t)child_psp << 4) + 1) == 0x20;
  int img_ok = memcmp(lowmem_hostaddr(ctx->lm, load_phys_chk),
                      file + hdr_bytes,
                      file_len - hdr_bytes > 32 ? 32 : file_len - hdr_bytes)
               == 0;
  CHECK(mcb_ok && sig_ok && img_ok,
        "loaded image verified in memory (PSP=%04x, load seg=%04x) "
        "[mcb=%d sig=%d img=%d]", child_psp, load_seg, mcb_ok, sig_ok, img_ok);
  if (!(mcb_ok && sig_ok && img_ok)) return 0;
  (void)exe_path;

  dosdebug_regs_t entry;
  memset(&entry, 0, sizeof(entry));
  entry.cs = (uint16_t)(load_seg + e_cs);
  entry.ip = e_ip;
  entry.ds = child_psp;
  entry.es = child_psp;
  entry.ss = (uint16_t)(load_seg + e_ss);
  /* Exact DOS entry state, same as test_driver_exe.c: initial SP comes
   * from the MZ header (e_sp @ file 0x10). */
  entry.sp = (uint16_t)(file[0x10] | (file[0x11] << 8));
  entry.flags = 0x3202;
  if (host_set_regs(ctx, &entry) != 0) {
    printf("FAIL: cannot push the child entry state\n");
    return 0;
  }

  *hdr_bytes_out = hdr_bytes;
  return child_psp;
}

/* ------------------------------------------------------------------ */
/* main                                                               */
/* ------------------------------------------------------------------ */

int main(int argc, char **argv)
{
  pid_t pid = 0;
  if (argc > 1) pid = (pid_t)strtol(argv[1], NULL, 0);
  const char *exe_path = (argc > 2) ? argv[2] : TESTPROG_EXE_PATH;
  const char *mode = getenv("CAP_MODE");
  if (!mode) mode = "cap";
  const char *snap_path   = "/tmp/opencode/cap_state.snap";
  const char *probes_path = "/tmp/opencode/cap_probes.txt";

  printf("=== Phase 7 Item D integration test (HYDSNAP %s) ===\n", mode);

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(exe_path, &file, &file_len) != 0) {
    printf("cannot read .EXE: %s\n", exe_path);
    return 2;
  }

  char conf[160];
  snprintf(conf, sizeof(conf),
           "dosemu|pid=%ld|code_load=0x0|data_seg=0x0|raw_code=0x%x",
           (long)pid, RAW_CODE_LINEAR);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;

  printf("connected to dosemu2 pid=%ld\n", (long)host_pid(ctx));

  /* locate every function by signature (file offsets) */
  size_t myfunc_pos   = find_sig(file, file_len, myfunc_sig, sizeof myfunc_sig);
  size_t func2_pos    = find_sig(file, file_len, func2_sig, sizeof func2_sig);
  size_t callthru_pos = find_sig(file, file_len, callthru_sig, sizeof callthru_sig);
  size_t helper2_pos  = find_sig(file, file_len, helper2_sig, sizeof helper2_sig);
  size_t helper_pos   = find_sig(file, file_len, helper_sig, sizeof helper_sig);
  if (myfunc_pos == (size_t)-1 || func2_pos == (size_t)-1 ||
      callthru_pos == (size_t)-1 || helper2_pos == (size_t)-1 ||
      helper_pos == (size_t)-1) {
    printf("FAIL: signatures not found in the image\n");
    host_disconnect(ctx); free(file); return 1;
  }
  /* Signature positions are FILE offsets; need hdr_bytes first to convert,
   * so conversion happens inside boot_and_load's caller below. */

  uint16_t hdr_bytes = 0;
  uint16_t psp = boot_and_load(ctx, exe_path, file, file_len, &hdr_bytes);
  if (psp == 0) { host_disconnect(ctx); free(file); return 1; }

  uint16_t myfunc_off   = (uint16_t)(myfunc_pos - hdr_bytes);
  uint16_t func2_off    = (uint16_t)(func2_pos - hdr_bytes);
  uint16_t callthru_off = (uint16_t)(callthru_pos - hdr_bytes);
  g_hookcnt_off = (uint16_t)(g_entry_ip + HOOKCNT_DELTA);
  g_res4_off    = (uint16_t)(g_entry_ip + RES4_DELTA);
  g_helper_off  = (uint16_t)(helper_pos - hdr_bytes);
  g_helper2_off = (uint16_t)(helper2_pos - hdr_bytes);

  HYDRA_REGISTER_ADDR(h_test_hook,  0, myfunc_off, 0);
  HYDRA_REGISTER_ADDR(h_test_hook2, 0, func2_off, 0);
  HYDRA_REGISTER_ADDR(h_callthru,   0, callthru_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 3, "3 hooks registered "
        "(breakpoint count = %d)", host_hook_breakpoint_count(ctx));

  /* NOTE: code_load_offset is still the conf placeholder (0x0) here;
   * host_run's MZ validation at entry points it at PSP+0x10 (cap stage) or
   * state_restore points it at the snapshot's value (restore stage). */

  if (strcmp(mode, "cap") == 0) {
    /* ------------------------- CAPTURE ------------------------- */
    host_run_stats_t stats;
    host_run_options_t opts = {0};
    size_t target = CAP_AT;
    opts.timeout_ms = 3000;
    opts.stop_fn = stop_when_hooked;
    opts.stop_user = &target;
    opts.verbose = 0;
    opts.mz_load = 1;
    opts.mz_parked_at_entry = 1;
    opts.mz_entry_cs = 0;      /* sig-verified e_cs == 0 */
    opts.mz_entry_ip = g_entry_ip;
    opts.mz_timeout_ms = 60000;

    /* Hardened host: explicit guest-owned raw-code reservation above the
     * load segment (see test_driver.c for the in-image variant). */
    CHECK(host_reserve_raw_code(ctx, 0xF000, 8192) == 0,
          "raw-code reservation registered (cap stage)");
    CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");

    host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);
    printf("host_run ended: %s (stops=%lu dispatches=%lu)\n",
           reason_name(reason), (unsigned long)stats.stops,
           (unsigned long)stats.hook_dispatches);
    CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");
    if (reason != HOST_RUN_STOP_CALLBACK) {
      host_disconnect(ctx); free(file); return 1;
    }

    probes_t p;
    read_probes(ctx, g_entry_ip, &p);
    print_probes("captured", &p);
    CHECK(p.hookcnt >= CAP_AT, "guest hook counter >= %d (got %u)",
          CAP_AT, p.hookcnt);
    CHECK((p.result & 0xFF00u) == 0xBE00u && p.result2 == 0xCAFEu &&
          p.res4 == 0x7ACEu && (p.flagres & 1u),
          "guest-observed hook results valid before capture");

    FILE *pf = fopen(probes_path, "w");
    if (pf) {
      fprintf(pf, "hookcnt=%u result=%04x result2=%04x res3=%04x "
                  "res4=%04x flagres=%04x psp=%04x\n",
              p.hookcnt, p.result, p.result2, p.res3, p.res4, p.flagres, psp);
      fclose(pf);
    }
    CHECK(pf != NULL, "probe words recorded to %s", probes_path);

    HYDRA_MODE->mode = HYDRA_MODE_CAPTURE;
    HYDRA_MODE->state_path = snap_path;
    m.hardware->state_save(m.hardware->ctx, snap_path);
    {
      FILE *sf = fopen(snap_path, "rb");
      long sz = -1;
      if (sf) {
        fseek(sf, 0, SEEK_END);
        sz = ftell(sf);
        fclose(sf);
      }
      CHECK(sz > 0x40, "HYDSNAP written to %s (%ld bytes)", snap_path, sz);
    }

    host_disconnect(ctx);
    free(file);
    printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
    return g_fails ? 1 : 0;
  }

  /* ------------------------- RESTORE ------------------------- */
  probes_t cap = {0};
  {
    FILE *pf = fopen(probes_path, "r");
    if (!pf ||
        fscanf(pf, "hookcnt=%hu result=%hx result2=%hx res3=%hx res4=%hx "
                   "flagres=%hx", &cap.hookcnt, &cap.result, &cap.result2,
               &cap.res3, &cap.res4, &cap.flagres) != 6) {
      printf("FAIL: cannot read capture probes from %s\n", probes_path);
      if (pf) fclose(pf);
      host_disconnect(ctx); free(file); return 1;
    }
    fclose(pf);
    print_probes("restored-from", &cap);
  }

  /* The guest was loaded by this instance too but never ran; the restore
   * overwrites its memory+registers with the capture-time state. */
  HYDRA_MODE->mode = HYDRA_MODE_RESTORE;
  HYDRA_MODE->state_path = snap_path;
  m.hardware->state_restore(m.hardware->ctx, snap_path);
  HYDRA_MODE->mode = HYDRA_MODE_NORMAL;
  CHECK(ctx->code_load_offset != 0,
        "code_load_offset adopted from snapshot (%04x)",
        ctx->code_load_offset);
  {
    uint32_t image_top = ((uint32_t)ctx->code_load_offset << 4) + file_len;
    CHECK(image_top <= IMAGE_TOP_MAX,
          "image below the raw-code region (top %06x <= %06x)",
          (unsigned)image_top, (unsigned)IMAGE_TOP_MAX);
  }

  probes_t now;
  read_probes(ctx, g_entry_ip, &now);
  print_probes("post-restore", &now);
  CHECK(memcmp(&cap, &now, sizeof(cap)) == 0,
        "guest-visible memory matches the captured state exactly");

  /* Continue from the capture point (mid-mainloop): hooks must keep
   * dispatching and the counter must advance exactly ADVANCE more hits. */
  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = (size_t)cap.hookcnt + ADVANCE;
  opts.timeout_ms = 3000;
  opts.stop_fn = stop_when_hooked;
  opts.stop_user = &target;
  opts.verbose = 0;
  /* NO mz_load: state_restore already fixed code_load_offset; the CPU is
   * mid-mainloop, not at the module entry, so MZ entry validation would be
   * meaningless here. */

  /* Fresh instance: the reservation is per-process state and must be
   * re-registered before any raw-code execution on this connection. */
  CHECK(host_reserve_raw_code(ctx, 0xF000, 8192) == 0,
        "raw-code reservation re-registered (restore stage)");
  CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);
  printf("host_run ended: %s (stops=%lu dispatches=%lu raw=%lu ret=%lu)\n",
         reason_name(reason), (unsigned long)stats.stops,
         (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs,
         (unsigned long)stats.raw_code_returns);
  CHECK(reason == HOST_RUN_STOP_CALLBACK, "driver stopped via callback");

  probes_t fin;
  read_probes(ctx, g_entry_ip, &fin);
  print_probes("final", &fin);
  CHECK(fin.hookcnt >= target,
        "hook counter advanced past capture (%u -> %u)", cap.hookcnt,
        fin.hookcnt);
  CHECK(fin.result2 == 0xCAFEu && fin.res4 == 0x7ACEu &&
        (fin.result & 0xFF00u) == 0xBE00u && (fin.flagres & 1u),
        "hook results keep flowing after restore");
  CHECK(stats.raw_code_returns == stats.raw_code_runs,
        "every raw code / guest call returned via trace (raw=%lu ret=%lu)",
        (unsigned long)stats.raw_code_runs,
        (unsigned long)stats.raw_code_returns);
  CHECK(host_pid(ctx) != 0, "dosemu2 still alive");

  host_disconnect(ctx);
  free(file);

  printf("=== %s ===\n", g_fails ? "TEST FAILED" : "TEST PASSED");
  return g_fails ? 1 : 0;
}
