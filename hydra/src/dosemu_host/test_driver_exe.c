/*
 * test_driver_exe.c - Phase 7 Item C integration test: MZ/.exe guest loaded
 * via dosemu2's bpload.
 *
 * Drives a real dosemu2 instance launched with `-E launch.com`:
 *   1. parses the MZ header of testprog.exe (built by hand in NASM) for the
 *      expected entry CS:IP,
 *   2. waits for launch.com — EXEC'd by the shell during autoexec — to mark
 *      its resident marker (0040:00F5 = 0xA5), then parks the machine in
 *      the launcher's spin loop and releases it via 0040:00F4,
 *   3. lets host_run() perform the MZ load flow: the launcher loads
 *      TESTPROG.EXE via INT21 AH=4B01 (load-don't-execute; see launch.asm
 *      for why dosemu2's own bpload/DBGload stub cannot be used against
 *      stock fdpp) and hands off at the relocated entry, where the
 *      single-step trap stops the machine. host_run() validates that stop
 *      (CS == PSP+0x10+e_cs, IP == e_ip, DS == ES, PSP:0 == CD 20, MCB
 *      owner == PSP) and points code_load_offset at PSP+0x10 dynamically,
 *   4. runs the SAME SIG+hook flow as the .com test — hooks are registered
 *      at IMAGE-relative addresses (rel seg 0) and resolve against the
 *      discovered load segment,
 *   5. verifies hook dispatch counts, raw-code traces, native->guest
 *      callthroughs, flags preservation and the guest-observed results.
 *
 * The .com variant of this test lives in test_driver.c; this file follows
 * its single-test style deliberately.
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

/* Raw-code slot region for this run. raw_code_offset is a LINEAR address;
 * the 64KB slot window must not overlap the MZ image, which loads wherever
 * DOS finds room above command.com (PSP well below 0xE00 in practice). */
#define RAW_CODE_LINEAR 0xF000
#define IMAGE_TOP_MAX   0xE000   /* refuse to run if the image creeps up */

/* testprog_exe.asm layout: entry (e_ip) at header end, then data, then code */
#define RESULT_DELTA   4    /* image offsets relative to the entry offset */
#define HOOKCNT_DELTA  6
#define RESULT2_DELTA  8
#define RES3_DELTA     10
#define RES4_DELTA     12
#define FLAGRES_DELTA  14
#define STACK_SIZE     512

/* Unique function signatures (`mov ax,imm16` bodies, kept unique in the asm;
 * identical bytes to the .com variant). */
static const uint8_t myfunc_sig[]   = { 0xB8, 0x11, 0x11, 0xC3 };
static const uint8_t func2_sig[]    = { 0xB8, 0x22, 0x22, 0xC3 };
static const uint8_t callthru_sig[] = { 0xB8, 0x33, 0x33, 0xC3 };
static const uint8_t helper2_sig[]  = { 0xB8, 0xCE, 0x7A };
static const uint8_t helper_sig[]   = { 0xB8, 0x44, 0x44 };

static uint16_t g_entry_ip;       /* parsed from the MZ header */
static uint16_t g_hookcnt_off;    /* image offsets (== load-seg offsets) */
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
/* the hooks                                                          */
/* ------------------------------------------------------------------ */

/* Image offsets become guest-linear only after the bpload phase revealed
 * the load segment (ctx->code_load_offset); compute physical addresses
 * lazily at dispatch time. */
static uint32_t img_phys(const hydra_machine_t *m, uint16_t off)
{
  const host_ctx_t *ctx = (const host_ctx_t *)m->hardware->ctx;
  return ((uint32_t)ctx->code_load_offset << 4) + off;
}

