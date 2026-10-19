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
 * Skeletons are provided for the minimum surface a text-mode, keyboard-driven
 * Tier-1 target needs: video mode set + teletype, DOS print char / print
 * string / exit, timer ticks, and non-blocking key status.
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
  uint8_t ch = (uint8_t)(regs->ax & 0xff);
  uint8_t attr = (uint8_t)(regs->bx & 0xff);
  putc_or_fail(be, st, ch, attr);
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

/* AH=01h: check for a keystroke without removing it. ZF clear with the key
 * in AX when one is pending, ZF set otherwise. Non-blocking by contract. */
static int k_check_key(native_host_ctx_t *ctx, hydra_machine_registers_t *regs,
                       const hydra_backend_t *be, void *st)
{
  (void)ctx;
  uint16_t key = 0;
  if (be && be->poll_key && be->poll_key(st, &key)) {
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
  [0x0e] = v_teletype,
};

static const hydra_int_ah_fn kbd_ah[KBD_AH_N] = {
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
