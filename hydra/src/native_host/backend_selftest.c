/* Self-tests for the device HAL, terminal backend and INT dispatch skeleton
 * (issue #95). Invoked from test_native_host.c so the native host keeps a
 * single main() (every native-host source links into one test binary).
 *
 * Covers: backend lookup, text-mode accept / graphics-mode reject (the
 * fail-closed contract), 40-column geometry, teletype into the shadow buffer,
 * tick scaling, the implemented INT functions (including the INT 10h text
 * services and INT 16h peek / consume), and — via fork — that unimplemented
 * services and out-of-range text requests abort rather than silently
 * returning.
 */

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
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

/* The live hardware whose memory the later probes touch, and the snapshot the
 * restore probe reads. */
static hydra_machine_hardware_t *g_restore_hw;
static const char *g_restore_path;

/* A backend that renders nothing: no video methods at all. Installed in a
 * forked child so the probe cannot disturb this process's registry. */
static const hydra_backend_t NO_TEXT_BACKEND = { .name = "test-no-text" };
/* Before any backend is selected. The store must land in guest memory and the
 * projection must stay silent: aborting here would turn a valid guest store
 * into a process exit. present() is checked too, since it shares the rule. */
static void probe_store_without_backend(void)
{
  hydra_native_backend_clear_probe();
  hydra_native_backend_present();                    /* must be a no-op */
  g_restore_hw->mem_write16(g_restore_hw->ctx, 0xB8000u, 0x0741u);
  g_restore_hw->mem_write8(g_restore_hw->ctx, 0xB8001u, 0x1e);
  if (g_restore_hw->mem_read16(g_restore_hw->ctx, 0xB8000u) != 0x1e41u)
    _exit(1);
}

static void probe_store_without_text_backend(void)
{
  hydra_native_backend_install_probe(&NO_TEXT_BACKEND, NULL);
  g_restore_hw->mem_write16(g_restore_hw->ctx, 0xB8000u, 0x0741u);
  g_restore_hw->mem_write8(g_restore_hw->ctx, 0xB8001u, 0x1e);
  /* write16 puts 'A' + 07h, then write8 replaces the attribute: the bytes are
   * there, which is all this probe asserts. */
  if (g_restore_hw->mem_read16(g_restore_hw->ctx, 0xB8000u) != 0x1e41u)
    _exit(1);
}


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

