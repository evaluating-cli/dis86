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

#include <fcntl.h>
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
 * dispatch table directly, so a broken seam cannot pass.
 *
 * Coverage note: every test here takes the !hydra_raw_code_available() arm,
 * because the native host never reserves a raw-code region. The other arm -
 * reserved region, i.e. a guest CPU - cannot be reached from this process:
 * hydra_impl_raw_code() hands the snippet to the guest via hydra_impl_call_far()
 * and blocks. That arm is covered end to end by the dosemu CI lane
 * (dosemu_host/test_driver.c: INT(0x28), raw_code_runs == raw_code_returns). */

static hydra_machine_t *g_probe_m;

static void probe_impl_unhandled(void)
{
  g_probe_m->registers->ax = 0x0000;
  hydra_impl_int(0x22);
}

/* Status a forked child exits with when the probe did *not* end the process.
 * It must differ from every status a test checks for: with 0 here, "exited
 * with status 0" and "never exited at all" were the same outcome, so the
 * AL=0 assertion passed even with the exit path removed entirely. */
#define PROBE_NO_EXIT_STATUS 0xFE

/* Exit code for probe_impl_exit, so one probe can check that AL is what the
 * core reports. */
static int g_probe_exit_code;

/* What hydra_impl_int() returned in the child. The parent reads it over a
 * pipe rather than through the exit status: the previous contract ended the
 * process from inside the hook, so a child that exited with the code would
 * look exactly like a child that reported the code and carried on. */
static int g_probe_int_result = -1;

/* The result travels as an int, not as a byte: the exit code is AL, so every
 * byte value is a legal exit code (0..255) and none of them can serve as a
 * sentinel for "the interrupt was serviced and the guest continues". The
 * primitive returns -1 for that case, and -1 is not a legal exit code. */

static void probe_impl_exit(void)
{
  /* AH=4Ch (terminate), AL = exit code */
  g_probe_m->registers->ax = 0x4c00u | (uint8_t)g_probe_exit_code;
  g_probe_int_result = hydra_impl_int(0x21);
}

/* A serviced interrupt: the guest continues, so the primitive says so. */
static void probe_impl_serviced(void)
{
  g_probe_m->registers->ax = 0x0200;     /* INT 21h AH=02h */
  g_probe_int_result = hydra_impl_int(0x21);
}

/* The child must survive the interrupt and report what the primitive
 * returned. Both halves matter: the byte proves the code, and the sentinel
 * exit status proves the hook did not end the process — the core reports a
 * termination now, it does not act on it. */
static int probe_int_reports(void (*probe)(void), int expected)
{
  int fds[2];
  if (pipe(fds) != 0)
    return 0;
  fflush(stdout);
  int stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
  pid_t pid = fork();
  if (pid < 0) {
    close(fds[0]);
    close(fds[1]);
    return 0;
  }
  if (pid == 0) {
    close(fds[0]);
    probe();
    int result = g_probe_int_result;
    ssize_t n = write(fds[1], &result, sizeof result);
    (void)n;
    fflush(stdout);
    _exit(PROBE_NO_EXIT_STATUS);        /* the hook must not end the process */
  }
  close(fds[1]);
  int got = 0;
  ssize_t n = read(fds[0], &got, sizeof got);  /* ends when the child closes it */
  close(fds[0]);
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  if (stdin_flags >= 0)
    fcntl(STDIN_FILENO, F_SETFL, stdin_flags);
  return n == (ssize_t)sizeof got && got == expected &&
         WIFEXITED(status) && WEXITSTATUS(status) == PROBE_NO_EXIT_STATUS;
}

/* --- does the teardown actually run? ------------------------------------ */

/* The claim that terminal state is restored on the way out of a terminating
 * guest used to rest on inspection alone: nothing asserted that the atexit
 * handler ran. This backend exists so the parent can observe it — shutdown
 * writes a byte down a pipe the parent is reading.
 *
 * It is installed with hydra_native_backend_install_probe because the
 * by-name registry has no way to name a backend that only exists here. */
static int g_atexit_pipe = -1;

static void atexit_probe_shutdown(void *state)
{
  (void)state;
  if (g_atexit_pipe >= 0) {
    const char byte = 'X';
    ssize_t n = write(g_atexit_pipe, &byte, 1);
    (void)n;
  }
}

static const hydra_backend_t ATEXIT_PROBE_BACKEND = {
  .name     = "atexit-probe",
  .shutdown = atexit_probe_shutdown,
};

/* Child side: swap in the probe backend, terminate as a guest would, and
 * then exit the way a host driver does - with the guest's exit code. The
 * termination itself no longer ends the process: the core reports it and
 * the driver decides. */
static int g_atexit_exit_code;

