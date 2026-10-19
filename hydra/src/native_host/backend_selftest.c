/* Self-tests for the device HAL, terminal backend and INT dispatch skeleton
 * (issue #95). Invoked from test_native_host.c so the native host keeps a
 * single main() (every native-host source links into one test binary).
 *
 * Covers: backend lookup, text-mode accept / graphics-mode reject (the
 * fail-closed contract), 40-column geometry, teletype into the shadow buffer,
 * tick scaling, the implemented INT functions (including INT 16h peek /
 * consume), and — via fork — that both classes of unimplemented service abort
 * rather than silently returning.
 */

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "hydra_backend.h"
#include "hydra_machine.h"
#include "internal.h"

static int t_failures;

#define TCHECK(cond, label) do {                        \
  if (cond) printf("PASS: %s\n", (label));              \
  else { printf("FAIL: %s\n", (label)); t_failures++; } \
} while (0)

/* State shared with the forked probes: a probe either aborts (expected) or
 * falls through and exits 0 (a failure). */
static native_host_ctx_t *g_probe_ctx;
static const hydra_backend_t *g_probe_be;
static void *g_probe_st;

/* Deterministic input backend for the INT 16h tests: poll peeks, read
 * consumes, so the key-present path does not depend on a tty. */
typedef struct {
  uint16_t key;
  int pending;
} fake_input_t;

static int fake_poll_key(void *state, uint16_t *key)
{
  fake_input_t *in = (fake_input_t *)state;
  if (!in->pending)
    return 0;
  *key = in->key;
  return 1;
}

static int fake_read_key(void *state, uint16_t *key)
{
  fake_input_t *in = (fake_input_t *)state;
  if (!in->pending)
    return 0;
  *key = in->key;
  in->pending = 0;
  return 1;
}

static const hydra_backend_t FAKE_INPUT_BACKEND = {
  .name     = "test-input",
  .poll_key = fake_poll_key,
  .read_key = fake_read_key,
};

static void probe_unhandled_vector(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0000;                 /* AH=00h */
  native_int_dispatch(g_probe_ctx, &regs, 0x22, g_probe_be, g_probe_st);
}

static void probe_unhandled_ah(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x3500;                 /* AH=35h: registered vector, no handler */
  native_int_dispatch(g_probe_ctx, &regs, 0x21, g_probe_be, g_probe_st);
}