static void probe_cursor_page_one(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0200;
  regs.bx = 0x0100;                 /* BH=1: page 1 */
  regs.dx = 0x0000;
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static void probe_cursor_out_of_range(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0200;
  regs.dx = 0x1900;                 /* row 25 in a 25-row window */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static void probe_scroll_window_out_of_range(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0601;
  regs.bx = 0x0700;                 /* blank attribute */
  regs.cx = 0x0000;                 /* top 0, left 0 */
  regs.dx = 0x1900;                 /* bottom 25 */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static void probe_write_page_one(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0941;                 /* AH=09h, AL='A' */
  regs.bx = 0x011e;                 /* BH=1: page 1 */
  regs.cx = 1;
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static void probe_teletype_page_one(void)
{
  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0e41;                 /* AH=0Eh, AL='A' */
  regs.bx = 0x0107;                 /* BH=1: page 1 */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

/* A run starting at the last row's penultimate cell would write past the
 * visible window, which this host does not model: both character writers must
 * fail loud rather than clip or wrap somewhere the guest cannot see. */
static void probe_write_run_out_of_range(void)
{
  hydra_machine_registers_t regs = { 0 };
  g_probe_be->set_cursor(g_probe_st, BACKEND_TEXT_ROWS - 1,
                         BACKEND_TEXT_COLS - 2);
  regs.ax = 0x0941;                 /* AH=09h, AL='A' */
  regs.bx = 0x001e;                 /* page 0, attribute 1Eh */
  regs.cx = 3;                      /* one cell past the last one */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
}

static void probe_write_char_run_out_of_range(void)
{
  hydra_machine_registers_t regs = { 0 };
  g_probe_be->set_cursor(g_probe_st, BACKEND_TEXT_ROWS - 1,
                         BACKEND_TEXT_COLS - 2);
  regs.ax = 0x0a41;                 /* AH=0Ah, AL='A' */
  regs.cx = 3;
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

/* Run `probe` in a forked child with stdout redirected, and return everything
 * the child wrote in a malloc'd NUL-terminated buffer.
 *
 * `use_pty` picks what the child's stdout looks like: a pty makes it a
 * terminal (the backend renders) and a pipe makes it a file (the backend must
 * stay silent). Either way the child inherits the parent's backend state by
 * copy-on-write, so it draws into the same shadow buffer without the parent
 * ever seeing the drawing. Returns 0 on setup or child failure. */
static int capture_stdout(int use_pty, void (*probe)(void), char **out, size_t *out_len)
{
  int read_fd = -1, write_fd = -1;

  if (use_pty) {
    int master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0)
      return 0;
    if (grantpt(master) != 0 || unlockpt(master) != 0) {
      close(master);
      return 0;
    }
    const char *slave_name = ptsname(master);
    if (!slave_name) {
      close(master);
      return 0;
    }
    read_fd = master;
    write_fd = open(slave_name, O_RDWR | O_NOCTTY);
    if (write_fd < 0) {
      close(master);
      return 0;
    }
  } else {
    int fds[2];
    if (pipe(fds) != 0)
      return 0;
    read_fd = fds[0];
    write_fd = fds[1];
  }

  fflush(stdout);
  pid_t pid = fork();
  if (pid < 0) {
    close(read_fd);
    close(write_fd);
    return 0;
  }
  if (pid == 0) {
    close(read_fd);
    if (dup2(write_fd, STDOUT_FILENO) < 0)
      _exit(1);
    close(write_fd);
    probe();
    fflush(stdout);
    _exit(0);
  }

  close(write_fd);

  char *acc = NULL;
  size_t len = 0, cap = 0;
  char buf[4096];
  for (;;) {
    ssize_t n = read(read_fd, buf, sizeof(buf));
    if (n > 0) {
      if (len + (size_t)n + 1 > cap) {
        cap = (len + (size_t)n + 1) * 2;
        char *grown = realloc(acc, cap);
        if (!grown) {
          free(acc);
          close(read_fd);
          return 0;
        }
        acc = grown;
      }
      memcpy(acc + len, buf, (size_t)n);
      len += (size_t)n;
    } else if (n < 0 && errno == EINTR) {
      continue;
    } else {
      break;
    }
  }
  close(read_fd);

  int status = 0;
  if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
    free(acc);
    return 0;
  }
  if (!acc) {
    acc = malloc(1);
    if (!acc)
      return 0;
  }
  acc[len] = '\0';
  *out = acc;
  *out_len = len;
  return 1;
}

/* present() probes. They draw the same character and differ only in how often
 * they present, so identical captures are the assertion. They reuse the
 * g_probe_be / g_probe_st handles the abort probes already carry. */
static void probe_present_once(void)
{
  g_probe_be->teletype(g_probe_st, 'Z', 0x07);
  g_probe_be->present(g_probe_st);
}

static void probe_present_twice(void)
{
  g_probe_be->teletype(g_probe_st, 'Z', 0x07);
  g_probe_be->present(g_probe_st);
  g_probe_be->present(g_probe_st);
}

static void probe_present_never(void)
{
  g_probe_be->teletype(g_probe_st, 'Z', 0x07);
}

/* Count non-overlapping occurrences of `needle` in `hay`. */
static int count_substr(const char *hay, const char *needle)
{
  int n = 0;
  const size_t len = strlen(needle);
  if (len == 0)
    return 0;
  for (const char *at = hay; (at = strstr(at, needle)) != NULL; at += len)
    n++;
  return n;
}

/* Cursor-only changes: drawing is settled by the first present(), then the
 * guest only moves the cursor (CR/LF) and presents again. The second present
 * must emit the cursor escape and must not repaint the cells. */
static void probe_present_cursor_only(void)
{
  g_probe_be->set_video_mode(g_probe_st, 0x03);
  g_probe_be->teletype(g_probe_st, 'A', 0x07);
  g_probe_be->present(g_probe_st);
  g_probe_be->teletype(g_probe_st, '\r', 0x07);
  g_probe_be->teletype(g_probe_st, '\n', 0x07);
  g_probe_be->present(g_probe_st);
}

/* A cursor move through INT 10h is the same kind of change: it must reach the
 * terminal even though no cell was rewritten. */
static void probe_cursor_int_present(void)
{
  g_probe_be->set_video_mode(g_probe_st, 0x03);
  g_probe_be->present(g_probe_st);

  hydra_machine_registers_t regs = { 0 };
  regs.ax = 0x0200;                 /* AH=02h, BH=0 (page 0) */
  regs.dx = 0x0305;                 /* row 3, column 5 */
  native_int_dispatch(g_probe_ctx, &regs, 0x10, g_probe_be, g_probe_st);
  g_probe_be->present(g_probe_st);
}

/* Fill a 3x3 window with distinct cells and put markers just outside it, so
 * a scroll test can tell a window copy from a full-screen one. */
static void seed_scroll_window(const hydra_backend_t *be, void *st)
{
  for (uint8_t r = 1; r <= 3; r++)
    for (uint8_t c = 1; c <= 3; c++)
      be->putchar_text(st, r, c, (uint8_t)('a' + (r - 1) * 3 + (c - 1)), 0x07);
  be->putchar_text(st, 0, 0, 'Z', 0x07);
  be->putchar_text(st, 4, 1, 'Q', 0x07);
  be->set_cursor(st, 10, 20);
}

/* The host-facing wrapper, driven through the selected backend's state so the
 * wrapper and the drawing agree on which terminal this is. An empty wrapper
 * body emits nothing here, so this is what makes its only caller a real test. */
static void probe_wrapper_present(void)
{
  void *st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&st);
  be->teletype(st, 'W', 0x07);
  hydra_native_backend_present();
  hydra_native_backend_present();
}

/* A restore loads the image with one fread, bypassing the per-store mirror,
 * so present() must still have something to say afterwards: the frame has to
 * be re-projected from the restored bytes. Settle the frame first, or a stale
 * dirty flag would make the present emit for the wrong reason. */
static void probe_restore_then_present(void)
{
  void *st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&st);

  be->set_video_mode(st, 0x03);
  hydra_native_backend_present();          /* settle: frame clean */

  g_restore_hw->state_restore(g_restore_hw->ctx, g_restore_path);
  hydra_native_backend_present();
}

/* A guest that draws a title screen and then blocks on a key must have that
 * screen on the terminal *while it waits*. The child gets a pty as stdin and
 * stdout, so read_key() takes its blocking path; the parent reads the frame
 * off the master before it sends the key. A read_key() that does not present
 * first leaves the child silent, the 2 s window expires, and the test fails —
 * it cannot pass on a frame written after the key arrived. */
static int present_before_blocking(void)
{
  int master = posix_openpt(O_RDWR | O_NOCTTY);
  if (master < 0)
    return 0;
  if (grantpt(master) != 0 || unlockpt(master) != 0) {
    close(master);
    return 0;
  }
  const char *slave_name = ptsname(master);
  if (!slave_name) {
    close(master);
    return 0;
  }
  int slave = open(slave_name, O_RDWR | O_NOCTTY);
  if (slave < 0) {
    close(master);
    return 0;
  }

  fflush(stdout);
  pid_t pid = fork();
  if (pid < 0) {
    close(master);
    close(slave);
    return 0;
  }
  if (pid == 0) {
    close(master);
    if (dup2(slave, STDIN_FILENO) < 0 || dup2(slave, STDOUT_FILENO) < 0)
      _exit(1);
    close(slave);
    g_probe_be->set_video_mode(g_probe_st, 0x03);
    g_probe_be->teletype(g_probe_st, 'K', 0x07);
    uint16_t key = 0;
    int ok = g_probe_be->read_key(g_probe_st, &key);
    fflush(stdout);
    _exit(ok && key ? 0 : 1);
  }
  close(slave);

  char seen[4096];
  size_t seen_len = 0;
  int saw_frame = 0;

  /* 2 s of slices: enough for a loaded CI runner, short enough that a
   * regression fails the suite instead of hanging it. */
  for (int tries = 0; tries < 200 && !saw_frame; tries++) {
    struct pollfd pfd = { .fd = master, .events = POLLIN };
    if (poll(&pfd, 1, 10) <= 0)
      continue;
    ssize_t n = read(master, seen + seen_len, sizeof(seen) - 1 - seen_len);
    if (n <= 0)
      break;
    seen_len += (size_t)n;
    seen[seen_len] = '\0';
    if (strstr(seen, "\033[H\033[2J") && strchr(seen, 'K'))
      saw_frame = 1;
  }

  /* Sent after the loop either way, so a failing child still exits. The
   * newline is what makes a canonical-mode pty slave deliver the line: a bare
   * byte would sit in the input queue and the child would block forever. */
  if (write(master, "a\n", 2) < 0) {
    saw_frame = 0;
    /* Closing the master is EOF on the slave, so a child that never got the
     * key still leaves read_key() instead of wedging the whole run. */
    close(master);
    master = -1;
  }

  int status = 0;
  waitpid(pid, &status, 0);
  if (master >= 0)
    close(master);

  return saw_frame && WIFEXITED(status) && WEXITSTATUS(status) == 0;
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

  /* INT 10h text services: cursor, scroll and character/attribute I/O. */
  {
    hydra_machine_registers_t regs = { 0 };
    uint8_t ch = 0, attr = 0;
    uint8_t row = 0, col = 0;

    TCHECK(be->set_video_mode(st, 0x03) == 0, "INT 10h text: reset to 80x25");
    TCHECK(be->set_cursor(st, 3, 5) == 0 &&
               be->get_cursor(st, &row, &col) == 0 && row == 3 && col == 5,
           "INT 10h text: cursor accessors round-trip");
    TCHECK(be->set_cursor(st, BACKEND_TEXT_ROWS, 0) < 0 &&
               be->set_cursor(st, 0, BACKEND_TEXT_COLS) < 0,
           "INT 10h text: cursor accessors refuse cells outside the geometry");

    regs.ax = 0x0200;
    regs.dx = 0x050a;               /* row 5, column 10 */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=02h handled");
    regs.ax = 0x0300;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=03h handled");
    TCHECK(regs.dx == 0x050a && regs.cx == 0x0607,
           "INT 10h AH=03h returns the cursor and the default shape");

    regs.ax = 0x0941;               /* AH=09h, AL='A' */
    regs.bx = 0x001e;               /* BH=0 page, BL=1Eh attribute */
    regs.cx = 3;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=09h handled");
    TCHECK(be->get_cell(st, 5, 10, &ch, &attr) == 0 && ch == 'A' && attr == 0x1e &&
               be->get_cell(st, 5, 11, &ch, &attr) == 0 && ch == 'A' &&
               attr == 0x1e &&
               be->get_cell(st, 5, 12, &ch, &attr) == 0 && ch == 'A' &&
               attr == 0x1e,
           "INT 10h AH=09h writes CX cells with the character and attribute");
    TCHECK(be->get_cell(st, 5, 13, &ch, &attr) == 0 && ch == ' ' && attr == 0x07,
           "INT 10h AH=09h stops after CX cells");
    TCHECK(be->get_cursor(st, &row, &col) == 0 && row == 5 && col == 10,
           "INT 10h AH=09h leaves the cursor alone");

    regs.ax = 0x0800;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=08h handled");
    TCHECK(regs.ax == 0x1e41, "INT 10h AH=08h returns character and attribute");

    /* Give the second target a different attribute so AH=0Ah has to keep each
     * cell's own attribute rather than copying the cursor cell's. */
    be->putchar_text(st, 5, 11, 'x', 0x2e);
    regs.ax = 0x0a42;               /* AH=0Ah, AL='B' */
    regs.bx = 0x0000;               /* BL is ignored for AH=0Ah */
    regs.cx = 2;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=0Ah handled");
    TCHECK(be->get_cell(st, 5, 10, &ch, &attr) == 0 && ch == 'B' && attr == 0x1e,
           "INT 10h AH=0Ah writes the cursor cell with its attribute");
    TCHECK(be->get_cell(st, 5, 11, &ch, &attr) == 0 && ch == 'B' && attr == 0x2e,
           "INT 10h AH=0Ah keeps each target cell's attribute");
    TCHECK(be->get_cell(st, 5, 12, &ch, &attr) == 0 && ch == 'A' && attr == 0x1e,
           "INT 10h AH=0Ah stops after CX cells");
    TCHECK(be->get_cursor(st, &row, &col) == 0 && row == 5 && col == 10,
           "INT 10h AH=0Ah leaves the cursor alone");

    regs.ax = 0x0a43;               /* AL='C' */
    regs.cx = 0;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=0Ah with CX=0 handled");
    TCHECK(be->get_cell(st, 5, 10, &ch, &attr) == 0 && ch == 'B',
           "INT 10h AH=0Ah with CX=0 writes nothing");

    /* Repetition wraps at the screen width, not at the cursor's line. */
    TCHECK(be->set_cursor(st, 5, 78) == 0,
           "INT 10h text: cursor to row 5, column 78");
    regs.ax = 0x0944;               /* AH=09h, AL='D' */
    regs.bx = 0x002e;
    regs.cx = 3;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=09h handles a run across a row boundary");
    TCHECK(be->get_cell(st, 5, 78, &ch, &attr) == 0 && ch == 'D' && attr == 0x2e &&
               be->get_cell(st, 5, 79, &ch, &attr) == 0 && ch == 'D' &&
               attr == 0x2e &&
               be->get_cell(st, 6, 0, &ch, &attr) == 0 && ch == 'D' &&
               attr == 0x2e,
           "INT 10h AH=09h wraps the run into the next row");
    TCHECK(be->get_cell(st, 6, 1, &ch, &attr) == 0 && ch == ' ' && attr == 0x07,
           "INT 10h AH=09h stops after CX cells across the wrap");

    /* A run ending exactly on the last cell is still inside the window. */
    TCHECK(be->set_cursor(st, BACKEND_TEXT_ROWS - 1, BACKEND_TEXT_COLS - 2) == 0,
           "INT 10h text: cursor to the last row's penultimate cell");
    regs.ax = 0x0a45;               /* AH=0Ah, AL='E' */
    regs.cx = 2;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=0Ah handles a run ending on the last cell");
    TCHECK(be->get_cell(st, BACKEND_TEXT_ROWS - 1, BACKEND_TEXT_COLS - 2,
                        &ch, &attr) == 0 && ch == 'E' &&
               be->get_cell(st, BACKEND_TEXT_ROWS - 1, BACKEND_TEXT_COLS - 1,
                            &ch, &attr) == 0 && ch == 'E',
           "INT 10h AH=0Ah writes both cells of the boundary run");

    regs.ax = 0x0f00;
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=0Fh handled");
    TCHECK(regs.ax == 0x5003 && (regs.bx >> 8) == 0,
           "INT 10h AH=0Fh returns mode 03h and 80 columns");

    TCHECK(be->set_video_mode(st, 0x01) == 0, "INT 10h text: reset to 40x25");
    regs.ax = 0x0f00;
    native_int_dispatch(ctx, &regs, 0x10, be, st);
    TCHECK(regs.ax == 0x2801, "INT 10h AH=0Fh reports 40 columns in mode 01h");
    TCHECK(be->set_video_mode(st, 0x03) == 0, "INT 10h text: back to 80x25");

    /* Scroll up: rows 1..3 hold a..i; one line moves d..f into row 1 and the
     * last row is blanked with BH. Cells outside the window keep their bytes,
     * and the cursor does not move. */
    seed_scroll_window(be, st);
    regs.ax = 0x0601;               /* AH=06h, AL=1 line */
    regs.bx = 0x1f00;               /* BH=1Fh blank attribute */
    regs.cx = 0x0101;               /* top 1, left 1 */
    regs.dx = 0x0303;               /* bottom 3, right 3 */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=06h handled");
    TCHECK(be->get_cell(st, 1, 1, &ch, &attr) == 0 && ch == 'd' && attr == 0x07,
           "INT 10h AH=06h moves the second row into the first");
    TCHECK(be->get_cell(st, 1, 3, &ch, &attr) == 0 && ch == 'f',
           "INT 10h AH=06h moves the whole row");
    TCHECK(be->get_cell(st, 2, 1, &ch, &attr) == 0 && ch == 'g',
           "INT 10h AH=06h moves the third row up");
    TCHECK(be->get_cell(st, 3, 1, &ch, &attr) == 0 && ch == ' ' && attr == 0x1f,
           "INT 10h AH=06h blanks the bottom row with BH");
    TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'Z',
           "INT 10h AH=06h leaves cells outside the window alone");
    TCHECK(be->get_cell(st, 4, 1, &ch, &attr) == 0 && ch == 'Q',
           "INT 10h AH=06h leaves rows below the window alone");
    TCHECK(be->get_cursor(st, &row, &col) == 0 && row == 10 && col == 20,
           "INT 10h AH=06h leaves the cursor alone");

    /* Scroll down: the mirror image, with the blank row at the top. */
    seed_scroll_window(be, st);
    regs.ax = 0x0701;               /* AH=07h, AL=1 line */
    regs.bx = 0x2e00;               /* BH=2Eh blank attribute */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=07h handled");
    TCHECK(be->get_cell(st, 1, 1, &ch, &attr) == 0 && ch == ' ' && attr == 0x2e,
           "INT 10h AH=07h blanks the top row with BH");
    TCHECK(be->get_cell(st, 2, 1, &ch, &attr) == 0 && ch == 'a',
           "INT 10h AH=07h moves the first row down");
    TCHECK(be->get_cell(st, 3, 3, &ch, &attr) == 0 && ch == 'f',
           "INT 10h AH=07h moves the second row down and clips the window");
    TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'Z',
           "INT 10h AH=07h leaves cells outside the window alone");
    TCHECK(be->get_cursor(st, &row, &col) == 0 && row == 10 && col == 20,
           "INT 10h AH=07h leaves the cursor alone");

    /* AL=0 clears the window instead of shifting it. */
    seed_scroll_window(be, st);
    regs.ax = 0x0600;               /* AH=06h, AL=0: clear */
    regs.bx = 0x7000;               /* BH=70h blank attribute */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=06h with AL=0 handled");
    TCHECK(be->get_cell(st, 1, 1, &ch, &attr) == 0 && ch == ' ' && attr == 0x70 &&
               be->get_cell(st, 3, 3, &ch, &attr) == 0 && ch == ' ' && attr == 0x70,
           "INT 10h AH=06h with AL=0 clears the whole window");
    TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'Z' &&
               be->get_cell(st, 4, 1, &ch, &attr) == 0 && ch == 'Q',
           "INT 10h AH=06h with AL=0 leaves cells outside the window alone");
    TCHECK(be->get_cursor(st, &row, &col) == 0 && row == 10 && col == 20,
           "INT 10h AH=06h with AL=0 leaves the cursor alone");

    /* Scrolling by at least the window height also clears it. */
    seed_scroll_window(be, st);
    regs.ax = 0x0605;               /* AL=5, more than the 3-row window */
    regs.bx = 0x6000;               /* BH=60h blank attribute */
    TCHECK(native_int_dispatch(ctx, &regs, 0x10, be, st) == 0,
           "INT 10h AH=06h with AL >= window height handled");
    TCHECK(be->get_cell(st, 1, 1, &ch, &attr) == 0 && ch == ' ' && attr == 0x60,
           "INT 10h AH=06h with AL >= window height clears the window");
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

  /* --- text geometry and cell reads (host-side video mirroring) --------- */
  {
    hydra_text_geometry_t geo;
    uint8_t ch = 0, attr = 0;

    TCHECK(be->set_video_mode(st, 0x03) == 0 && be->text_geometry(st, &geo) == 0 &&
               geo.base == 0xB8000u && geo.mode == 0x03 &&
               geo.rows == BACKEND_TEXT_ROWS && geo.cols == BACKEND_TEXT_COLS,
           "text_geometry: mode 03h renders the 80x25 window at B8000h");

    TCHECK(be->set_video_mode(st, 0x01) == 0 && be->text_geometry(st, &geo) == 0 &&
               geo.base == 0xB8000u && geo.cols == 40,
           "text_geometry: mode 01h keeps B8000h and reports 40 columns");

    TCHECK(be->set_video_mode(st, 0x07) == 0 && be->text_geometry(st, &geo) == 0 &&
               geo.base == 0xB0000u && geo.rows == BACKEND_TEXT_ROWS && geo.cols == 80,
           "text_geometry: mode 07h moves the window to B0000h");

    TCHECK(be->set_video_mode(st, 0x03) == 0, "text_geometry: back to 80x25 text mode");

    be->putchar_text(st, 5, 7, 'Q', 0x1e);
    TCHECK(be->get_cell(st, 5, 7, &ch, &attr) == 0 && ch == 'Q' && attr == 0x1e,
           "get_cell: reads back an explicit cell write (character and attribute)");
    TCHECK(be->get_cell(st, BACKEND_TEXT_ROWS, 0, &ch, &attr) < 0 &&
               be->get_cell(st, 0, BACKEND_TEXT_COLS, &ch, &attr) < 0,
           "get_cell: refuses cells outside the geometry");
  }

  /* --- present(): repaint only what changed, and only on a terminal ----- */
  {
    char *once = NULL, *twice = NULL, *never = NULL, *headless = NULL;
    size_t once_len = 0, twice_len = 0, never_len = 0, headless_len = 0;

    /* Reset the frame and the cursor so the probes start from a known state;
     * each probe runs in a forked child, so this parent-side reset is what
     * every child sees. */
    be->set_video_mode(st, 0x03);
    g_probe_be = be;
    g_probe_st = st;

    TCHECK(capture_stdout(1, probe_present_once, &once, &once_len),
           "present(): capture on a pty");
    TCHECK(once_len > 0 && strstr(once, "\033[H\033[2JZ") != NULL,
           "present(): a terminal gets the drawn frame");

    TCHECK(capture_stdout(1, probe_present_never, &never, &never_len),
           "present(): capture without a present() call");
    TCHECK(never_len == 0, "present(): drawing alone emits nothing");

    TCHECK(capture_stdout(1, probe_present_twice, &twice, &twice_len),
           "present(): capture of two calls");
    TCHECK(twice_len == once_len && memcmp(twice, once, once_len) == 0,
           "present(): a second call with nothing new emits nothing");

    TCHECK(capture_stdout(0, probe_present_once, &headless, &headless_len),
           "present(): capture on a pipe");
    TCHECK(headless_len == 0, "present(): silent when stdout is not a terminal");

    TCHECK(present_before_blocking(),
           "present(): read_key renders the frame before it blocks on a key");

    free(once);
    free(twice);
    free(never);
    free(headless);
  }

  /* --- present(): a cursor-only change still reaches the terminal ------ */
  {
    char *cursor = NULL;
    size_t cursor_len = 0;

    /* Reset the frame in the parent so the child's first present() is a
     * known one-repaint frame. */
    be->set_video_mode(st, 0x03);
    g_probe_be = be;
    g_probe_st = st;

    TCHECK(capture_stdout(1, probe_present_cursor_only, &cursor, &cursor_len),
           "present(): capture of a cursor-only change");
    TCHECK(cursor && count_substr(cursor, "\033[H\033[2J") == 1,
           "present(): a cursor-only change does not repaint the cells");
    /* CR then LF from (0,1) lands on row 1, column 0 -> ANSI "row 2, col 1". */
    TCHECK(cursor && strstr(cursor, "\033[2;1H") != NULL,
           "present(): a cursor-only change still moves the terminal cursor");
    free(cursor);
  }

  /* --- INT 10h AH=02h: a cursor move reaches present() ----------------- */
  {
    char *cursor = NULL;
    size_t cursor_len = 0;

    be->set_video_mode(st, 0x03);
    g_probe_ctx = ctx;
    g_probe_be = be;
    g_probe_st = st;

    TCHECK(capture_stdout(1, probe_cursor_int_present, &cursor, &cursor_len),
           "INT 10h AH=02h: capture of a cursor move");
    TCHECK(cursor && count_substr(cursor, "\033[H\033[2J") == 1,
           "INT 10h AH=02h: a cursor move does not repaint the cells");
    TCHECK(cursor && strstr(cursor, "\033[4;6H") != NULL,
           "INT 10h AH=02h: the moved cursor reaches the terminal");
    free(cursor);
  }

  /* --- fail-loud: both classes of missing service must abort ------------ */
  be->set_video_mode(st, 0x03);
  g_probe_ctx = ctx;
  g_probe_be = be;
  g_probe_st = st;

  TCHECK(probe_aborts(probe_unhandled_vector),
         "unhandled INT vector aborts (fail loud)");
  TCHECK(probe_aborts(probe_unhandled_ah),
         "unhandled AH inside a registered vector aborts (fail loud)");
  TCHECK(probe_aborts(probe_graphics_mode),
         "graphics mode on the terminal backend aborts (fail closed)");
  TCHECK(probe_aborts(probe_cursor_page_one),
         "INT 10h AH=02h rejects page 1 (fail loud)");
  TCHECK(probe_aborts(probe_cursor_out_of_range),
         "INT 10h AH=02h rejects a row outside the window (fail loud)");
  TCHECK(probe_aborts(probe_scroll_window_out_of_range),
         "INT 10h AH=06h rejects a window outside the screen (fail loud)");
  TCHECK(probe_aborts(probe_write_page_one),
         "INT 10h AH=09h rejects page 1 (fail loud)");
  TCHECK(probe_aborts(probe_write_run_out_of_range),
         "INT 10h AH=09h rejects a run past the last cell (fail loud)");
  TCHECK(probe_aborts(probe_write_char_run_out_of_range),
         "INT 10h AH=0Ah rejects a run past the last cell (fail loud)");
  TCHECK(probe_aborts(probe_teletype_page_one),
         "INT 10h AH=0Eh rejects page 1 (fail loud)");

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
 * returned. Both halves matter: the value proves the code, and the sentinel
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

/********************************************************************/
/* Video-memory mirroring, from a live machine's hardware vtable.
 *
 * A text-mode guest draws by storing into the text window, so the store must
 * reach the backend's frame through native_mem_write8/16. The flat array stays
 * authoritative: every assertion about the frame is paired with one about
 * guest memory, because a mirror that replaced the store instead of
 * projecting it would pass the first and fail the second. */

int hydra_backend_videotest(hydra_machine_t *m)
{
  t_failures = 0;

  hydra_machine_hardware_t *hw = m->hardware;
  hydra_native_backend_select("terminal");
  void *st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&st);
  if (!be || !st || !be->text_buffer) {
    TCHECK(0, "video mirror: terminal backend selected");
    return t_failures;
  }

  uint8_t ch = 0, attr = 0;
  const uint8_t *frame = be->text_buffer(st, NULL, NULL);
  const size_t frame_bytes = BACKEND_TEXT_ROWS * BACKEND_TEXT_COLS;

  /* --- mode 03h: 80x25 at B8000h, cells are (character, attribute) ------ */
  be->set_video_mode(st, 0x03);

  hw->mem_write8(hw->ctx, 0xB8000u, 'A');
  TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'A',
         "video mirror: B8000h character store reaches the shared frame");
  TCHECK(hw->mem_read8(hw->ctx, 0xB8000u) == 'A',
         "video mirror: guest memory stays authoritative (readback)");
  TCHECK(attr == 0x07,
         "video mirror: a character store keeps the cell's attribute");

  hw->mem_write8(hw->ctx, 0xB8001u, 0x1e);
  TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'A' && attr == 0x1e,
         "video mirror: an attribute store keeps the cell's character");

  hw->mem_write16(hw->ctx, 0xB8002u, 0x0742u);   /* 'B', attribute 07h */
  TCHECK(be->get_cell(st, 0, 1, &ch, &attr) == 0 && ch == 'B' && attr == 0x07,
         "video mirror: an aligned word store fills one cell");

  /* An odd-aligned word store is two bus cycles: the low byte lands in the
   * attribute of cell 2 and the high byte in the character of cell 3. */
  hw->mem_write16(hw->ctx, 0xB8005u, 0x0843u);
  TCHECK(be->get_cell(st, 0, 2, &ch, &attr) == 0 && ch == ' ' && attr == 0x43,
         "video mirror: an odd word store splits (attribute half)");
  TCHECK(be->get_cell(st, 0, 3, &ch, &attr) == 0 && ch == 0x08 && attr == 0x07,
         "video mirror: an odd word store splits (character half)");

  /* Row stride comes from the backend's geometry, not from a constant. */
  hw->mem_write16(hw->ctx, 0xB8000u + 80u * 2u, 0x0743u);   /* 'C', row 1 col 0 */
  TCHECK(be->get_cell(st, 1, 0, &ch, &attr) == 0 && ch == 'C',
         "video mirror: the row stride follows the 80-column geometry");

  /* --- what must not render --------------------------------------------- */
  uint8_t before[BACKEND_TEXT_ROWS * BACKEND_TEXT_COLS];

  memcpy(before, frame, frame_bytes);
  hw->mem_write8(hw->ctx, 0x1234u, 'Z');
  TCHECK(memcmp(before, frame, frame_bytes) == 0,
         "video mirror: ordinary memory stores leave the frame alone");

  /* Page 1 of an 80x25 mode starts at B9000h. It is real guest memory and
   * nothing displays it, exactly as on the hardware. */
  hw->mem_write16(hw->ctx, 0xB9000u, 0x0758u);   /* 'X' */
  TCHECK(memcmp(before, frame, frame_bytes) == 0 &&
             hw->mem_read16(hw->ctx, 0xB9000u) == 0x0758u,
         "video mirror: page 1 stores stay in guest memory and render nowhere");

  /* An odd-aligned word store is two bus cycles, so its halves are routed
   * separately and can straddle an end of the window. At the last cell of a
   * 80x25 window (B8F9Fh) the low byte is that cell's attribute — inside the
   * window — and the high byte is already past the end. The in-window half
   * renders; the other half is page 1 and renders nowhere. */
  {
    uint8_t last_ch = 0, last_attr = 0;
    const uint32_t window_end = 0xB8000u + 80u * 25u * 2u;

    hw->mem_write16(hw->ctx, window_end - 1u, 0x4cabu);   /* lo=AB attr, hi=4C */

    TCHECK(be->get_cell(st, BACKEND_TEXT_ROWS - 1, BACKEND_TEXT_COLS - 1,
                        &last_ch, &last_attr) == 0 && last_attr == 0xab,
           "video mirror: a word store straddling the window end renders its "
           "in-window half");
    TCHECK(last_ch != 0xab,
           "video mirror: the straddle does not leak the other half into the cell");
    TCHECK(hw->mem_read8(hw->ctx, window_end) == 0x4c,
           "video mirror: the out-of-window half still lands in guest memory");

    /* The symmetric case at the start: a store at B7FFFh puts its low byte in
     * the gap before the window and its high byte in the first cell. */
    hw->mem_write16(hw->ctx, 0xB8000u - 1u, 0x4707u);     /* hi=47 'G' at B8000h */

    TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'G',
           "video mirror: a word store straddling the window start renders its "
           "in-window half");
    TCHECK(hw->mem_read8(hw->ctx, 0xB8000u - 1u) == 0x07,
           "video mirror: the below-window half still lands in guest memory");
  }

  /* --- mode 07h is MDA: the window is B0000h --------------------------- */
  be->set_video_mode(st, 0x07);
  hw->mem_write16(hw->ctx, 0xB0000u, 0x0753u);   /* 'S' */
  TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'S',
         "video mirror: mode 07h renders the B0000h window");

  hw->mem_write16(hw->ctx, 0xB8000u, 0x0759u);   /* 'Y', no window there */
  TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'S' &&
             hw->mem_read16(hw->ctx, 0xB8000u) == 0x0759u,
         "video mirror: mode 07h leaves B8000h unrendered (but stored)");

  /* --- 40-column text modes use the same base and a 40-cell row -------- */
  be->set_video_mode(st, 0x00);
  hw->mem_write16(hw->ctx, 0xB8000u + 40u * 2u, 0x0754u);   /* 'T', row 1 col 0 */
  TCHECK(be->get_cell(st, 1, 0, &ch, &attr) == 0 && ch == 'T',
         "video mirror: 40-column modes use a 40-cell row stride");

  /* The byte path has its own copy of the row arithmetic, so it gets its own
   * store: row 2, column 5, at 40 cells per row. */
  hw->mem_write8(hw->ctx, 0xB8000u + 2u * (40u * 2u) + 5u * 2u, 'U');
  TCHECK(be->get_cell(st, 2, 5, &ch, &attr) == 0 && ch == 'U',
         "video mirror: 40-column byte stores use the same stride");

  /* --- the host-facing present entry point renders ----------------------- */
  {
    char *wrapped = NULL;
    size_t wrapped_len = 0;

    TCHECK(capture_stdout(1, probe_wrapper_present, &wrapped, &wrapped_len),
           "present: capture through hydra_native_backend_present()");
    TCHECK(wrapped && strstr(wrapped, "\033[H\033[2J") != NULL &&
               strchr(wrapped, 'W') != NULL,
           "present: the host entry point renders the drawn frame");
    TCHECK(wrapped && count_substr(wrapped, "\033[H\033[2J") == 1,
           "present: a second host entry point call with nothing new emits nothing");
    free(wrapped);
  }

  /* --- state_restore() re-projects the window (kept last: it replaces the
   * whole guest image) --------------------------------------------------- */
  {
    char path[] = "/tmp/native_host_restore_proj_XXXXXX";
    int fd = mkstemp(path);

    TCHECK(fd >= 0, "restore: temp snapshot created");
    if (fd >= 0) {
      close(fd);

      be->set_video_mode(st, 0x03);
      hw->mem_write16(hw->ctx, 0xB8000u, 0x1e41u);         /* 'A', attr 1Eh */
      hw->state_save(hw->ctx, path);

      hw->mem_write16(hw->ctx, 0xB8000u, 0x1e42u);         /* 'B' */
      TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'B',
             "restore: the frame follows the live store before the restore");

      hw->state_restore(hw->ctx, path);

      /* Both halves of the cell: a re-projection that carried the character
       * but reset the attribute would pass a character-only assertion while
       * drawing the restored screen in the wrong colour. */
      TCHECK(be->get_cell(st, 0, 0, &ch, &attr) == 0 && ch == 'A' && attr == 0x1e,
             "restore: the restored window is re-projected into the frame");
      TCHECK(hw->mem_read16(hw->ctx, 0xB8000u) == 0x1e41u,
             "restore: guest memory holds the restored bytes");

      /* The real failure mode was invisible: the frame looked right in
       * get_cell() only if it was re-projected, and present() stayed silent
       * because no store had dirtied it. */
      g_restore_hw = hw;
      g_restore_path = path;
      char *restored = NULL;
      size_t restored_len = 0;
      TCHECK(capture_stdout(1, probe_restore_then_present, &restored, &restored_len),
             "restore: capture of the first frame after a restore");
      TCHECK(restored && strstr(restored, "\033[H\033[2J") != NULL &&
                 strchr(restored, 'A') != NULL,
             "restore: present() emits the restored frame (the restore marks it dirty)");
      free(restored);

      unlink(path);
    }
  }

  /* --- a store into the text window with no text backend is a clean no-op.
   * Same guard the "no backend selected yet" case takes: the bytes are already
   * in guest memory, and a projection must never abort the process. ---------- */
  {
    g_restore_hw = hw;
    fflush(stdout);
    const int stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
    pid_t pid = fork();
    if (pid == 0) {
      probe_store_without_text_backend();
      fflush(stdout);
      _exit(0);
    }
    int status = 0;
    const pid_t waited = pid > 0 ? waitpid(pid, &status, 0) : -1;
    if (stdin_flags >= 0)
      fcntl(STDIN_FILENO, F_SETFL, stdin_flags);

    TCHECK(waited == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
           "video mirror: a store with a backend that renders no text is a "
           "clean no-op (no abort)");
  }

  /* --- the same store before any backend is selected --------------------- */
  {
    g_restore_hw = hw;
    fflush(stdout);
    const int stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
    pid_t pid = fork();
    if (pid == 0) {
      probe_store_without_backend();
      fflush(stdout);
      _exit(0);
    }
    int status = 0;
    const pid_t waited = pid > 0 ? waitpid(pid, &status, 0) : -1;
    if (stdin_flags >= 0)
      fcntl(STDIN_FILENO, F_SETFL, stdin_flags);

    TCHECK(waited == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
           "video mirror: a store before any backend is selected keeps the "
           "bytes and does not abort");
  }

  return t_failures;
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
  exec.machine.registers->bx = 0x0000;     /* BH=0: page 0 */
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
