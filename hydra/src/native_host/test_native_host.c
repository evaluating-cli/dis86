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
/* A 16-bit access is two bus cycles, so the second half gets its own
 * routing decision: 0x42 is counter 2 but 0x43 is the write-only control
 * register, and 0x44 is not mapped at all. Both halves must be checked,
 * which is what keeps io_in16/io_out16 from quietly accepting a half. */
static void probe_io_in16_half(hydra_machine_hardware_t *hw)  { hw->io_in16(hw->ctx, 0x42); }
static void probe_io_out16_half(hydra_machine_hardware_t *hw) { hw->io_out16(hw->ctx, 0x43, 0); }
static void probe_mem_oob_read8(hydra_machine_hardware_t *hw)
{
  hw->mem_read8(hw->ctx, NATIVE_HOST_MEM_SIZE);
}
static void probe_mem_oob_write16(hydra_machine_hardware_t *hw)
{
  hw->mem_write16(hw->ctx, NATIVE_HOST_MEM_SIZE - 1, 0x1234);
}

/* The PIT control register is write-only on real hardware; reading it must
 * still fail loud rather than returning a stale or invented byte. */
static void probe_pit_control_read(hydra_machine_hardware_t *hw)
{
  hw->io_in8(hw->ctx, 0x43);
}

/* Deterministic input backend for the port tests: the terminal backend
 * cannot have a key injected without a tty, and the key-present path is the
 * one worth covering. */
typedef struct {
  uint16_t key;
  int pending;
} fake_port_input_t;

static int fake_port_poll_key(void *state, uint16_t *key)
{
  fake_port_input_t *in = (fake_port_input_t *)state;
  if (!in->pending)
    return 0;
  *key = in->key;
  return 1;
}

static int fake_port_read_key(void *state, uint16_t *key)
{
  fake_port_input_t *in = (fake_port_input_t *)state;
  if (!in->pending)
    return 0;
  *key = in->key;
  in->pending = 0;
  return 1;
}

/* Counted so the test can assert that swapping this backend out runs its
 * teardown. It matters for the real backend too: the terminal's teardown is
 * what puts stdin back the way it found it. */
static int g_fake_shutdowns;

static void fake_port_shutdown(void *state)
{
  (void)state;
  g_fake_shutdowns++;
}

static const hydra_backend_t FAKE_PORT_BACKEND = {
  .name     = "fake-port-input",
  .poll_key = fake_port_poll_key,
  .read_key = fake_port_read_key,
  .shutdown = fake_port_shutdown,
};

/* Installed over FAKE_PORT_BACKEND purely to check that installing a backend
 * tears the outgoing one down. Every hook is optional, so a name is enough. */
static const hydra_backend_t NULL_PROBE_BACKEND = {
  .name = "null-probe",
};

