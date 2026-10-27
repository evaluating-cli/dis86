/* Selective INT dispatch for the standalone native host (issue #95).
 *
 * Two-level table: vector -> AH handler. Registered vectors are the ones #95
 * names (10h video, 1Ah timer, 21h DOS, 33h mouse) plus 16h (keyboard), which
 * #95's Tier-1 criteria require ("keyboard-only (INT 16h or direct port
 * polling)") and which is the natural place for the backend's input pump.
 *
 * Anything unregistered, and any unimplemented AH inside a registered vector,
 * fails loud: a diagnostic naming the vector, the service and the AH value,
 * then abort. That is the house fail-closed style and #95's stated contract.
 *
 * The video surface is the text-mode subset a DOS program drives: mode set and
 * teletype, cursor get/set, scroll, character/attribute read/write, and video
 * state. DOS print char / print string / exit, timer ticks, and key status /
 * blocking key read complete the minimum surface a keyboard-driven Tier-1
 * target needs.
 */

#include <stdio.h>

#include "header.h"
#include "hydra_backend.h"

/* ZF is bit 6 of the flags word (see machine.h's FLAGS_ZF). */
#define FLAG_ZF (1u << 6)

typedef struct {
  const char *name;
  const hydra_int_ah_fn *ah;
  size_t n;
} int_vector_t;

static void putc_or_fail(const hydra_backend_t *be, void *st, uint8_t ch, uint8_t attr)
{
  if (!be || !be->teletype)
    FAIL("native host: no backend supplies teletype output");
  be->teletype(st, ch, attr);
}

/* Every service below addresses page 0 only: the shadow frame and the text
 * window model one active page, so a call naming another page would otherwise
 * report success while drawing somewhere the guest cannot see. */
static void require_page_zero(const hydra_machine_registers_t *regs, const char *service)
{
  uint8_t page = (uint8_t)((regs->bx >> 8) & 0xff);
  if (page != 0)
    FAIL("native host: INT 10h %s page %u is not modeled (page 0 only)",
         service, (unsigned)page);
}

static void text_geometry_or_fail(const hydra_backend_t *be, void *st,
                                  hydra_text_geometry_t *geo, const char *service)
{
  if (!be || !be->text_geometry)
    FAIL("native host: no backend supplies text geometry for INT 10h %s", service);
  if (be->text_geometry(st, geo) < 0 || geo->rows == 0 || geo->cols == 0)
    FAIL("native host: the '%s' backend has no text window for INT 10h %s",
         be->name ? be->name : "?", service);
}

static void get_cursor_or_fail(const hydra_backend_t *be, void *st,
                               uint8_t *row, uint8_t *col, const char *service)
{
  if (!be || !be->get_cursor)
    FAIL("native host: no backend supplies a text cursor for INT 10h %s", service);
  if (be->get_cursor(st, row, col) < 0)
    FAIL("native host: the '%s' backend has no text cursor for INT 10h %s",
         be->name ? be->name : "?", service);
}

static void set_cursor_or_fail(const hydra_backend_t *be, void *st,
                               uint8_t row, uint8_t col, const char *service)
{
  if (!be || !be->set_cursor)
    FAIL("native host: no backend supplies a text cursor for INT 10h %s", service);
  if (be->set_cursor(st, row, col) == 0)
    return;

  hydra_text_geometry_t geo;
  if (be->text_geometry && be->text_geometry(st, &geo) == 0 &&
      geo.rows != 0 && geo.cols != 0)
    FAIL("native host: INT 10h %s cursor (%u,%u) is outside the %ux%u text window",
         service, (unsigned)row, (unsigned)col,
         (unsigned)geo.rows, (unsigned)geo.cols);
  FAIL("native host: INT 10h %s cursor (%u,%u) is outside the text window",
       service, (unsigned)row, (unsigned)col);
}

static void get_cell_or_fail(const hydra_backend_t *be, void *st,
                             uint8_t row, uint8_t col, uint8_t *ch, uint8_t *attr,
                             const char *service)
{
  if (!be || !be->get_cell)
    FAIL("native host: no backend supplies text cells for INT 10h %s", service);
  if (be->get_cell(st, row, col, ch, attr) < 0)
    FAIL("native host: INT 10h %s cell (%u,%u) is outside the text window",
         service, (unsigned)row, (unsigned)col);
}