static void probe_graphics_mode(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0013;                 /* AH=00h, AL=13h (VGA 320x200x256) */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static int probe_aborts(void (*probe)(void))
{
  fflush(stdout);
  pid_t pid = fork();
  if (pid < 0)
    return 0;
  if (pid == 0) {
    probe();
    fflush(stdout);
    _exit(0);
  }
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  return WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT;
}

int hydra_backend_selftest(void)
{
  t_failures = 0;

  /* --- backend registry ------------------------------------------------- */
  const hydra_backend_t *be = hydra_backend_by_name("terminal");
  TCHECK(be != NULL, "backend registry: \"terminal\" resolves");
  TCHECK(be && strcmp(be->name, "terminal") == 0, "backend reports its own name");
  TCHECK(hydra_backend_by_name("sdl2") == NULL,
         "backend registry: \"sdl2\" absent in this lane (optional dep)");
  TCHECK(hydra_backend_by_name(NULL) == hydra_backend_terminal(),
         "backend registry: NULL selects the terminal backend");
  if (!be)
    return t_failures;

  /* --- init ------------------------------------------------------------- */
  void *st = NULL;
  TCHECK(be->init(&st, "") == 0 && st != NULL, "terminal backend init");
  if (!st)
    return t_failures;

  /* --- video modes: text yes, graphics no ------------------------------- */
  uint8_t rows = 0, cols = 0;
  TCHECK(be->set_video_mode(st, 0x03) == 0, "video mode 03h (text) accepted");
  TCHECK(be->set_video_mode(st, 0x07) == 0, "video mode 07h (mono text) accepted");
  TCHECK(be->set_video_mode(st, 0x13) < 0, "video mode 13h (VGA graphics) refused");
  TCHECK(be->set_video_mode(st, 0x03) == 0, "back to text mode after a refusal");

  /* 40-column text modes must report (and wrap at) 40 columns. */
  TCHECK(be->set_video_mode(st, 0x00) == 0, "video mode 00h (40-column text) accepted");
  be->text_buffer(st, &rows, &cols);
  TCHECK(rows == BACKEND_TEXT_ROWS && cols == 40,
         "video mode 00h reports 40x25 geometry");
  TCHECK(be->set_video_mode(st, 0x01) == 0, "video mode 01h (40-column text) accepted");
  be->text_buffer(st, &rows, &cols);
  TCHECK(cols == 40, "video mode 01h reports 40-column geometry");
  TCHECK(be->set_video_mode(st, 0x03) == 0, "back to 80-column text mode");

  /* --- teletype into the shadow buffer ---------------------------------- */
  be->teletype(st, 'H', 0x07);
  be->teletype(st, 'i', 0x07);
  be->teletype(st, '\n', 0x07);
  be->teletype(st, '!', 0x07);

  const uint8_t *buf = be->text_buffer(st, &rows, &cols);
  TCHECK(buf != NULL && rows == BACKEND_TEXT_ROWS && cols == BACKEND_TEXT_COLS,
         "shadow buffer reports 80x25 geometry");
  TCHECK(buf && buf[0] == 'H' && buf[1] == 'i', "teletype writes into row 0");
  TCHECK(buf && buf[cols + 0] == '!' && buf[cols + 1] == ' ',
         "newline advances to row 1 (later cells stay blank)");

  /* --- ticks ------------------------------------------------------------ */
  uint32_t t0 = be->ticks(st);
  TCHECK(t0 == 0 || t0 < 100000u, "tick counter starts near zero");
  TCHECK(be->ticks(st) >= t0, "tick counter is monotonic");

  /* --- INT dispatch: implemented surface -------------------------------- */
  native_host_ctx_t *ctx = (native_host_ctx_t *)calloc(1, sizeof(*ctx));
  TCHECK(ctx != NULL, "guest-memory context allocated for dispatch tests");
  if (!ctx) {
    be->shutdown(st);
    return t_failures;
  }

  /* INT 10h AH=0Eh: teletype. */
  {
    hydra_machine_registers_t regs = { 0 };
    regs.ax = 0x0e58;               /* AH=0Eh, AL='X' */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0, "INT 10h AH=0Eh handled");
    const uint8_t *b = be->text_buffer(st, NULL, NULL);
    TCHECK(b && b[cols + 1] == 'X', "INT 10h AH=0Eh reached the backend");
  }

  /* INT 21h AH=02h: print the character in DL. */
  {
    hydra_machine_registers_t regs = { 0 };
    regs.ax = 0x0200;
    regs.dx = 0x0059;               /* 'Y' */
    TCHECK(native_int_dispatch(ctx, &regs, 0x21, be, st) == 0, "INT 21h AH=02h handled");
    TCHECK((regs.ax & 0xff) == 0x59, "INT 21h AH=02h returns the character in AL");
    const uint8_t *b = be->text_buffer(st, NULL, NULL);
    TCHECK(b && b[cols + 2] == 'Y', "INT 21h AH=02h reached the backend");
  }

  /* INT 21h AH=09h: '$'-terminated string at DS:DX. */
  {
    hydra_machine_registers_t regs = { 0 };
    regs.ax = 0x0900;
    regs.ds = 0x0200;
    regs.dx = 0x0000;
    const uint32_t base = (uint32_t)regs.ds * 16u;
    memcpy(ctx->mem + base, "OK$", 3);

    TCHECK(native_int_dispatch(ctx, &regs, 0x21, be, st) == 0, "INT 21h AH=09h handled");
    TCHECK((regs.ax & 0xff) == 0x24, "INT 21h AH=09h returns '$' in AL");
    const uint8_t *b = be->text_buffer(st, NULL, NULL);
    TCHECK(b && b[cols + 3] == 'O' && b[cols + 4] == 'K',
           "INT 21h AH=09h printed the guest string");
  }

  /* INT 1Ah AH=00h: tick count in CX:DX. */
  {
    hydra_machine_registers_t regs = { 0 };
    regs.ax = 0x0000;
    TCHECK(native_int_dispatch(ctx, &regs, 0x1a, be, st) == 0, "INT 1Ah AH=00h handled");
    uint32_t got = ((uint32_t)regs.cx << 16) | regs.dx;
    TCHECK(got >= t0, "INT 1Ah AH=00h returns a tick count consistent with the backend");
  }

  /* INT 16h: AH=01h peeks, AH=00h consumes. Use the deterministic fake input
   * backend so the key-present path is covered without a tty. */
  {
    fake_input_t in = { 0 };
    hydra_machine_registers_t regs = { 0 };

    regs.ax = 0x0100;
    TCHECK(native_int_dispatch(ctx, &regs, 0x16, &FAKE_INPUT_BACKEND, &in) == 0,
           "INT 16h AH=01h handled");
    TCHECK((regs.flags & (1u << 6)) != 0,
           "INT 16h AH=01h sets ZF when no key is pending");

    in.key = 0x1e61;                /* 'a': scan 1Eh, ASCII 61h */
    in.pending = 1;
    regs.ax = 0x0100;
    regs.flags = 0x0000;
    TCHECK(native_int_dispatch(ctx, &regs, 0x16, &FAKE_INPUT_BACKEND, &in) == 0,
           "INT 16h AH=01h handled with a key pending");
    TCHECK(regs.ax == 0x1e61 && (regs.flags & (1u << 6)) == 0,
           "INT 16h AH=01h returns the key word and clears ZF");

    regs.ax = 0x0100;
    regs.flags = 0x0000;
    native_int_dispatch(ctx, &regs, 0x16, &FAKE_INPUT_BACKEND, &in);
    TCHECK(regs.ax == 0x1e61 && in.pending,
           "INT 16h AH=01h peeks the same key twice");

    regs.ax = 0x0000;
    TCHECK(native_int_dispatch(ctx, &regs, 0x16, &FAKE_INPUT_BACKEND, &in) == 0,
           "INT 16h AH=00h handled");
    TCHECK(regs.ax == 0x1e61 && !in.pending,
           "INT 16h AH=00h returns and consumes the key");

    regs.ax = 0x0100;
    regs.flags = 0x0000;
    native_int_dispatch(ctx, &regs, 0x16, &FAKE_INPUT_BACKEND, &in);
    TCHECK((regs.flags & (1u << 6)) != 0,
           "INT 16h AH=01h sets ZF after the key is consumed");
  }

  /* INT 21h AH=4Ch: exit request. */
  {
    hydra_machine_registers_t regs = { 0 };
    regs.ax = 0x4c00;
    TCHECK(native_int_dispatch(ctx, &regs, 0x21, be, st) == NATIVE_INT_EXIT,
           "INT 21h AH=4Ch signals exit");
  }

  /* --- fail-loud: both classes of missing service must abort ------------ */
  g_probe_ctx = ctx;
  g_probe_be = be;
  g_probe_st = st;

  TCHECK(probe_aborts(probe_unhandled_vector),
         "unhandled INT vector aborts (fail loud)");
  TCHECK(probe_aborts(probe_unhandled_ah),
         "unhandled AH inside a registered vector aborts (fail loud)");
  TCHECK(probe_aborts(probe_graphics_mode),
         "graphics mode on the terminal backend aborts (fail closed)");

  free(ctx);
  be->shutdown(st);
  return t_failures;
}

