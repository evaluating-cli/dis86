/* Terminal backend: the reference hydra_backend_t implementation (issue #95).
 *
 * Text mode only, rendered through ANSI on `present()`. Graphics modes are
 * refused by `set_video_mode`, which lets the INT 10h handler fail closed with
 * a "use the SDL backend" style diagnostic instead of emitting garbage.
 *
 * The backend keeps a shadow text buffer, so it is testable without a
 * terminal: `present()` only writes when stdout is a tty, which keeps batch
 * and CI output clean.
 */

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "hydra_backend.h"

typedef struct {
  uint8_t ch[BACKEND_TEXT_ROWS * BACKEND_TEXT_COLS];
  uint8_t attr[BACKEND_TEXT_ROWS * BACKEND_TEXT_COLS];
  uint8_t rows;
  uint8_t cols;
  uint8_t mode;
  uint8_t cur_row;
  uint8_t cur_col;
  struct timespec start;
  uint16_t key_buf;
  int key_pending;
  int stdin_nonblock;
  int saved_stdin_flags;
} terminal_t;

static void term_clear(terminal_t *t)
{
  memset(t->ch, ' ', sizeof(t->ch));
  memset(t->attr, 0x07, sizeof(t->attr));
  t->cur_row = 0;
  t->cur_col = 0;
}

static void term_scroll(terminal_t *t)
{
  const size_t rowbytes = t->cols;
  const size_t last = (size_t)(t->rows - 1) * rowbytes;
  memmove(t->ch, t->ch + rowbytes, last);
  memmove(t->attr, t->attr + rowbytes, last);
  memset(t->ch + last, ' ', rowbytes);
  memset(t->attr + last, 0x07, rowbytes);
}

static void term_newline(terminal_t *t)
{
  t->cur_col = 0;
  if (t->cur_row + 1 < t->rows) {
    t->cur_row++;
  } else {
    term_scroll(t);
  }
}

static int term_init(void **state_out, const char *opts)
{
  (void)opts;
  terminal_t *t = calloc(1, sizeof(*t));
  if (!t)
    return -1;

  t->rows = BACKEND_TEXT_ROWS;
  t->cols = BACKEND_TEXT_COLS;
  t->mode = 0x03;
  term_clear(t);
  clock_gettime(CLOCK_MONOTONIC, &t->start);

  /* Only touch stdin's blocking mode when it is an interactive terminal. A
   * piped or closed stdin must be left alone so batch runs are unaffected
   * and `poll_key` simply reports "no key". */
  if (isatty(STDIN_FILENO)) {
    int fl = fcntl(STDIN_FILENO, F_GETFL, 0);
    if (fl >= 0 && fcntl(STDIN_FILENO, F_SETFL, fl | O_NONBLOCK) == 0) {
      t->saved_stdin_flags = fl;
      t->stdin_nonblock = 1;
    }
  }

  *state_out = t;
  return 0;
}

static void term_shutdown(void *state)
{
  terminal_t *t = (terminal_t *)state;
  if (!t)
    return;
  if (t->stdin_nonblock)
    fcntl(STDIN_FILENO, F_SETFL, t->saved_stdin_flags);
  free(t);
}

static int term_set_video_mode(void *state, uint8_t mode)
{
  terminal_t *t = (terminal_t *)state;
  /* Text modes only: 0x00-0x03 (CGA/CO text) and 0x07 (MDA mono text).
   * Everything else is graphics, which this backend cannot render. Modes
   * 0x00/0x01 are 40-column; the rest are 80-column. */
  if (!(mode <= 0x03 || mode == 0x07))
    return -1;
  t->mode = mode;
  t->cols = (mode <= 0x01) ? 40 : BACKEND_TEXT_COLS;
  term_clear(t);
  return 0;
}

static void term_teletype(void *state, uint8_t ch, uint8_t attr)
{
  terminal_t *t = (terminal_t *)state;

  if (ch == '\r') { t->cur_col = 0; return; }
  if (ch == '\n') { term_newline(t); return; }
  if (ch == '\b') {
    if (t->cur_col > 0) t->cur_col--;
    return;
  }

  const size_t idx = (size_t)t->cur_row * t->cols + t->cur_col;
  t->ch[idx] = ch;
  /* In text mode BL is ignored, so 0 means "keep the default attribute". */
  t->attr[idx] = attr ? attr : (uint8_t)0x07;

  if (++t->cur_col >= t->cols)
    term_newline(t);
}

static void term_putchar_text(void *state, uint8_t row, uint8_t col, uint8_t ch, uint8_t attr)
{
  terminal_t *t = (terminal_t *)state;
  if (row >= t->rows || col >= t->cols)
    return;
  const size_t idx = (size_t)row * t->cols + col;
  t->ch[idx] = ch;
  /* Explicit-cell writes carry a real attribute; 0x00 is black-on-black and
   * must not be replaced with the teletype default. */
  t->attr[idx] = attr;
}

static void term_present(void *state)
{
  terminal_t *t = (terminal_t *)state;

  /* Headless (piped/CI) runs produce no escape sequences; a real terminal
   * gets a full repaint. */
  if (!isatty(STDOUT_FILENO))
    return;

  fputs("\033[H\033[2J", stdout);
  for (uint8_t r = 0; r < t->rows; r++) {
    for (uint8_t c = 0; c < t->cols; c++) {
      const size_t idx = (size_t)r * t->cols + c;
      uint8_t ch = t->ch[idx];
      if (ch < 0x20 || ch == 0x7f)
        ch = ' ';
      fputc((int)ch, stdout);
    }
    fputc('\n', stdout);
  }
  fflush(stdout);
}