static void probe_impl_exit_atexit_probe(void)
{
  hydra_native_backend_install_probe(&ATEXIT_PROBE_BACKEND, NULL);
  g_probe_m->registers->ax = 0x4c10u;      /* AH=4Ch, AL=0x10 */
  g_atexit_exit_code = hydra_impl_int(0x21);
}

/* Parent side: the child must exit with AL (0x10) *and* the handler must have
 * written its byte. Either one alone is not proof — a child that returned
 * normally would exit with the sentinel, and a child that ran no teardown
 * would close the pipe with nothing written. */
static int probe_atexit_ran(void)
{
  int fds[2];
  if (pipe(fds) != 0)
    return 0;
  fflush(stdout);
  pid_t pid = fork();
  if (pid < 0) {
    close(fds[0]);
    close(fds[1]);
    return 0;
  }
  if (pid == 0) {
    close(fds[0]);
    g_atexit_pipe = fds[1];
    probe_impl_exit_atexit_probe();
    fflush(stdout);
    /* exit(), not _exit(): a driver leaves through the normal exit path, and
     * that is the path the atexit teardown hangs off. */
    exit(g_atexit_exit_code < 0 ? PROBE_NO_EXIT_STATUS : g_atexit_exit_code);
  }
  close(fds[1]);
  char byte = 0;
  ssize_t n = read(fds[0], &byte, 1);      /* ends when the child closes it */
  close(fds[0]);
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  return n == 1 && byte == 'X' &&
         WIFEXITED(status) && WEXITSTATUS(status) == 0x10;
}

/* --- the guest's termination, end to end (issue #114) ---------------------- */

/* The guest the issue names: B8 05 4C CD 21 (mov ax, 0x4c05; int 21h). The
 * hook body below is its decompiled form, plus a marker statement after the
 * INT that must never run: INT 21h AH=4Ch does not return, and whatever the
 * decompiler emitted after it is code the guest never meant to reach. */
static int g_post_int_ran;

static hydra_result_t probe_terminating_hook(hydra_machine_t *m)
{
  AX = 0x4c05;
  INT(0x21);
  g_post_int_ran = 1;
  return HYDRA_RESULT_RESUME();
}

/* Status the child exits with when one of its own checks fails, so the
 * parent can tell "the guest exited with 5" from "the driver's check
 * failed". */
#define PROBE_DRIVER_FAILED 0xE0

/* The driver: run the machine until it stops, then report and exit with the
 * guest's code. The exec loop must report the termination by itself - one
 * call, no livelock cap - and the code after the INT must not have run. */
static void probe_guest_termination(void)
{
  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};

  hydra_machine_init(m.hardware, &audio, "native|code_load=0|data_seg=0");
  hydra_impl_register_addr(probe_terminating_hook, 0, 0, 0);

  m.registers->cs = 0;
  m.registers->ip = 0;
  m.registers->ds = 0;
  m.registers->es = 0;
  m.registers->ss = 0;
  m.registers->sp = 0x100;

  int calls = 0;
  int ret = HYDRA_EXEC_RESUME;
  while (calls < 10) {
    ret = hydra_machine_exec(&m, 0);

    /* Hydra's callstack bookkeeping is a two-phase event: the exec records
     * it and the host must drain it with notify() (the dosemu host driver
     * does this right after every exec). */
    hydra_machine_notify(&m);

    calls++;
    if (ret != HYDRA_EXEC_REDIRECT)
      break;
  }

  /* One call is structural, not a coincidence: the hook is registered at
   * 0:0 and the machine starts at CS:IP = 0:0, so the very first dispatch is
   * the terminating one. A second call would mean the guest kept running
   * after it had already asked to terminate. */
  int code = hydra_machine_exit_code();
  if (ret != HYDRA_EXEC_TERMINATED || code != 0x05 || g_post_int_ran || calls != 1)
    _exit(PROBE_DRIVER_FAILED);

  /* The driver reports first and exits with the guest's code afterwards. */
  _exit(code);
}

/* Parent side: the child must exit with the guest's exit code (5). */
static int probe_guest_terminates(void)
{
  fflush(stdout);
  int stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
  pid_t pid = fork();
  if (pid < 0)
    return 0;
  if (pid == 0) {
    probe_guest_termination();
    fflush(stdout);
    _exit(PROBE_DRIVER_FAILED);           /* the driver must have exited */
  }
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  if (stdin_flags >= 0)
    fcntl(STDIN_FILENO, F_SETFL, stdin_flags);
  return WIFEXITED(status) && WEXITSTATUS(status) == 0x05;
}

/* Finding 5: with no execution context installed, a primitive used to
 * dereference NULL and die with SIGSEGV. It must fail loud (SIGABRT with a
 * diagnostic) instead; probe_aborts() rejects any other signal. The clear is
 * local to the forked child, so the parent keeps its context. */
static void probe_impl_no_context(void)
{
  execution_context_set(NULL);
  hydra_impl_int(0x10);
}

