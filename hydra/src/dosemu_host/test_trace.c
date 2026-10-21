/*
 * test_trace.c - the Hydra trace sink (issue #97).
 *
 * The sink is a core facility, so this test lives with the other standalone
 * Hydra tests (test_unimpl_trap.c is a core-header test in the same place).
 * It needs no dosemu2: a fake machine with a flat RAM is enough to exercise
 * the argument capture at the native->guest call seam.
 *
 * The format is pinned here: the reader in dis86 --trace-analyze parses
 * exactly what this test expects, so a change to the writer without a
 * change to the reader is caught.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

#include "typedefs.h"
#include "hydra_machine.h"
#include "addr.h"
#include "conf.h"
#include "functions.h"
#include "trace.h"

/* Defined in hydra/src/conf.c. Declared here so the test does not need the
 * internal header (several Hydra headers have no include guards). */
extern hydra_conf_t HYDRA_CONF[1];

static int g_failures;
#define CHECK(cond, label) do {                                      \
  if (cond) printf("PASS: %s\n", label);                             \
  else { printf("FAIL: %s\n", label); g_failures++; }                \
} while (0)

/* ------------------------------------------------------------------ */
/* a fake machine: flat 64 KiB of RAM, no devices                      */
/* ------------------------------------------------------------------ */

static uint8_t g_ram[0x10000];

static uint8_t t_mem_read8(hydra_machine_ctx_t *ctx, uint32_t addr)
{
  (void)ctx;
  return g_ram[addr & 0xffff];
}

static uint16_t t_mem_read16(hydra_machine_ctx_t *ctx, uint32_t addr)
{
  (void)ctx;
  return (uint16_t)(g_ram[addr & 0xffff] | (g_ram[(addr + 1) & 0xffff] << 8));
}

static void t_mem_write8(hydra_machine_ctx_t *ctx, uint32_t addr, uint8_t v)
{
  (void)ctx;
  g_ram[addr & 0xffff] = v;
}

static void t_mem_write16(hydra_machine_ctx_t *ctx, uint32_t addr, uint16_t v)
{
  (void)ctx;
  g_ram[addr & 0xffff] = (uint8_t)(v & 0xff);
  g_ram[(addr + 1) & 0xffff] = (uint8_t)(v >> 8);
}

static void t_noop(hydra_machine_ctx_t *ctx, const char *path)
{
  (void)ctx; (void)path;
}

static uint8_t  t_io_in8(hydra_machine_ctx_t *ctx, uint16_t port)  { (void)ctx; (void)port; return 0; }
static uint16_t t_io_in16(hydra_machine_ctx_t *ctx, uint16_t port) { (void)ctx; (void)port; return 0; }
static void t_io_out8(hydra_machine_ctx_t *ctx, uint16_t port, uint8_t v) { (void)ctx; (void)port; (void)v; }
static void t_io_out16(hydra_machine_ctx_t *ctx, uint16_t port, uint16_t v) { (void)ctx; (void)port; (void)v; }

static void t_update_registers(hydra_machine_ctx_t *ctx, hydra_machine_registers_t *regs)
{
  (void)ctx; (void)regs;
}

static hydra_machine_t *make_machine(void)
{
  static hydra_machine_t m;
  memset(&m, 0, sizeof(m));
  m.hardware->mem_read8 = t_mem_read8;
  m.hardware->mem_read16 = t_mem_read16;
  m.hardware->mem_write8 = t_mem_write8;
  m.hardware->mem_write16 = t_mem_write16;
  m.hardware->io_in8 = t_io_in8;
  m.hardware->io_in16 = t_io_in16;
  m.hardware->io_out8 = t_io_out8;
  m.hardware->io_out16 = t_io_out16;
  m.hardware->state_save = t_noop;
  m.hardware->state_restore = t_noop;
  m.hardware->update_registers = t_update_registers;
  return &m;
}

/* ------------------------------------------------------------------ */
/* the metadata the name lookup resolves against                        */
/* ------------------------------------------------------------------ */

static hydra_function_def_t g_defs[1];

static hydra_function_metadata_t g_md = {
  1, g_defs,
};

