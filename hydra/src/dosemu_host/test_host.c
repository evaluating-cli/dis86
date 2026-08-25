/* Real dosemu2 host integration checks: verified lowmem, register fidelity,
 * process-persistent snapshot file, and basic debugger breakpoints. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "host.h"

static int g_failures;
#define CHECK(cond, label) do {                                      \
  if (cond) printf("PASS: %s\n", label);                             \
  else { printf("FAIL: %s\n", label); g_failures++; }                \
} while (0)

int main(int argc, char **argv)
{
  char conf[128] = "dosemu";
  if (argc > 1) {
    long pid = strtol(argv[1], NULL, 0);
    if (pid <= 0) return 2;
    snprintf(conf, sizeof(conf), "dosemu|pid=%ld", pid);
  }

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;
  CHECK(ctx != NULL, "host context created");
  if (!ctx) return 1;
  CHECK(host_pid(ctx) != 0, "connected to dosemu2");
  CHECK(ctx->have_initial_regs, "initial register state read");

  /* Verified lowmem selection is exercised by hydra_machine_init itself. */
  uint8_t *ivt = m.hardware->mem_hostaddr(m.hardware->ctx, 0);
  CHECK(ivt != NULL, "verified lowmem mapping exposed");
  CHECK(m.hardware->mem_read16(m.hardware->ctx, 0) != 0,
        "IVT visible through selected backing");

  dosdebug_regs_t original;
  CHECK(host_get_regs(ctx, &original) == 0, "read complete register state");

  /* Guest IF must round-trip exactly. This is the regression for the old
   * dosdebug_write_regs() OR-with-0x3202 behavior. */
  dosdebug_regs_t if0 = original;
  if0.flags &= (uint16_t)~0x0200u;
  CHECK(host_set_regs(ctx, &if0) == 0, "write register state with guest IF=0");
  dosdebug_regs_t after_if0;
  CHECK(host_get_regs(ctx, &after_if0) == 0 &&
        (after_if0.flags & 0x0200u) == 0,
        "guest-visible IF=0 reads back exactly");
  CHECK(host_set_regs(ctx, &original) == 0, "restore original register state");

  /* Snapshot must survive the process that created it. The format itself is a
   * regular file containing registers plus the complete 1 MiB+HMA window; this
   * test closes/reopens that file through state_restore after mutating both. */
  char snap[160];
  snprintf(snap, sizeof(snap), "/tmp/hydra_host_snapshot_%ld.bin", (long)getpid());
  unlink(snap);
  const uint32_t scratch = 0x50000u;
  uint16_t original_mem = m.hardware->mem_read16(m.hardware->ctx, scratch);

  dosdebug_regs_t saved_regs = original;
  saved_regs.ax = 0x3456;
  CHECK(host_set_regs(ctx, &saved_regs) == 0, "prepare snapshot AX");
  m.hardware->mem_write16(m.hardware->ctx, scratch, 0x1234);
  m.hardware->state_save(m.hardware->ctx, snap);

  struct stat st;
  CHECK(stat(snap, &st) == 0 && st.st_size > 0x110000,
        "snapshot persisted to a nonempty file");

  CHECK(host_set_reg(ctx, "AX", 0x7777) == 0, "mutate AX after snapshot");
  m.hardware->mem_write16(m.hardware->ctx, scratch, 0xbeef);
  m.hardware->state_restore(m.hardware->ctx, snap);

  dosdebug_regs_t restored;
  CHECK(host_get_regs(ctx, &restored) == 0 && restored.ax == 0x3456,
        "snapshot restored registers");
  CHECK(m.hardware->mem_read16(m.hardware->ctx, scratch) == 0x1234,
        "snapshot restored guest low memory");
  unlink(snap);

  /* Put the live guest back exactly as found before destructive checks. */
  m.hardware->mem_write16(m.hardware->ctx, scratch, original_mem);
  CHECK(host_set_regs(ctx, &original) == 0, "restore original CPU state");

  /* Basic breakpoint protocol smoke test at current CS:IP + a tiny walk loop. */
  dosdebug_regs_t cur;
  CHECK(host_get_regs(ctx, &cur) == 0, "read state before breakpoint smoke");
  uint16_t cs = cur.cs;
  uint16_t ip = cur.ip;
  uint8_t saved[8];
  uint32_t walk = (uint32_t)cs * 16u + 0x100u;
  for (int i = 0; i < 8; i++) saved[i] = m.hardware->mem_read8(m.hardware->ctx, walk + i);
  const uint8_t loop[8] = {0xEB,0x04,0x90,0x90,0x90,0x90,0xEB,0xFA};
  for (int i = 0; i < 8; i++) m.hardware->mem_write8(m.hardware->ctx, walk + i, loop[i]);
  CHECK(host_set_reg(ctx, "IP", 0x0102) == 0, "redirect IP for breakpoint smoke");
  int bpi = host_set_bp(ctx, cs, 0x0104);
  CHECK(bpi >= 0, "plant debugger breakpoint");
  dosdebug_regs_t stopregs;
  CHECK(bpi >= 0 && host_go_and_wait(ctx, &stopregs, 4000) == 0,
        "continue until breakpoint");
  CHECK(stopregs.cs == cs && stopregs.ip == 0x0104,
        "breakpoint reports exact CS:IP");
  if (bpi >= 0) host_clear_bp(ctx, bpi);
  host_stop(ctx);
  for (int i = 0; i < 8; i++) m.hardware->mem_write8(m.hardware->ctx, walk + i, saved[i]);
  host_set_reg(ctx, "IP", ip);

  host_disconnect(ctx);
  printf("%s\n", g_failures ? "TEST FAILED" : "ALL TESTS PASSED");
  return g_failures ? 1 : 0;
}
