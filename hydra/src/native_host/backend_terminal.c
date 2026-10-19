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

#include <fcntl.h>
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
  int stdin_nonblock;
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
    if (fl >= 0 && fcntl(STDIN_FILENO, F_SETFL, fl | O_NONBLOCK) == 0)
      t->stdin_nonblock = 1;
  }

  *state_out = t;
  return 0;
}

static void term_shutdown(void *state) { free(state); }

static int term_set_video_mode(void *state, uint8_t mode)
{
  terminal_t *t = (terminal_t *)state;
  /* Text modes only: 0x00-0x03 (CGA/CO text) and 0x07 (MDA mono text).
   * Everything else is graphics, which this backend cannot render. */
  if (!(mode <= 0x03 || mode == 0x07))
    return -1;
  t->mode = mode;
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
  t->attr[idx] = attr ? attr : (uint8_t)0x07;
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

static int term_poll_key(void *state, uint16_t *key)
{
  terminal_t *t = (terminal_t *)state;
  if (!t->stdin_nonblock)
    return 0;

  unsigned char c;
  ssize_t n = read(STDIN_FILENO, &c, 1);
  if (n != 1)
    return 0;
  *key = (uint16_t)c;
  return 1;
}

static uint32_t term_ticks(void *state)
{
  terminal_t *t = (terminal_t *)state;
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC, &now);

  uint64_t ms = (uint64_t)(now.tv_sec - t->start.tv_sec) * 1000u +
                (uint64_t)(now.tv_nsec - t->start.tv_nsec) / 1000000u;
  /* 18.2065 Hz, scaled with integers: ticks = ms * 18207 / 1e6. */
  return (uint32_t)((ms * 18207u) / 1000000u);
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