/* BIOS key word for one byte of terminal input: AH = scan code, AL = ASCII.
 * Common US-layout keys are mapped; unmapped bytes keep AH=0. ANSI escape
 * sequences are not decoded yet, so extended keys arrive as their component
 * bytes. */
static uint16_t term_key_from_byte(uint8_t c)
{
  static const uint8_t scan[128] = {
    ['\x1b'] = 0x01,
    ['1'] = 0x02, ['2'] = 0x03, ['3'] = 0x04, ['4'] = 0x05,
    ['5'] = 0x06, ['6'] = 0x07, ['7'] = 0x08, ['8'] = 0x09,
    ['9'] = 0x0a, ['0'] = 0x0b, ['-'] = 0x0c, ['='] = 0x0d,
    ['\b'] = 0x0e, ['\t'] = 0x0f,
    ['q'] = 0x10, ['w'] = 0x11, ['e'] = 0x12, ['r'] = 0x13,
    ['t'] = 0x14, ['y'] = 0x15, ['u'] = 0x16, ['i'] = 0x17,
    ['o'] = 0x18, ['p'] = 0x19, ['['] = 0x1a, [']'] = 0x1b,
    ['a'] = 0x1e, ['s'] = 0x1f, ['d'] = 0x20, ['f'] = 0x21,
    ['g'] = 0x22, ['h'] = 0x23, ['j'] = 0x24, ['k'] = 0x25,
    ['l'] = 0x26, [';'] = 0x27, ['\''] = 0x28, ['`'] = 0x29,
    ['\\'] = 0x2b,
    ['z'] = 0x2c, ['x'] = 0x2d, ['c'] = 0x2e, ['v'] = 0x2f,
    ['b'] = 0x30, ['n'] = 0x31, ['m'] = 0x32, [','] = 0x33,
    ['.'] = 0x34, ['/'] = 0x35,
    [' '] = 0x39,
  };

  if (c == '\r' || c == '\n')
    return 0x1c0d;
  if (c == 0x7f)
    return 0x0e08;

  uint8_t idx = c;
  if (idx >= 'A' && idx <= 'Z')
    idx = (uint8_t)(idx - 'A' + 'a');
  uint8_t sc = idx < 128 ? scan[idx] : 0;
  return (uint16_t)(((uint16_t)sc << 8) | c);
}

/* Peek without consuming: fill the one-key buffer when empty. */
static int term_key_fill(terminal_t *t)
{
  if (t->key_pending)
    return 1;
  if (!t->stdin_nonblock)
    return 0;

  unsigned char c;
  ssize_t n = read(STDIN_FILENO, &c, 1);
  if (n != 1)
    return 0;
  t->key_buf = term_key_from_byte(c);
  t->key_pending = 1;
  return 1;
}

static int term_poll_key(void *state, uint16_t *key)
{
  terminal_t *t = (terminal_t *)state;
  if (!term_key_fill(t))
    return 0;
  *key = t->key_buf;
  return 1;
}

static int term_read_key(void *state, uint16_t *key)
{
  terminal_t *t = (terminal_t *)state;
  if (t->key_pending) {
    *key = t->key_buf;
    t->key_pending = 0;
    return 1;
  }

  unsigned char c;
  if (t->stdin_nonblock) {
    struct pollfd pfd = { .fd = STDIN_FILENO, .events = POLLIN };
    int rc;
    do {
      rc = poll(&pfd, 1, -1);
    } while (rc < 0 && errno == EINTR);
    if (rc <= 0)
      return 0;
  }

  ssize_t n = read(STDIN_FILENO, &c, 1);
  if (n != 1)
    return 0;
  *key = term_key_from_byte(c);
  return 1;
}

static uint32_t term_ticks(void *state)
{
  terminal_t *t = (terminal_t *)state;
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC, &now);

  /* The nanosecond delta goes negative whenever the call lands in a later
   * second at a lower fractional second, so keep the whole sum signed. */
  int64_t ms = (int64_t)(now.tv_sec - t->start.tv_sec) * 1000
             + (now.tv_nsec - t->start.tv_nsec) / 1000000;
  if (ms < 0)
    ms = 0;
  /* 18.2065 Hz, scaled with integers: ticks = ms * 18207 / 1e6. */
  return (uint32_t)(((uint64_t)ms * 18207u) / 1000000u);
}

static const uint8_t *term_text_buffer(void *state, uint8_t *rows, uint8_t *cols)
{
  terminal_t *t = (terminal_t *)state;
  if (rows) *rows = t->rows;
  if (cols) *cols = t->cols;
  return t->ch;
}

const hydra_backend_t *hydra_backend_terminal(void)
{
  static const hydra_backend_t be = {
    .name           = "terminal",
    .init           = term_init,
    .shutdown       = term_shutdown,
    .set_video_mode = term_set_video_mode,
    .teletype       = term_teletype,
    .putchar_text   = term_putchar_text,
    .present        = term_present,
    .poll_key       = term_poll_key,
    .read_key       = term_read_key,
    .ticks          = term_ticks,
    .text_buffer    = term_text_buffer,
  };
  return &be;
}

const hydra_backend_t *hydra_backend_by_name(const char *name)
{
  if (!name || !*name)
    return hydra_backend_terminal();
  if (strcmp(name, "terminal") == 0)
    return hydra_backend_terminal();
  /* "sdl2" is not linked in this lane (optional dependency). */
  return NULL;
}
