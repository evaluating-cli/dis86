/* Device/backend HAL for the standalone native host (issue #95).
 *
 * One vtable, several implementations. `backend_terminal.c` is the reference
 * implementation (text mode only, fails closed on graphics); an SDL2 backend
 * would supply the same vtable behind an optional dependency and is not built
 * in this lane (see the investigation notes: `just check` compiles these
 * sources with a bare `cc`, so any backend must build without new deps).
 *
 * INT dispatch is wired end to end: `hydra_impl_int` routes here through the
 * optional `hydra_user_int` hook (dlsym, see internal.h) whenever
 * `hydra_raw_code_available()` is false, i.e. on a host with no guest CPU to
 * execute the `int` opcode itself. Hosts that do execute raw code (dosemu2)
 * never reach this layer, so they are unaffected.
 *
 * Port I/O is still deliberately separate: routing `inb`/`outb`/`insb`/`rep_*`
 * here needs a port -> device map that this vtable does not express.
 */
#pragma once

#include <stdint.h>

#include "native_host.h"

#define BACKEND_TEXT_COLS 80
#define BACKEND_TEXT_ROWS 25

/* The text window a backend renders, for hosts that mirror guest video memory
 * and for the INT 10h services that report geometry.
 *
 * `base` is the linear address of the window, or 0 when the backend renders no
 * text window at all. `mode` is the BIOS video mode most recently set. Cells
 * are row-major and two bytes wide (character, attribute) — the text-mode
 * layout of every adapter Hydra models: B8000h for the CGA-compatible modes
 * 00h-03h, B0000h for MDA mono mode 07h. */
typedef struct hydra_text_geometry {
  uint32_t base;
  uint8_t  mode;
  uint8_t  rows;
  uint8_t  cols;
} hydra_text_geometry_t;

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

  /* Where the rendered text window lives, or -1 when the backend has none.
   * `present()` renders only what changed since the last call, so a host may
   * call it once per frame (or whenever it knows the guest stopped drawing)
   * without paying for a repaint that would show the same frame again. */
  int  (*text_geometry)(void *state, hydra_text_geometry_t *out);

  /* Read one text cell, so a byte-wide guest store into video memory can be
   * a read-modify-write: a character store must keep the cell's attribute and
   * an attribute store must keep its character. Returns 0 on success, -1 when
   * the cell is out of range or the backend keeps no text buffer. */
  int  (*get_cell)(void *state, uint8_t row, uint8_t col, uint8_t *ch, uint8_t *attr);

  /* Input. Key words use the BIOS encoding: high byte = scan code, low byte =
   * ASCII (0 for extended keys). `poll_key` peeks at the next key and returns
   * 1 without consuming it, or 0 when none is pending; it never blocks.
   * `read_key` consumes and returns the next key, blocking until one arrives
   * (or input ends, in which case it returns 0). */
  int  (*poll_key)(void *state, uint16_t *key);
  int  (*read_key)(void *state, uint16_t *key);

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
 * guest termination (INT 21h AH=4Ch; AL carries the exit code). The host's
 * hook maps that onto the core's HYDRA_USER_INT_TERMINATE, so the core stops
 * the hook at the interrupt and reports the termination to the host driver.
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

/* Select the active backend by name (NULL or "" = the default, terminal).
 * Fails loud on an unknown name. Implemented in backend_host.c.
 *
 * hydra_user_init() calls this once, before any guest code can run. Calling it
 * again from a running host is allowed (the old backend is shut down first),
 * but only from the thread that owns execution: it mutates process state and
 * does not synchronize with a concurrent dispatch. */
void hydra_native_backend_select(const char *name);

/* Test seam, implemented in backend_host.c: install a backend directly instead
 * of looking it up by name. Exists so backend_selftest.c can install a probe
 * backend that only the test knows about, and thereby assert that the atexit
 * teardown really runs when a driver exits after a guest termination. Not
 * part of the host interface; no host calls it. */
void hydra_native_backend_install_probe(const hydra_backend_t *be, void *state);

/* The active backend and its state, as selected above. Non-mutating: it fails
 * loud if no backend has been selected, rather than initializing one from a
 * hook thread. Useful to a host that wants to inspect the backend, and to
 * tests reading the shadow buffer. */
const hydra_backend_t *hydra_native_backend_active(void **state_out);

/* Render whatever the active backend has drawn since its last repaint.
 *
 * The port driver's frame call: cheap and idempotent when nothing changed, and
 * a no-op before a backend has been selected (rendering must never be the
 * thing that ends a run). The terminal backend also calls it for itself before
 * blocking on a key, so a title screen that is waiting for input is on screen.
 */
void hydra_native_backend_present(void);

/* Self-test entry points, called from test_native_host.c so the whole native
 * host keeps a single main() (all native-host sources link into one binary).
 * Each returns the number of failures.
 *
 * hydra_backend_selftest() covers the HAL and dispatch in isolation;
 * hydra_backend_linktest() covers the wiring from a live machine, i.e. that
 * hydra_impl_int reaches a backend through the dlsym hook;
 * hydra_backend_videotest() covers guest stores landing in the text window
 * from a live machine's hardware vtable.
 */
int hydra_backend_selftest(void);
int hydra_backend_linktest(hydra_machine_t *m);
int hydra_backend_videotest(hydra_machine_t *m);