/********************************************************************/
/* Wiring: hydra_impl_int -> raw-code guard -> dlsym hook -> dispatch ->
 * backend. These run against a live machine rather than against the
 * dispatch table directly, so a broken seam cannot pass. */

static hydra_machine_t *g_probe_m;

static void probe_impl_unhandled(void)
{
  g_probe_m->registers->ax = 0x0000;
  hydra_impl_int(0x22);
}

static void probe_impl_exit(void)
{
  g_probe_m->registers->ax = 0x4c00;      /* INT 21h AH=4Ch, exit code 0 */
  hydra_impl_int(0x21);
}

int hydra_backend_linktest(hydra_machine_t *m)
{
  t_failures = 0;

  TCHECK(hydra_user_int_hook != NULL,
         "wiring: hydra_user_int hook resolved by dlsym");
  if (!hydra_user_int_hook)
    return t_failures;

  /* hydra_impl_int finds the machine through the active execution context,
   * which is NULL outside a running hook. Install one aliasing this machine
   * so the primitive can be exercised directly. */
  hydra_exec_ctx_t exec;
  memset(&exec, 0, sizeof(exec));
  exec.machine = *m;
  execution_context_set(&exec);

  hydra_native_backend_select("terminal");
  void *st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&st);
  TCHECK(be != NULL && strcmp(be->name, "terminal") == 0,
         "wiring: terminal backend selected through the registry");
  if (!be)
    return t_failures;

  uint8_t rows = 0, cols = 0;
  const uint8_t *buf = be->text_buffer(st, &rows, &cols);
  TCHECK(buf != NULL && cols > 0, "wiring: backend exposes a shadow buffer");
  if (!buf)
    return t_failures;

  /* Registers are read from the execution context's copy of the machine. */
  exec.machine.registers->ax = 0x0e41;     /* INT 10h AH=0Eh, AL='A' */
  hydra_impl_int(0x10);
  TCHECK(buf[0] == 'A',
         "wiring: INT 10h AH=0Eh via hydra_impl_int reaches the backend");

  exec.machine.registers->ax = 0x0200;     /* INT 21h AH=02h */
  exec.machine.registers->dx = 0x0042;     /* DL='B' */
  hydra_impl_int(0x21);
  TCHECK(buf[1] == 'B',
         "wiring: INT 21h AH=02h via hydra_impl_int reaches the backend");

  /* A string read out of guest memory proves the host ctx is threaded
   * through, not just the registers. */
  exec.machine.registers->ax = 0x0900;     /* INT 21h AH=09h */
  exec.machine.registers->ds = 0x0200;
  exec.machine.registers->dx = 0x0000;
  {
    native_host_ctx_t *ctx = (native_host_ctx_t *)exec.machine.hardware->ctx;
    memcpy(ctx->mem + (0x0200u * 16u), "CD$", 3);
  }
  hydra_impl_int(0x21);
  TCHECK(buf[2] == 'C' && buf[3] == 'D',
         "wiring: INT 21h AH=09h reads guest memory through the host ctx");

  /* Both classes of failure still abort when reached through the primitive. */
  g_probe_m = &exec.machine;
  TCHECK(probe_aborts(probe_impl_unhandled),
         "wiring: unhandled vector via hydra_impl_int aborts");
  TCHECK(probe_aborts(probe_impl_exit),
         "wiring: guest termination aborts with a diagnostic (no exit path yet)");

  /* Leave the execution context as we found it. */
  execution_context_set(NULL);
  return t_failures;
}