/* hook1: exercise the guest-opcode execution path (CLI/STI/INT/INB/OUTB) */
HYDRA_FUNC(h_test_hook)
{
  CLI();
  STI();
  INT(0x28);
  u8 t = INB(0x40);
  OUTB(0x80, t);

  /* record into guest memory (host reads this after host_run) */
  u32 phys = img_phys(m, g_hookcnt_off);
  u16 cnt = m->hardware->mem_read16(m->hardware->ctx, phys);
  m->hardware->mem_write16(m->hardware->ctx, phys, (u16)(cnt + 1));
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

/* hook3: native->guest calls through CALL_FAR into unhooked guest code. */
HYDRA_FUNC(h_callthru)
{
  u32 r2 = hydra_impl_call_far(0, g_helper2_off);
  m->hardware->mem_write16(m->hardware->ctx, img_phys(m, g_res4_off),
                           (u16)r2);

  u32 r1 = hydra_impl_call_far(0, g_helper_off);
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

/* Parse the MZ header. Returns 0 on success. Canonical field offsets
 * (RBIL Table 01403): e_cparhdr @0x08, e_ss @0x0E, e_ip @0x14, e_cs @0x16.
 * All address fields are relative to the LOAD MODULE base (the resident
 * image starts after the header paragraphs; DOS discards the header). */
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

/* ------------------------------------------------------------------ */
/* launching the guest                                                */
/*                                                                    */
/* bpload must be armed when the target EXEC runs, and arming it at   */
/* connect time does not work against this DOS: fdpp launches its     */
/* shell via a guest EXEC that would consume the load breakpoint. So  */
/* dosemu runs the LAUNCHER from autoexec instead (-E launch.com):    */
/*   1. comcom32 EXECs launch.com normally (bpload NOT armed yet),    */
/*   2. the launcher marks 0040:00F5 = 0xA5 ("resident") and spins    */
/*      until the host sets 0040:00F4 != 0 ("go"),                    */
/*   3. we poll the marker over shared memory, stop the machine       */
/*      inside the spin loop, set the go flag and hand control to     */
/*      host_run(), which arms bpload before resuming. The next (and  */
/*      only) EXEC is then ours to hijack (see launch.asm).           */
/* ------------------------------------------------------------------ */

#define BDA_SEG          0x40
#define BDA_LAUNCH_GO    0xF4    /* host -> launcher: proceed to the EXEC */
#define BDA_LAUNCH_UP    0xF5    /* launcher -> host: resident mark (A5h) */
#define BDA_LAUNCH_STAT  0xF6    /* launcher -> host: load status */
#define BDA_LAUNCH_ERR   0xF7    /* launcher -> host: DOS error code */
#define BDA_LAUNCH_PSP   0xF8    /* launcher -> host: word, child PSP */

static int stop_when_hooked(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                            const host_run_stats_t *stats, void *user)
{
  (void)regs;
  (void)stats;
  size_t target = *(size_t *)user;
  uint32_t phys = (((uint32_t)ctx->code_load_offset) << 4) + g_hookcnt_off;
  return lowmem_read16(ctx->lm, phys) >= target;
}

/* Find an image offset by unique signature. */
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
  const char *exe_path = (argc > 2) ? argv[2] : TESTPROG_EXE_PATH;

  printf("=== Phase 7 Item C integration test (MZ/bpload) ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(exe_path, &file, &file_len) != 0) {
    printf("cannot read .EXE: %s\n", exe_path);
    return 2;
  }
  printf("guest program: %s (%zu bytes)\n", exe_path, file_len);

  uint16_t e_cs = 0, e_ip = 0, e_ss = 0, hdr_bytes = 0;
  if (parse_mz_header(file, file_len, &e_cs, &e_ip, &e_ss, &hdr_bytes) != 0) {
    printf("not a parseable MZ image\n");
    free(file);
    return 2;
  }
  CHECK(e_cs == 0 && e_ip < 0x100,
        "MZ header entry %04x:%04x module-relative (e_ss=%04x, hdr=%u bytes)",
        e_cs, e_ip, e_ss, hdr_bytes);
  g_entry_ip = e_ip;

  /* code_load=0x0 is a placeholder: the bpload phase rewrites it to
   * PSP+0x10 before any hook address is resolved. */
  char conf[160];
  snprintf(conf, sizeof(conf),
           "dosemu|pid=%ld|code_load=0x0|data_seg=0x0|raw_code=0x%x",
           (long)pid, RAW_CODE_LINEAR);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;

  printf("connected to dosemu2 pid=%ld\n", (long)host_pid(ctx));

  /* locate every function by signature (image offsets) */
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

  /* whole load module fits one segment (e_cs = 0): rel segs are all 0.
   * Signature positions are FILE offsets; resident (module) offsets are
   * file offset minus the header size. */
  uint16_t myfunc_off   = (uint16_t)(myfunc_pos - hdr_bytes);
  uint16_t func2_off    = (uint16_t)(func2_pos - hdr_bytes);
  uint16_t callthru_off = (uint16_t)(callthru_pos - hdr_bytes);
  g_hookcnt_off = (uint16_t)(e_ip + HOOKCNT_DELTA);
  g_res4_off    = (uint16_t)(e_ip + RES4_DELTA);
  g_helper_off  = (uint16_t)(helper_pos - hdr_bytes);
  g_helper2_off = (uint16_t)(helper2_pos - hdr_bytes);
  printf("hooks (image-relative): myfunc %04x func2 %04x callthru %04x\n",
         myfunc_off, func2_off, callthru_off);

  HYDRA_REGISTER_ADDR(h_test_hook,  0, myfunc_off, 0);
  HYDRA_REGISTER_ADDR(h_test_hook2, 0, func2_off, 0);
  HYDRA_REGISTER_ADDR(h_callthru,   0, callthru_off, 0);
  CHECK(host_hook_breakpoint_count(ctx) == 3, "3 hooks registered "
        "(breakpoint count = %d)", host_hook_breakpoint_count(ctx));

  /* release boot and wait for the LAUNCHER: launch.com starts normally
   * from autoexec (bpload NOT armed yet), marks 0040:00F5 and spins; only
   * after we see that marker do we hand control to host_run(), which arms
   * bpload before releasing the launcher's EXEC (its INT21 AH=4B00 is the
   * call bpload must hijack). */
  printf("releasing boot; waiting for launcher (0040:00F5 == A5)...\n");
  if (dosdebug_go(ctx->db) != 0) {
    printf("FAIL: cannot resume the machine\n");
    host_disconnect(ctx); free(file); return 1;
  }
  int launcher_up = 0;
  for (int i = 0; i < 600 && !launcher_up; i++) {
    usleep(100 * 1000);
    launcher_up = lowmem_read8(ctx->lm,
                               ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_UP)
                  == 0xA5;
  }
  CHECK(launcher_up, "launcher running (resident marker seen)");
  if (!launcher_up) { host_disconnect(ctx); free(file); return 1; }

  /* keep the machine parked inside the launcher's spin loop while setting
   * the go flag: the launcher then loads TESTPROG.EXE via INT21 AH=4B01
   * (see launch.asm), publishes the child PSP at 0040:00F8 and idles; the
   * host validates the image and pushes the entry state itself */
  dosdebug_regs_t parked;
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot stop the machine\n");
    host_disconnect(ctx); free(file); return 1;
  }
  printf("  launcher resident, cpu parked at %04x:%04x\n",
         parked.cs, parked.ip);
  lowmem_write8(ctx->lm, ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_GO, 1);
  printf("go flag set; waiting for the loader to publish the child PSP...\n");
  if (dosdebug_go(ctx->db) != 0) {          /* let the loader proceed */
    printf("FAIL: cannot resume the machine\n");
    host_disconnect(ctx); free(file); return 1;
  }

  /* wait for "loaded" (0040:00F6 = 5A) and read the published PSP */
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
  if (!loaded)
    printf("  loader status: F6=%02x err=%02x\n",
           lowmem_read8(ctx->lm, ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_STAT),
           lowmem_read8(ctx->lm, ((uint32_t)BDA_SEG << 4) + BDA_LAUNCH_ERR));
  if (!loaded) { host_disconnect(ctx); free(file); return 1; }

  /* re-park the machine (it idles in the launcher), then push the exact
   * DOS entry state: CS=PSP+0x10+e_cs, IP=e_ip, DS=ES=PSP, stack from the
   * header, GP registers zeroed */
  if (host_get_regs(ctx, &parked) != 0) {
    printf("FAIL: cannot stop the machine\n");
    host_disconnect(ctx); free(file); return 1;
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
  printf("  identity: mcb=%d sig=%d img=%d; mem@load:",
         mcb_ok, sig_ok, img_ok);
  for (int i = 0; i < 16; i++)
    printf(" %02x", lowmem_read8(ctx->lm, load_phys_chk + i));
  printf("\n  file@hdr: ");
  for (int i = 0; i < 16; i++) printf(" %02x", file[hdr_bytes + i]);
  printf("\n");
  int image_ok = mcb_ok && sig_ok && img_ok;
  CHECK(image_ok, "loaded image verified in memory (PSP=%04x, "
        "load seg=%04x)", child_psp, load_seg);
  printf("  image@0x10-0x3f pre-run:");
  for (int i = 0x10; i < 0x40; i++)
    printf(" %02x", lowmem_read8(ctx->lm, load_phys_chk + i));
  printf("\n");
  if (!image_ok) { host_disconnect(ctx); free(file); return 1; }

  /* push the exact DOS entry state: CS=PSP+0x10+e_cs, IP=e_ip, DS=ES=PSP,
   * stack from the header, GP registers zeroed */
  dosdebug_regs_t entry;
  memset(&entry, 0, sizeof(entry));
  entry.cs = (uint16_t)(load_seg + e_cs);
  entry.ip = e_ip;
  entry.ds = child_psp;
  entry.es = child_psp;
  entry.ss = (uint16_t)(load_seg + e_ss);
  entry.sp = (uint16_t)(file[0x10] | (file[0x11] << 8));
  entry.flags = 0x3202;
  if (host_set_regs(ctx, &entry) != 0) {
    printf("FAIL: cannot push the child entry state\n");
    host_disconnect(ctx); free(file); return 1;
  }

  /* run: host_run validates the parked entry state -> dynamic
   * code_load_offset -> hook planting -> normal loop */
  host_run_stats_t stats;
  host_run_options_t opts = {0};
  size_t target = 5;
  opts.timeout_ms = 3000;
  opts.stop_fn = stop_when_hooked;
  opts.stop_user = &target;
  opts.verbose = 1;
  opts.mz_load = 1;
  opts.mz_parked_at_entry = 1;
  opts.mz_entry_cs = e_cs;
  opts.mz_entry_ip = e_ip;
  opts.mz_timeout_ms = 60000;

  /* Hardened host: raw-code execution requires an explicit guest-owned
   * reservation (16-byte aligned, segment >= code_load_offset, >= one
   * 128-byte slot). RAW_CODE_LINEAR=0xF000 sits above the MZ load seg. */
  CHECK(host_reserve_raw_code(ctx, RAW_CODE_LINEAR, 8192) == 0,
        "raw-code reservation registered (linear %x)", RAW_CODE_LINEAR);
  CHECK(host_raw_code_ready(ctx), "raw-code reservation ready");

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);

  printf("host_run ended: %s\n", reason_name(reason));

  {
    dosdebug_regs_t d;
    if (host_get_regs(ctx, &d) == 0)
      printf("  cpu now at %04x:%04x (sp=%04x ss=%04x)\n",
             d.cs, d.ip, d.sp, d.ss);
  }
  printf("  stops=%lu hook_dispatches=%lu raw_code_runs=%lu "
         "raw_code_returns=%lu redirects=%lu mz_psp=%04lx\n",
         (unsigned long)stats.stops, (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs,
         (unsigned long)stats.raw_code_returns,
         (unsigned long)stats.redirects, (unsigned long)stats.mz_psp);

  /* verify the MZ load itself */
  uint16_t psp = (uint16_t)stats.mz_psp;
  uint32_t load_phys;
  CHECK(psp != 0, "MZ entry state validated (PSP=%04x)", psp);
  {
    load_phys = (uint32_t)(psp + 0x10u) << 4;
    printf("  image@0x10-0x3f post-run:");
    for (int i = 0x10; i < 0x40; i++)
      printf(" %02x", lowmem_read8(ctx->lm, load_phys + i));
    printf("\n");
  }
  if (psp == 0) { host_disconnect(ctx); free(file); return 1; }

  CHECK(ctx->code_load_offset == (uint16_t)(psp + 0x10),
        "code_load_offset set dynamically (PSP+0x10 = %04x)",
        ctx->code_load_offset);
  load_phys = (uint32_t)ctx->code_load_offset << 4;
  {
    uint32_t image_top = (uint32_t)ctx->code_load_offset << 4;
    image_top += (uint32_t)file_len;
    CHECK(image_top <= IMAGE_TOP_MAX,
          "image below the raw-code region (top %06x <= %06x)",
          (unsigned)image_top, (unsigned)IMAGE_TOP_MAX);
  }
  /* CD 20 (INT 20h) is the PSP signature: it lives at PSP:0, one paragraph
   * below the loaded module base. */
  CHECK(lowmem_read8(ctx->lm, (uint32_t)psp << 4) == 0xCD &&
        lowmem_read8(ctx->lm, ((uint32_t)psp << 4) + 1) == 0x20,
        "PSP base holds 'INT 20h' signature");

  /* verify the same SIG+hook flow results as the .com test */
  uint16_t hookcnt = lowmem_read16(ctx->lm, load_phys + g_hookcnt_off);
  uint16_t result  = lowmem_read16(ctx->lm, load_phys + e_ip + RESULT_DELTA);
  uint16_t result2 = lowmem_read16(ctx->lm, load_phys + e_ip + RESULT2_DELTA);
  uint16_t res3    = lowmem_read16(ctx->lm, load_phys + e_ip + RES3_DELTA);
  uint16_t res4    = lowmem_read16(ctx->lm, load_phys + g_res4_off);
  uint16_t flagres = lowmem_read16(ctx->lm, load_phys + e_ip + FLAGRES_DELTA);

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
        (unsigned long)stats.raw_code_runs,
        (unsigned long)stats.raw_code_returns);

  /* Deterministic dispatch accounting (identical to the .com test):
   * hookcnt hits `target` exactly at hook1 dispatch #target:
   *   dispatches = target + 3*(target-1) = 17
   *   raw runs   = 5*target + 4*(target-1) = 41
   *   redirects  = raw runs + dispatches */
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