/* A PIT read is two 8-bit cycles: low byte, then high byte. */
static uint16_t pit_read16(hydra_machine_hardware_t *hw)
{
  uint8_t lo = hw->io_in8(hw->ctx, 0x40);
  uint8_t hi = hw->io_in8(hw->ctx, 0x40);
  return (uint16_t)(lo | ((uint16_t)hi << 8));
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

  /* The backend is chosen during init, not lazily on the first dispatch, so
   * a hook thread can never race with first-use initialization. */
  void *backend_state = NULL;
  const hydra_backend_t *backend = hydra_native_backend_active(&backend_state);
  CHECK(backend != NULL && backend_state != NULL,
        "backend selected during init (no lazy first-use init)");

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
    {probe_io_in16_half, "io_in16 second half still routed (0x42 -> write-only 0x43)"},
    {probe_io_out16_half, "io_out16 second half still routed (0x43 -> unmapped 0x44)"},
    {probe_pit_control_read, "routed but read-only port still fails loud (0x43)"},
    {probe_mem_oob_read8, "mem_read8 out of bounds fail loud (abort)"},
    {probe_mem_oob_write16, "mem_write16 out of bounds fail loud (abort)"},
  };
  for (size_t i = 0; i < sizeof(probes) / sizeof(probes[0]); i++)
    CHECK(probe_aborts(probes[i].probe, hw), probes[i].label);

  /* --- port -> device routing (issue #95) --------------------------------
   * The two devices routed so far are the keyboard and the PIT: between them
   * they cover #95's Tier 1 "keyboard-only, direct port polling" criterion.
   * Everything unmapped still aborts, which the probes above verify. */

  /* Keyboard with nothing pending. The data port must not block: read_key()
   * waits for a key, so a guest that polls 0x60 without checking 0x64 first
   * would hang instead of seeing an empty buffer. */
  CHECK(hw->io_in8(hw->ctx, 0x64) == 0x00,
        "port routing: keyboard status 0x64 clear with no key pending");
  CHECK(hw->io_in8(hw->ctx, 0x60) == 0x00,
        "port routing: keyboard data 0x60 returns 0 instead of blocking");

  /* Keyboard with a key waiting. 0x64 reports it, 0x60 carries the scan
   * code (high byte of the BIOS key word), and the read consumes it. */
  {
    fake_port_input_t fi = { 0 };
    fi.key = 0x1e61;                       /* 'a': scan 1Eh, ASCII 61h */
    fi.pending = 1;
    hydra_native_backend_install_probe(&FAKE_PORT_BACKEND, &fi);

    CHECK(hw->io_in8(hw->ctx, 0x64) == 0x01,
          "port routing: keyboard status 0x64 set when a key is pending");
    CHECK(hw->io_in8(hw->ctx, 0x60) == 0x1e,
          "port routing: keyboard data 0x60 returns the scan code");
    CHECK(hw->io_in8(hw->ctx, 0x64) == 0x00,
          "port routing: reading 0x60 consumes the key");

    hydra_native_backend_select(NULL);     /* restore the default backend */
  }

  /* Backend lifecycle: installing a backend over a live one must tear the
   * outgoing one down, the way select() does. Skipping that leaves the
   * terminal's stdin in non-blocking mode, and the next select() then saves
   * those already-broken flags as the user's original ones - so the atexit
   * handler would "restore" a non-blocking shell. The suite cannot see this
   * directly (CI stdin is a pipe, where the flag is never set), hence the
   * counter. */
  {
    fake_port_input_t fi = { 0 };
    g_fake_shutdowns = 0;
    hydra_native_backend_install_probe(&FAKE_PORT_BACKEND, &fi);
    hydra_native_backend_install_probe(&NULL_PROBE_BACKEND, NULL);
    CHECK(g_fake_shutdowns == 1,
          "backend lifecycle: install_probe shuts down the outgoing backend");
    hydra_native_backend_select(NULL);     /* restore the default backend */
  }

  /* PIT. Before any control word, the device defaults to lo/hi - the mode
   * nearly every guest programs, and the one port_devices_init() sets so
   * that a channel is never left in the "latch command" state the zeroed
   * struct would otherwise mean. */
  hw->io_out8(hw->ctx, 0x40, 0x34);        /* low, then high, no control word */
  hw->io_out8(hw->ctx, 0x40, 0x12);        /* reload 0x1234, clock restarts */
  {
    uint16_t v = pit_read16(hw);
    CHECK(v > 0x1100 && v <= 0x1234,
          "port routing: PIT defaults to lo/hi before any control word");
  }

  /* A written reload must bound the count, and the count must actually
   * move: those two together are what a guest timing loop depends on. */
  hw->io_out8(hw->ctx, 0x43, 0x36);        /* ch0, lo/hi, mode 3, binary */
  hw->io_out8(hw->ctx, 0x40, 0x00);        /* reload low byte  */
  hw->io_out8(hw->ctx, 0x40, 0x08);        /* reload high byte -> 0x0800 */

  uint16_t c1 = pit_read16(hw);
  /* With a reload of 0x0800 the count can only land in 1..0x0800: it is
   * reload minus the ticks elapsed since, and 0x0800 fits in 16 bits so it
   * never truncates to 0. A counter that ignored the reload would run on the
   * 0x10000 default period, whose count truncates to 0 at tick 0 — so the
   * != 0 half is what actually pins the reload down. */
  CHECK(c1 != 0 && c1 <= 0x0800,
        "port routing: PIT honours a written reload (count within 0x0800)");
  usleep(20000);
  uint16_t c2 = pit_read16(hw);
  CHECK(c1 != c2, "port routing: PIT counter advances over time");

  /* --- 16-bit access: byte order, and the second half's own port --------
   * A 16-bit access to an 8-bit device is two bus cycles, so io_out16(p,v)
   * must put v's low byte on p and its high byte on p+1, and io_in16(p)
   * must compose them back the same way. The two channels get different
   * periods from one word write; the periods are large enough that the few
   * ticks spent between the reload and the latch cannot wrap either of
   * them, and both are latched so the read does not race the counter. */
  hw->io_out8(hw->ctx, 0x43, 0x36);        /* ch0, lo/hi, mode 3, binary */
  hw->io_out8(hw->ctx, 0x43, 0x76);        /* ch1, lo/hi, mode 3, binary */
  hw->io_out16(hw->ctx, 0x40, 0x80c0);     /* ch0 low = 0xc0, ch1 low = 0x80 */
  hw->io_out8(hw->ctx, 0x40, 0x10);        /* ch0 -> period 0x10c0 */
  hw->io_out8(hw->ctx, 0x41, 0x10);        /* ch1 -> period 0x1080 */
  hw->io_out8(hw->ctx, 0x43, 0x00);        /* latch ch0 */
  hw->io_out8(hw->ctx, 0x43, 0x40);        /* latch ch1 */
  {
    uint16_t w = hw->io_in16(hw->ctx, 0x40);
    /* Low byte from ch0 (period 0x10c0), high byte from ch1 (0x1080), with
     * a few ticks of slack for the time between the reload and the latch.
     * Swapping the bytes, or dropping the second half's port, moves both
     * halves out of range. */
    CHECK((w & 0xff) >= 0xa0 && (w & 0xff) <= 0xc0,
          "port routing: io_in16 low byte comes from port+0 (ch0)");
    CHECK((w >> 8) >= 0x60 && (w >> 8) <= 0x80,
          "port routing: io_in16 high byte comes from port+1 (ch1)");
  }

  /* --- the RW field governs reads, not just writes ----------------------
   * A real 8253/8254 returns the same byte on every read in the LSB-only
   * and MSB-only modes; only RW=11 alternates. With a period of 0xff the
   * count's high byte is always 0x00, so a device that ignored the mode
   * and alternated regardless would emit 0x00 on every second read. */
  hw->io_out8(hw->ctx, 0x43, 0x14);        /* ch0, LSB only, mode 2, binary */
  hw->io_out8(hw->ctx, 0x40, 0xff);        /* period 0x00ff */
  {
    uint8_t b0 = hw->io_in8(hw->ctx, 0x40);
    uint8_t b1 = hw->io_in8(hw->ctx, 0x40);
    uint8_t b2 = hw->io_in8(hw->ctx, 0x40);
    /* Three reads, all the low byte of a count that is still near its
     * reload: never the 0x00 the high byte would be. */
    CHECK(b0 >= 0xe0 && b1 >= 0xe0 && b2 >= 0xe0,
          "port routing: PIT RW=01 returns the low byte on every read");
  }

  /* --- RW=00 is the counter-latch command, not an access mode -----------
   * It has to (a) freeze a snapshot, so a latch-then-read pair cannot tear
   * across a low-byte carry, and (b) leave the programmed access mode
   * alone, so a latch issued after "LSB only" does not turn reads back
   * into an alternating low/high pair. */
  hw->io_out8(hw->ctx, 0x43, 0x36);        /* ch0, lo/hi */
  hw->io_out8(hw->ctx, 0x40, 0x00);
  hw->io_out8(hw->ctx, 0x40, 0x10);        /* period 0x1000 */
  hw->io_out8(hw->ctx, 0x43, 0x00);        /* latch ch0 */
  usleep(1000);                            /* ~1193 ticks: the count moves */
  {
    /* Both bytes come from the latch, so this is the count as of the latch
     * and not as of now: still near 0x1000, where a live read 1 ms later
     * would already be down around 0x0b50. */
    uint16_t v = pit_read16(hw);
    CHECK(v > 0x0f00 && v <= 0x1000,
          "port routing: PIT latch command freezes the count");
  }

  hw->io_out8(hw->ctx, 0x43, 0x14);        /* ch0, LSB only */
  hw->io_out8(hw->ctx, 0x40, 0xff);        /* period 0x00ff */
  hw->io_out8(hw->ctx, 0x43, 0x00);        /* latch ch0 -- must not reset RW */
  {
    uint8_t a = hw->io_in8(hw->ctx, 0x40); /* latched low byte */
    uint8_t b = hw->io_in8(hw->ctx, 0x40); /* live low byte, LSB-only */
    /* Both are the low byte of a count near 0xff. A latch command that
     * overwrote the access mode would alternate here and hand back 0x00
     * (the high byte) as the second read. */
    CHECK(a >= 0xe0 && b >= 0xe0,
          "port routing: PIT latch command keeps the LSB-only access mode");
  }

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
  g_failures += hydra_backend_videotest(&m);

  if (g_failures) {
    printf("FAIL: %d failures\n", g_failures);
    return 1;
  }
  printf("PASS: native host skeleton (issue #98)\n");
  return 0;
}