static void putcell_or_fail(const hydra_backend_t *be, void *st,
                            uint8_t row, uint8_t col, uint8_t ch, uint8_t attr,
                            const char *service)
{
  if (!be || !be->putchar_text)
    FAIL("native host: no backend supplies explicit text cells for INT 10h %s",
         service);
  be->putchar_text(st, row, col, ch, attr);
}

/********************************************************************/
/* INT 10h - video services                                          */

/* AH=00h: set video mode. AL = mode. */
static int v_set_mode(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                      const hydra_backend_t *be, void *st)
{
  (void)ctx;
  uint8_t mode = (uint8_t)(regs->ax & 0xff);
  if (!be || !be->set_video_mode)
    FAIL("native host: no backend supplies set_video_mode");

  if (be->set_video_mode(st, mode) < 0)
    FAIL("native host: INT 10h AH=00h video mode 0x%02x is not supported by the "
         "'%s' backend (text modes only; build with the SDL backend for "
         "graphics modes)",
         (unsigned)mode, be->name);
  return 0;
}

/* AH=0Eh: teletype output. AL = character, BH = page, BL = attribute. */
static int v_teletype(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                      const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=0Eh");
  uint8_t ch = (uint8_t)(regs->ax & 0xff);
  uint8_t attr = (uint8_t)(regs->bx & 0xff);
  putc_or_fail(be, st, ch, attr);
  return 0;
}

/* AH=02h: set cursor position. DH = row, DL = column, BH = page. */
static int v_set_cursor(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                        const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=02h");
  set_cursor_or_fail(be, st, (uint8_t)((regs->dx >> 8) & 0xff),
                     (uint8_t)(regs->dx & 0xff), "AH=02h");
  return 0;
}

/* AH=03h: read cursor position and shape. DH/DL = row/column, CX = 0607h
 * (the default shape; AH=01h is not modeled, so the shape never changes). */
static int v_get_cursor(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                        const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=03h");
  uint8_t row = 0, col = 0;
  get_cursor_or_fail(be, st, &row, &col, "AH=03h");
  regs->dx = (uint16_t)(((uint16_t)row << 8) | col);
  regs->cx = 0x0607;
  return 0;
}

/* AH=06h/07h: scroll a window. AL = lines (0 = clear), BH = blank attribute,
 * CH/CL = upper-left, DH/DL = lower-right. The cursor does not move. */
static void blank_window(const hydra_backend_t *be, void *st,
                         uint8_t top, uint8_t left, uint8_t bottom, uint8_t right,
                         uint8_t attr, const char *service)
{
  for (uint8_t r = top; r <= bottom; r++)
    for (uint8_t c = left; c <= right; c++)
      putcell_or_fail(be, st, r, c, ' ', attr, service);
}

static int v_scroll(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                    const hydra_backend_t *be, void *st, int up,
                    const char *service)
{
  (void)ctx;
  hydra_text_geometry_t geo;
  text_geometry_or_fail(be, st, &geo, service);

  uint8_t lines = (uint8_t)(regs->ax & 0xff);
  uint8_t attr = (uint8_t)((regs->bx >> 8) & 0xff);
  uint8_t top = (uint8_t)((regs->cx >> 8) & 0xff);
  uint8_t left = (uint8_t)(regs->cx & 0xff);
  uint8_t bottom = (uint8_t)((regs->dx >> 8) & 0xff);
  uint8_t right = (uint8_t)(regs->dx & 0xff);

  if (top > bottom || left > right || bottom >= geo.rows || right >= geo.cols)
    FAIL("native host: INT 10h %s window (%u,%u)-(%u,%u) is outside the "
         "%ux%u text window", service,
         (unsigned)top, (unsigned)left, (unsigned)bottom, (unsigned)right,
         (unsigned)geo.rows, (unsigned)geo.cols);

  uint8_t height = (uint8_t)(bottom - top + 1u);
  if (lines == 0 || lines >= height) {
    blank_window(be, st, top, left, bottom, right, attr, service);
    return 0;
  }

  /* Copy in the direction that never reads a cell already overwritten: up
   * moves a row toward row 0, so increasing rows; down is the reverse. */
  if (up) {
    for (uint8_t r = top; r <= (uint8_t)(bottom - lines); r++) {
      for (uint8_t c = left; c <= right; c++) {
        uint8_t ch = 0, cell_attr = 0;
        get_cell_or_fail(be, st, (uint8_t)(r + lines), c, &ch, &cell_attr, service);
        putcell_or_fail(be, st, r, c, ch, cell_attr, service);
      }
    }
    blank_window(be, st, (uint8_t)(bottom - lines + 1u), left, bottom, right,
                 attr, service);
  } else {
    for (uint8_t r = bottom; r >= (uint8_t)(top + lines); r--) {
      for (uint8_t c = left; c <= right; c++) {
        uint8_t ch = 0, cell_attr = 0;
        get_cell_or_fail(be, st, (uint8_t)(r - lines), c, &ch, &cell_attr, service);
        putcell_or_fail(be, st, r, c, ch, cell_attr, service);
      }
    }
    blank_window(be, st, top, left, (uint8_t)(top + lines - 1u), right, attr,
                 service);
  }
  return 0;
}

