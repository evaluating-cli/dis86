/* Device/backend HAL for the standalone native host (issue #95).
 *
 * One vtable, several implementations. `backend_terminal.c` is the reference
 * implementation (text mode only, fails closed on graphics); an SDL2 backend
 * would supply the same vtable behind an optional dependency and is not built
 * in this lane (see the investigation notes: `just check` compiles these
 * sources with a bare `cc`, so any backend must build without new deps).
 *
 * This is a skeleton: nothing calls into it yet. The wiring step (routing
 * `hydra_impl_int` and the port primitives here when
 * `hydra_raw_code_available()` is false) is deliberately separate, so this
 * addition cannot affect the dosemu host.
 */
#pragma once

#include <stdint.h>

#include "native_host.h"

#define BACKEND_TEXT_COLS 80
#define BACKEND_TEXT_ROWS 25

typedef struct hydra_backend hydra_backend_t;

struct hydra_backend {
  const char *name;

  int  (*init)(void **state_out, const char *opts);
  void (*shutdown)(void *state);

  /* Video. `set_video_mode` returns 0 on success and -1 when the backend
   * cannot render that mode; the INT 10h handler turns -1 into a fail-closed
   * diagnostic rather than guessing at a rendering.
   *
   * `teletype` advances a cursor and scrolls; `putchar_text` writes an
   * explicit cell. Both land in a shadow buffer that `text_buffer` exposes,
   * so tests read state instead of capturing stdout. */
  int  (*set_video_mode)(void *state, uint8_t mode);
  void (*teletype)(void *state, uint8_t ch, uint8_t attr);
  void (*putchar_text)(void *state, uint8_t row, uint8_t col, uint8_t ch, uint8_t attr);
  void (*present)(void *state);

  /* Input. Returns 1 when a key was available (and stores it in *key),
   * 0 otherwise. Never blocks. */
  int  (*poll_key)(void *state, uint16_t *key);

  /* Timer: the BIOS tick count at the 18.2 Hz nominal rate. */
  uint32_t (*ticks)(void *state);

  /* Text shadow buffer for text backends; NULL otherwise. */
  const uint8_t *(*text_buffer)(void *state, uint8_t *rows, uint8_t *cols);
};

const hydra_backend_t *hydra_backend_terminal(void);

/* NULL when the name is unknown (e.g. "sdl2" in a lane built without it).
 * NULL or empty selects the terminal backend. */
const hydra_backend_t *hydra_backend_by_name(const char *name);

/* Selective INT dispatch (issue #95): a two-level table, vector -> AH handler.
 *
 * Returns 0 when a handler ran, or NATIVE_INT_EXIT when the handler requests
 * program termination (INT 21h AH=4Ch; AL carries the exit code).
 *
 * An unhandled vector, or an unhandled AH inside a handled vector, fails loud
 * (diagnostic naming both, then abort) — the house fail-closed style, and the
 * behaviour #95 asks for.
 */
#define NATIVE_INT_EXIT 2

typedef int (*hydra_int_ah_fn)(native_host_ctx_t *ctx,
                               hydra_machine_registers_t *regs,
                               const hydra_backend_t *be,
                               void *be_state);

int native_int_dispatch(native_host_ctx_t *ctx,
                        hydra_machine_registers_t *regs,
                        uint8_t num,
                        const hydra_backend_t *be,
                        void *be_state);

/* Self-test entry point, called from test_native_host.c so the whole native
 * host keeps a single main() (all of native_host/*.c link into one binary).
 * Returns the number of failures. */
int hydra_backend_selftest(void);
