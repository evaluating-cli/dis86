/* Host-independent unit test for the native host skeleton (issue #98): a
 * hydra_machine_hardware_t implementation on a linear guest-memory array
 * that builds and runs with no dosemu2 process. Covers the memory
 * roundtrips and endianness, the register mirror, the fail-loud io/memory
 * stubs, the snapshot roundtrip, and the absent-user-hook step-hook path. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "internal.h"
#include "native_host.h"
#include "hydra_backend.h"

static int g_failures;
#define CHECK(cond, label) do {                                      \
  if (cond) printf("PASS: %s\n", label);                             \
  else { printf("FAIL: %s\n", label); g_failures++; }                \
} while (0)

/* Fork into a probe that must abort (fail-loud stub). The probe has no
 * return path: either it aborts (expected) or the child exits 0 (failure). */
static int probe_aborts(void (*probe)(hydra_machine_hardware_t *),
                        hydra_machine_hardware_t *hw)
{
  pid_t pid = fork();
  if (pid < 0)
    return 0;
  if (pid == 0) {
    probe(hw);
    _exit(0);
  }
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}

static void probe_io_in8(hydra_machine_hardware_t *hw)     { hw->io_in8(hw->ctx, 0x0201); }
static void probe_io_in16(hydra_machine_hardware_t *hw)    { hw->io_in16(hw->ctx, 0x0201); }
static void probe_io_out8(hydra_machine_hardware_t *hw)    { hw->io_out8(hw->ctx, 0x0201, 0xff); }
static void probe_io_out16(hydra_machine_hardware_t *hw)   { hw->io_out16(hw->ctx, 0x0201, 0xffff); }
static void probe_mem_oob_read8(hydra_machine_hardware_t *hw)
{
  hw->mem_read8(hw->ctx, NATIVE_HOST_MEM_SIZE);
}
static void probe_mem_oob_write16(hydra_machine_hardware_t *hw)
{
  hw->mem_write16(hw->ctx, NATIVE_HOST_MEM_SIZE - 1, 0x1234);
}

int main(void)
{
  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};

  /* The discovery seam (api_impl.c) must find hydra_user_init in the linked
   * native host exactly like it does for the dosemu host. */
  hydra_machine_init(m.hardware, &audio, "native");

  hydra_machine_hardware_t *hw = m.hardware;
  native_host_ctx_t *ctx = (native_host_ctx_t *)hw->ctx;
  CHECK(ctx != NULL, "native host context created");
  if (!ctx)
    return 1;
  CHECK(strstr(native_host_id(), "linear") != NULL,
        "native host library directly linked (native_host_id)");

  /* Both conf outputs must be assigned (api_impl.c aborts otherwise) and the
   * defaults match the dosemu host's 0 defaults. */
  CHECK(HYDRA_CONF->code_load_offset == 0 && HYDRA_CONF->data_section_seg == 0,
        "conf defaults propagated (code_load/data_seg)");
  CHECK(HYDRA_CONF->raw_code_offset == 0 && HYDRA_CONF->raw_code_size == 0,
        "raw-code reservation never guessed");

  /* mem roundtrips: byte, word at an ODD address (both lanes visible), and
   * hostaddr aliasing back onto the same array. */
  hw->mem_write8(hw->ctx, 0x1234, 0xab);
  CHECK(hw->mem_read8(hw->ctx, 0x1234) == 0xab, "mem_read8/write8 roundtrip");

  hw->mem_write16(hw->ctx, 0x1235, 0xcdef);
  CHECK(hw->mem_read8(hw->ctx, 0x1235) == 0xef && hw->mem_read8(hw->ctx, 0x1236) == 0xcd,
        "mem_write16 little-endian byte lanes at odd address");
  CHECK(hw->mem_read16(hw->ctx, 0x1235) == 0xcdef, "mem_read16/write16 roundtrip");

  *hw->mem_hostaddr(hw->ctx, 0x1235) = 0x01;
  CHECK(hw->mem_read16(hw->ctx, 0x1235) == 0xcd01,
        "mem_hostaddr aliases the linear array");

  /* update_registers mirror: the host-side copy is authoritative. */
  ctx->regs.ax = 0x1111;
  ctx->regs.bx = 0x2222;
  ctx->regs.sp = 0x3333;
  ctx->regs.cs = 0x0200;
  ctx->regs.flags = 0x0042;
  memset(m.registers, 0, sizeof(m.registers));
  hw->update_registers(hw->ctx, m.registers);
  CHECK(m.registers->ax == 0x1111 && m.registers->bx == 0x2222 &&
            m.registers->sp == 0x3333 && m.registers->cs == 0x0200 &&
            m.registers->flags == 0x0042,
        "update_registers mirrors the native host register copy");

  /* Fail loud: every unmodeled port and every out-of-bounds access aborts. */
  struct {
    void (*probe)(hydra_machine_hardware_t *);
    const char *label;
  } const probes[] = {
    {probe_io_in8, "io_in8 fail loud (abort)"},
    {probe_io_in16, "io_in16 fail loud (abort)"},
    {probe_io_out8, "io_out8 fail loud (abort)"},
    {probe_io_out16, "io_out16 fail loud (abort)"},
    {probe_mem_oob_read8, "mem_read8 out of bounds fail loud (abort)"},
    {probe_mem_oob_write16, "mem_write16 out of bounds fail loud (abort)"},
  };
  for (size_t i = 0; i < sizeof(probes) / sizeof(probes[0]); i++)
    CHECK(probe_aborts(probes[i].probe, hw), probes[i].label);

  /* Snapshot roundtrip: save the labeled state, smash memory + registers,
   * restore, and require the world back byte-exact. */
  char path[64];
  snprintf(path, sizeof(path), "/tmp/native_host_state_test_XXXXXX");
  int fd = mkstemp(path);
  if (fd < 0) {
    printf("FAIL: could not create state file %s\n", path);
    return 1;
  }
  close(fd);

  hw->mem_write16(hw->ctx, 0x0450, 0xbeef);
  hw->mem_write8(hw->ctx, 0x8000, 0x5a);
  ctx->regs.sp = 0x7777;
  ctx->regs.cs = 0x0200;
  hw->state_save(hw->ctx, path);

  hw->mem_write16(hw->ctx, 0x0450, 0x0000);
  hw->mem_write8(hw->ctx, 0x8000, 0x00);
  ctx->regs.sp = 0x0000;
  ctx->regs.cs = 0x0444;
  hw->state_restore(hw->ctx, path);

  CHECK(hw->mem_read16(hw->ctx, 0x0450) == 0xbeef &&
            hw->mem_read8(hw->ctx, 0x8000) == 0x5a &&
            ctx->regs.sp == 0x7777 && ctx->regs.cs == 0x0200,
        "state_save/state_restore roundtrip (memory + registers)");

  unlink(path);

  /* Absent-user-hook path: the native host defines no hydra_user_step_hook,
   * so api_impl's cache stays NULL and the call is a safe no-op. */
  hydra_machine_step_hook(&m);
  CHECK(1, "step_hook no-op with absent user hook");

  /* Device HAL, terminal backend and INT dispatch skeleton (issue #95).
   * Separate entry point so the native host keeps a single main(); returns
   * its failure count to be folded into the run's total. */
  printf("\n--- device HAL / terminal backend / INT dispatch (issue #95) ---\n");
  g_failures += hydra_backend_selftest();
  g_failures += hydra_backend_linktest(&m);

  if (g_failures) {
    printf("FAIL: %d failures\n", g_failures);
    return 1;
  }
  printf("PASS: native host skeleton (issue #98)\n");
  return 0;
}