static int v_scroll_up(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                       const hydra_backend_t *be, void *st)
{
  return v_scroll(ctx, regs, be, st, 1, "AH=06h");
}

static int v_scroll_down(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                         const hydra_backend_t *be, void *st)
{
  return v_scroll(ctx, regs, be, st, 0, "AH=07h");
}

/* AH=08h: read the character and attribute at the cursor into AL/AH. */
static int v_read_char(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                       const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=08h");
  uint8_t row = 0, col = 0, ch = 0, attr = 0;
  get_cursor_or_fail(be, st, &row, &col, "AH=08h");
  get_cell_or_fail(be, st, row, col, &ch, &attr, "AH=08h");
  regs->ax = (uint16_t)(((uint16_t)attr << 8) | ch);
  return 0;
}

/* AH=09h: write AL with attribute BL, CX times, at the cursor. Text mode has
 * one cell there, so one write is equivalent to the repetition, and the
 * cursor stays put. */
static int v_write_char_attr(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                             const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=09h");
  if (regs->cx == 0)
    return 0;
  uint8_t row = 0, col = 0;
  get_cursor_or_fail(be, st, &row, &col, "AH=09h");
  putcell_or_fail(be, st, row, col, (uint8_t)(regs->ax & 0xff),
                  (uint8_t)(regs->bx & 0xff), "AH=09h");
  return 0;
}

/* AH=0Ah: write AL with the cell's current attribute, CX times. BL is a
 * graphics-mode color and is ignored here. */
static int v_write_char(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                        const hydra_backend_t *be, void *st)
{
  (void)ctx;
  require_page_zero(regs, "AH=0Ah");
  if (regs->cx == 0)
    return 0;
  uint8_t row = 0, col = 0, ch = 0, attr = 0;
  get_cursor_or_fail(be, st, &row, &col, "AH=0Ah");
  get_cell_or_fail(be, st, row, col, &ch, &attr, "AH=0Ah");
  putcell_or_fail(be, st, row, col, (uint8_t)(regs->ax & 0xff), attr, "AH=0Ah");
  return 0;
}

/* AH=0Fh: video state. AL = mode, AH = columns, BH = active page (0). */
static int v_get_mode(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                      const hydra_backend_t *be, void *st)
{
  (void)ctx;
  hydra_text_geometry_t geo;
  text_geometry_or_fail(be, st, &geo, "AH=0Fh");
  regs->ax = (uint16_t)(((uint16_t)geo.cols << 8) | geo.mode);
  regs->bx = (uint16_t)(regs->bx & 0x00ff);
  return 0;
}

/********************************************************************/
/* INT 21h - DOS services                                            */

/* AH=02h: write character in DL to stdout. DOS returns AL = DL. */
static int d_print_char(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                        const hydra_backend_t *be, void *st)
{
  (void)ctx;
  uint8_t ch = (uint8_t)(regs->dx & 0xff);
  putc_or_fail(be, st, ch, 0x07);
  regs->ax = (uint16_t)((regs->ax & 0xff00) | ch);
  return 0;
}

/* AH=09h: write the '$'-terminated string at DS:DX. DOS returns AL = 24h. */
static int d_print_string(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                          const hydra_backend_t *be, void *st)
{
  uint32_t lin = (uint32_t)regs->ds * 16u + regs->dx;

  for (;;) {
    if (lin >= NATIVE_HOST_MEM_SIZE)
      FAIL("native host: INT 21h AH=09h string at %04x:%04x runs past the end "
           "of guest memory without a '$' terminator",
           (unsigned)regs->ds, (unsigned)regs->dx);
    uint8_t c = ctx->mem[lin++];
    if (c == '$')
      break;
    putc_or_fail(be, st, c, 0x07);
  }

  regs->ax = (uint16_t)((regs->ax & 0xff00) | 0x24);
  return 0;
}