/* nop needs no machine, so unlike the primitives above it must stay a silent
 * no-op with no context installed; only the guard's fail-loud path is new. */
static void probe_impl_nop_no_context(void)
{
  execution_context_set(NULL);
  hydra_impl_nop();
}

/* Fork into a probe that must exit normally with a specific status. Covers
 * guest termination, where INT 21h AH=4Ch ends the process with the code from
 * AL, and the no-op path, which must *not* exit and so lands on the sentinel.
 *
 * stdin's status flags are saved and restored around the fork because fcntl
 * flags live on the open file description, which fork() shares. A child that
 * exits through the exit path runs the backend's atexit handler, which
 * restores terminal state — and would otherwise clear O_NONBLOCK on the
 * *parent's* stdin while the parent's terminal state still believed it was
 * set, so a later key read would block instead of returning EAGAIN. */
static int probe_exits_with(void (*probe)(void), int expected)
{
  fflush(stdout);
  int stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
  pid_t pid = fork();
  if (pid < 0)
    return 0;
  if (pid == 0) {
    probe();
    fflush(stdout);
    _exit(PROBE_NO_EXIT_STATUS);
  }
  int status = 0;
  if (waitpid(pid, &status, 0) != pid)
    return 0;
  if (stdin_flags >= 0)
    fcntl(STDIN_FILENO, F_SETFL, stdin_flags);
  return WIFEXITED(status) && WEXITSTATUS(status) == expected;
}

int hydra_backend_linktest(hydra_machine_t *m)
{
  t_failures = 0;
  int context_installed = 0;

  TCHECK(hydra_user_int_hook != NULL,
         "wiring: hydra_user_int hook resolved by dlsym");
  if (!hydra_user_int_hook)
    goto done;

  /* hydra_impl_int finds the machine through the active execution context,
   * which is NULL outside a running hook. Install one aliasing this machine
   * so the primitive can be exercised directly. */
  hydra_exec_ctx_t exec;
  memset(&exec, 0, sizeof(exec));
  exec.machine = *m;
  execution_context_set(&exec);
  context_installed = 1;

  hydra_native_backend_select("terminal");
  void *st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&st);
  TCHECK(be != NULL && strcmp(be->name, "terminal") == 0,
         "wiring: terminal backend selected through the registry");
  if (!be)
    goto done;

  uint8_t rows = 0, cols = 0;
  const uint8_t *buf = be->text_buffer(st, &rows, &cols);
  TCHECK(buf != NULL && cols > 0, "wiring: backend exposes a shadow buffer");
  if (!buf)
    goto done;

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
  /* INT 21h AH=4Ch reports the exit code from AL; it does not end the
   * process. Check a distinctive non-zero value as well as zero, so a
   * hardcoded value could not pass by accident. */
  g_probe_exit_code = 0x42;
  TCHECK(probe_int_reports(probe_impl_exit, 0x42),
         "wiring: INT 21h AH=4Ch reports the code from AL");
  g_probe_exit_code = 0x00;
  TCHECK(probe_int_reports(probe_impl_exit, 0x00),
         "wiring: INT 21h AH=4Ch reports 0 when AL is 0");
  /* 255 is a legal exit code, and every byte value is one, so the report
   * must not be packed into a byte with a sentinel: this case is what a
   * single-byte protocol would have confused with "the guest continues". */
  g_probe_exit_code = 0xFF;
  TCHECK(probe_int_reports(probe_impl_exit, 0xFF),
         "wiring: INT 21h AH=4Ch reports 255 (every byte is a legal code)");
  /* A serviced interrupt says so: the guest continues. -1 is not a legal
   * exit code, so the two cannot be confused. */
  TCHECK(probe_int_reports(probe_impl_serviced, -1),
         "wiring: a serviced interrupt continues the guest");
  TCHECK(probe_aborts(probe_impl_no_context),
         "wiring: primitive with no execution context aborts, not SIGSEGV");
  /* Expects the sentinel: nop must not end the process. Expecting 0 here would
   * make this pass whether or not nop ever exits. */
  TCHECK(probe_exits_with(probe_impl_nop_no_context, PROBE_NO_EXIT_STATUS),
         "wiring: nop with no execution context stays a no-op");
  /* The teardown itself, not just the status: a driver that exits after a
   * guest termination must run the backend's atexit handler. */
  TCHECK(probe_atexit_ran(),
         "wiring: guest termination runs the backend atexit teardown");
  /* End to end: the exec loop reports the termination to the driver, which
   * can then exit with the guest's code - and the guest's post-INT code did
   * not run. */
  TCHECK(probe_guest_terminates(),
         "wiring: the driver observes the termination and the guest's code");

done:
  /* Leave the execution context as we found it, including on an early exit. */
  if (context_installed)
    execution_context_set(NULL);
  return t_failures;
}