int main(void)
{
  /* The code load segment the trace records addresses against. */
  HYDRA_CONF->code_load_offset = 0x0080;

  g_defs[0].name = "F_known";
  g_defs[0].addr = ADDR_MAKE(0x0000, 0x0010);
  hydra_function_metadata_set(&g_md);

  hydra_machine_t *m = make_machine();

  /* --- 1. the sink is off by default --- */
  CHECK(!hydra_trace_enabled(), "the trace sink is off by default");
  CHECK(hydra_trace_mem(NULL) == NULL, "no memory buffer without a sink");

  /* --- 2. the format, event by event --- */
  CHECK(hydra_trace_open_mem() == 0, "the memory sink opens");
  CHECK(hydra_trace_enabled(), "the sink reports itself as open");

  hydra_trace_dispatch("F_known", 0x0080, 0x0010, "enter", NULL);
  hydra_trace_dispatch("F_known", 0x0080, 0x0010, "leave", "resume");
  hydra_trace_call("F_known", 0x0080, 0x0012, 0x0080, 0x0010);
  hydra_trace_ret(NULL, 0x0080, 0x0014, 0x0080, 0x0012);
  hydra_trace_step(0x0080, 0x0134);
  hydra_trace_int(0x21, 0x4c);

  size_t len = 0;
  const char *text = hydra_trace_mem(&len);
  CHECK(text != NULL, "the memory sink hands out its text");

  static const char *expected =
    "# hydra trace v1\n"
    "# code_load=0080\n"
    "dispatch enter cs=0080 ip=0010 name=F_known\n"
    "dispatch leave cs=0080 ip=0010 name=F_known result=resume\n"
    "call from=0080:0012 to=0080:0010 name=F_known\n"
    "ret from=0080:0014 to=0080:0012\n"
    "step cs=0080 ip=0134\n"
    "int num=21 ah=4c\n";

  CHECK(text && strcmp(text, expected) == 0,
        "the recorded text matches the pinned format");
  if (text && strcmp(text, expected) != 0) {
    printf("---- recorded ----\n%s---- expected ----\n%s------------\n",
           text, expected);
  }

  /* --- 3. the code load is re-emitted when it changes --- */
  HYDRA_CONF->code_load_offset = 0x0100;
  hydra_trace_step(0x0100, 0x0000);
  text = hydra_trace_mem(&len);
  CHECK(text && strstr(text, "# code_load=0100\nstep cs=0100 ip=0000\n") != NULL,
        "a changed code load is re-emitted before the next event");
  HYDRA_CONF->code_load_offset = 0x0080;

  hydra_trace_close();
  CHECK(!hydra_trace_enabled(), "the sink closes");

  /* --- 4. the arguments captured at the call seam --- */
  memset(g_ram, 0, sizeof(g_ram));
  m->registers->cs = 0x0080;
  m->registers->ip = 0x0012;
  m->registers->ss = 0x0100;
  m->registers->sp = 0x0100;
  m->registers->ax = 0x0001;
  m->registers->dx = 0x0000;

  /* The two argument words, as the callee will read them: arg1 at the top
   * of the stack, arg2 right above it. */
  g_ram[0x1000 + 0x100 + 0] = 0x11;
  g_ram[0x1000 + 0x100 + 1] = 0x11;
  g_ram[0x1000 + 0x100 + 2] = 0x22;
  g_ram[0x1000 + 0x100 + 3] = 0x22;

  hydra_trace_open_mem();
  hydra_trace_native_call(m, ADDR_MAKE(0x0000, 0x0010), 2);
  hydra_trace_native_ret(m, ADDR_MAKE(0x0000, 0x0010));

  text = hydra_trace_mem(&len);
  CHECK(text && strstr(text,
        "native_call from=0080:0012 to=0080:0010 name=F_known nargs=2 arg0=1111 arg1=2222\n") != NULL,
        "the call seam records the argument words the callee will read");
  CHECK(text && strstr(text,
        "native_ret from=0080:0012 to=0080:0010 dx=0000 ax=0001\n") != NULL,
        "the call seam records the returned value");
  hydra_trace_close();

  /* --- 5. an unknown callee carries no name --- */
  hydra_trace_open_mem();
  hydra_trace_native_call(m, ADDR_MAKE(0x0000, 0x00ff), 0);
  text = hydra_trace_mem(&len);
  CHECK(text && strstr(text,
        "native_call from=0080:0012 to=0080:00ff nargs=0\n") != NULL,
        "an address with no metadata carries no name field");
  hydra_trace_close();

  /* --- 6. the counters --- */
  hydra_run_stats_t stats;
  CHECK(hydra_machine_stats(&stats) == 0, "the run statistics are readable");
  CHECK(stats.calls >= 1, "the call counter moved");
  CHECK(stats.returns >= 1, "the return counter moved");
  CHECK(stats.native_calls >= 2, "the native call counter moved");
  CHECK(stats.native_returns >= 1, "the native return counter moved");
  CHECK(stats.steps >= 2, "the step counter moved");
  CHECK(stats.interrupts >= 1, "the interrupt counter moved");

  /* --- 7. a file sink writes the same text --- */
  const char *path = "/tmp/hydra-trace-test.trace";
  if (hydra_trace_open(path) == 0) {
    hydra_trace_step(0x0080, 0x0200);
    hydra_trace_close();

    FILE *f = fopen(path, "r");
    char buf[512] = {0};
    if (f) {
      size_t n = fread(buf, 1, sizeof(buf) - 1, f);
      buf[n] = '\0';
      fclose(f);
    }
    CHECK(strstr(buf, "# hydra trace v1\n") != NULL &&
          strstr(buf, "step cs=0080 ip=0200\n") != NULL,
          "the file sink writes the same format");
    remove(path);
  } else {
    CHECK(0, "the file sink opens");
  }

  if (g_failures) {
    printf("FAILURES: %d\n", g_failures);
    return 1;
  }
  printf("ALL TESTS PASSED\n");
  return 0;
}