/* AH=4Ch: terminate with the exit code in AL. */
static int d_exit(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                  const hydra_backend_t *be, void *st)
{
  (void)ctx; (void)regs; (void)be; (void)st;
  return NATIVE_INT_EXIT;
}

/********************************************************************/
/* INT 16h - keyboard services                                       */

/* AH=00h: wait for a keystroke, remove it and return it in AX. */
static int k_read_key(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                      const hydra_backend_t *be, void *st)
{
  (void)ctx;
  uint16_t key = 0;
  if (!be || !be->read_key)
    FAIL("native host: no backend supplies read_key");
  if (!be->read_key(st, &key))
    FAIL("native host: INT 16h AH=00h keyboard input ended without a key");
  regs->ax = key;
  return 0;
}

/* AH=01h: check for a keystroke without removing it. ZF clear with the key
 * in AX when one is pending, ZF set otherwise. Non-blocking by contract. */
static int k_check_key(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                       const hydra_backend_t *be, void *st)
{
  (void)ctx;
  uint16_t key = 0;
  if (!be || !be->poll_key)
    FAIL("native host: no backend supplies poll_key");
  if (be->poll_key(st, &key)) {
    regs->ax = key;
    regs->flags = (uint16_t)(regs->flags & ~FLAG_ZF);
  } else {
    regs->flags = (uint16_t)(regs->flags | FLAG_ZF);
  }
  return 0;
}

/********************************************************************/
/* INT 1Ah - timer services                                          */

/* AH=00h: read the system tick count into CX:DX (high:low). */
static int t_get_ticks(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                       const hydra_backend_t *be, void *st)
{
  (void)ctx;
  if (!be || !be->ticks)
    FAIL("native host: no backend supplies a tick source");
  uint32_t ticks = be->ticks(st);
  regs->cx = (uint16_t)(ticks >> 16);
  regs->dx = (uint16_t)(ticks & 0xffff);
  return 0;
}

/********************************************************************/
/* Tables                                                            */

#define VIDEO_AH_N 0x10u
#define KBD_AH_N   0x08u
#define TIMER_AH_N 0x08u
#define DOS_AH_N   0x80u
#define MOUSE_AH_N 0x40u

static const hydra_int_ah_fn video_ah[VIDEO_AH_N] = {
  [0x00] = v_set_mode,
  [0x02] = v_set_cursor,
  [0x03] = v_get_cursor,
  [0x06] = v_scroll_up,
  [0x07] = v_scroll_down,
  [0x08] = v_read_char,
  [0x09] = v_write_char_attr,
  [0x0a] = v_write_char,
  [0x0e] = v_teletype,
  [0x0f] = v_get_mode,
};

static const hydra_int_ah_fn kbd_ah[KBD_AH_N] = {
  [0x00] = k_read_key,
  [0x01] = k_check_key,
};

static const hydra_int_ah_fn timer_ah[TIMER_AH_N] = {
  [0x00] = t_get_ticks,
};

static const hydra_int_ah_fn dos_ah[DOS_AH_N] = {
  [0x02] = d_print_char,
  [0x09] = d_print_string,
  [0x4c] = d_exit,
};

/* Registered, but every function fails loud: mouse support is Tier 2. */
static const hydra_int_ah_fn mouse_ah[MOUSE_AH_N] = { 0 };

static const int_vector_t INT_TABLE[256] = {
  [0x10] = { "video",    video_ah, VIDEO_AH_N },
  [0x16] = { "keyboard", kbd_ah,   KBD_AH_N   },
  [0x1a] = { "timer",    timer_ah, TIMER_AH_N },
  [0x21] = { "dos",      dos_ah,   DOS_AH_N   },
  [0x33] = { "mouse",    mouse_ah, MOUSE_AH_N },
};

int native_int_dispatch(native_host_ctx_t *ctx,
                        hydra_machine_registers_t *regs,
                        uint8_t num,
                        const hydra_backend_t *be,
                        void *be_state)
{
  const int_vector_t *v = &INT_TABLE[num];

  if (!v->name)
    FAIL("native host: INT %02xh has no dispatch entry (no service model for "
         "this vector on the standalone native host)",
         (unsigned)num);

  uint8_t ah = (uint8_t)((regs->ax >> 8) & 0xff);
  if (ah >= v->n || !v->ah[ah])
    FAIL("native host: INT %02xh (%s) AH=%02xh is not implemented",
         (unsigned)num, v->name, (unsigned)ah);

  return v->ah[ah](ctx, regs, be, be_state);
}
