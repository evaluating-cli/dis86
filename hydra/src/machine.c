#include "internal.h"

void hydra_impl_unknown(const char *func, int line)
{
  fprintf(stderr, "FAIL: UNKNOWN INSTRUCTION: UNIMPL AT %s:%d\n", func, line);
  abort();                                                              \
}

#define U32_MAKE(upper, lower) ((u32)(upper) << 16 | (u32)(lower))

/* Raw-code slot counter (see hydra_impl_raw_code). Slots are process-monotonic:
 * simx86 caches translated code by guest linear address and external memfd
 * writes do not invalidate that cache, so an address that has executed raw
 * code must never be reused by this host process. */
static u32 raw_code_slot = 0;

u32 hydra_impl_call_far(u16 seg, u16 off)
{
  u16 exec_id = 0;
  hydra_exec_ctx_t *exec = execution_context_get(&exec_id);

  hydra_machine_t *m = &exec->machine;
  u32 addr_off = (u32)m->registers->ss * 16 + (u16)(m->registers->sp - 2);
  u32 addr_seg = (u32)m->registers->ss * 16 + (u16)(m->registers->sp - 4);
  m->hardware->mem_write16(m->hardware->ctx, addr_off, 0xffff);
  m->hardware->mem_write16(m->hardware->ctx, addr_seg, exec_id);
  m->registers->sp -= 4;

  exec->saved_cs = m->registers->cs;
  exec->saved_ip = m->registers->ip;
  exec->maybe_reloc = 0;

  exec->result.type = HYDRA_RESULT_TYPE_CALL;
  exec->result.new_cs = seg;
  exec->result.new_ip = off;

  pthread_cond_signal(exec->cond_main);
  pthread_cond_wait(exec->cond_child, exec->mutex);

  execution_context_set(exec);
  m->registers->cs = exec->saved_cs;
  m->registers->ip = exec->saved_ip;

  return U32_MAKE(m->registers->dx, m->registers->ax);
}

u32 hydra_impl_call_near_off(u16 off, int maybe_reloc)
{
  u16 exec_id = 0;
  hydra_exec_ctx_t *exec = execution_context_get(&exec_id);

  assert(exec_id <= 255);

  hydra_machine_t *m = &exec->machine;
  u32 addr = (u32)m->registers->ss * 16 + m->registers->sp;
  m->hardware->mem_write16(m->hardware->ctx, addr - 2, 0xff00 + exec_id);
  m->registers->sp -= 2;

  exec->saved_cs = m->registers->cs;
  exec->saved_ip = m->registers->ip;
  exec->maybe_reloc = maybe_reloc;

  assert(m->registers->cs >= CODE_START_SEG);
  exec->result.type = HYDRA_RESULT_TYPE_CALL_NEAR;
  exec->result.new_ip = off;

  pthread_cond_signal(exec->cond_main);
  pthread_cond_wait(exec->cond_child, exec->mutex);

  if (!maybe_reloc) {
    assert(m->registers->cs == exec->saved_cs);
  }

  execution_context_set(exec);
  m->registers->ip = exec->saved_ip;

  return U32_MAKE(m->registers->dx, m->registers->ax);
}

u32 hydra_impl_call_near_abs(u16 abs_off)
{
  u16 exec_id = 0;
  hydra_exec_ctx_t *exec = execution_context_get(&exec_id);
  hydra_machine_t *m = &exec->machine;

  return hydra_impl_call_near_off(abs_off - 16*(m->registers->cs - CODE_START_SEG), 0);
}

u32 hydra_impl_call_far_cs(u16 cs_reg_value, u16 off)
{
  assert(cs_reg_value >= CODE_START_SEG);
  return hydra_impl_call_far(cs_reg_value - CODE_START_SEG, off);
}

u32 hydra_impl_call_far_indirect(u32 addr)
{
  u16 seg = addr>>16;
  u16 off = addr;
  assert(seg >= CODE_START_SEG);
  return hydra_impl_call_far(seg - CODE_START_SEG, off);
}

void hydra_impl_raw_code(u8 *code, size_t code_sz)
{
#define MAX_RAW_CODE 128
  assert(code_sz <= MAX_RAW_CODE);

  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  /* External memfd writes do not invalidate simx86 translations. Every snippet
     therefore uses a fresh 128-byte slot within an explicitly guest-reserved
     region. Never reuse or wrap a slot inside this host process. */
  if (HYDRA_CONF->raw_code_size < MAX_RAW_CODE)
    FAIL("No Hydra raw-code region has been reserved by the guest/launcher");

  const u32 max_slots = HYDRA_CONF->raw_code_size / MAX_RAW_CODE;
  if (raw_code_slot >= max_slots)
    FAIL("Hydra raw-code reservation exhausted after %u snippets", raw_code_slot);

  u32 slot_addr = HYDRA_CONF->raw_code_offset + raw_code_slot * MAX_RAW_CODE;
  raw_code_slot++;

  u8 code_saved[MAX_RAW_CODE];
  u16 code_seg = (u16)(slot_addr >> 4);
  u16 code_off = (u16)(slot_addr & 0xf);
  u8 *code_ptr = m->hardware->mem_hostaddr(m->hardware->ctx, slot_addr);
  if (!code_ptr)
    FAIL("Hydra raw-code slot 0x%x is outside mapped guest memory", slot_addr);

  memcpy(code_saved, code_ptr, MAX_RAW_CODE);
  memcpy(code_ptr, code, code_sz);

  hydra_impl_call_far(code_seg - CODE_START_SEG, code_off);

  memcpy(code_ptr, code_saved, MAX_RAW_CODE);
}

void hydra_impl_raw_code_reset(void)
{
  /* Legacy API retained for source compatibility. This is intentionally a
   * no-op on the dosemu2 external host: resetting would reuse a guest linear
   * address whose simx86 translation may still be cached. */
}

/* NOP/CLD/STD/CLI/STI touch no operands and no memory. Hosts that can execute
 * raw snippets (dosemu2 reserves a guest raw-code region) keep the historical
 * path, so the emulated CPU state and dispatch accounting are unchanged.
 * Hosts with no guest CPU to run snippets (the standalone native host) have no
 * raw-code region; there these five ops are applied straight to the mirrored
 * flag register, which the host propagates to its own state. */
static bool hydra_raw_code_available(void)
{
  return HYDRA_CONF->raw_code_size >= MAX_RAW_CODE;
}

void hydra_impl_nop(void)
{
  if (hydra_raw_code_available()) {
    u8 code[] = {0x90, 0xcb};
    hydra_impl_raw_code(code, ARRAY_SIZE(code));
  }
}

void hydra_impl_cld(void)
{
  if (hydra_raw_code_available()) {
    u8 machine_code[] = {0xfc, 0xcb};
    hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));
    return;
  }
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;
  FLAGS_DF_OFF();
}

void hydra_impl_std(void)
{
  if (hydra_raw_code_available()) {
    u8 machine_code[] = {0xfd, 0xcb};
    hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));
    return;
  }
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;
  FLAGS_DF_ON();
}

void hydra_impl_cli(void)
{
  if (hydra_raw_code_available()) {
    u8 machine_code[] = {0xfa, 0xcb};
    hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));
    return;
  }
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;
  FLAGS_IF_OFF();
}

void hydra_impl_sti(void)
{
  if (hydra_raw_code_available()) {
    u8 machine_code[] = {0xfb, 0xcb};
    hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));
    return;
  }
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;
  FLAGS_IF_ON();
}

u8 hydra_impl_inb(u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ax = m->registers->ax;
  u16 save_dx = m->registers->dx;
  m->registers->dx = port;

  u8 machine_code[] = {0xec, 0xcb};
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  u8 ret = (u8)m->registers->ax;
  m->registers->ax = save_ax;
  m->registers->dx = save_dx;
  return ret;
}

void hydra_impl_outb(u16 port, u8 val)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ax = m->registers->ax;
  u16 save_dx = m->registers->dx;
  m->registers->ax = (u16)val;
  m->registers->dx = port;

  u8 machine_code[] = {0xee, 0xcb};
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ax = save_ax;
  m->registers->dx = save_dx;
}

void hydra_impl_int(u8 num)
{
  if (hydra_raw_code_available()) {
    u8 machine_code[] = {0xcd, num, 0xcb};
    hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));
    return;
  }

  /* No guest CPU can execute the `int` here: this host reserved no raw-code
   * region. Hand the interrupt to the host's device layer instead (dlsym
   * hook, see internal.h). Guarded the same way as nop/cld/std/cli/sti, so
   * hosts that do execute raw code take the unchanged path above. */
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  int rc = hydra_user_int_hook ? hydra_user_int_hook(m, num) : -1;
  if (rc == 0)
    return;
  if (rc > 0)
    FAIL("native host: the handler for INT %02xh requested guest termination "
         "(exit code %u) but the native host has no exit path wired yet",
         (unsigned)num, (unsigned)(m->registers->ax & 0xff));
  FAIL("native host: INT %02xh was not serviced: no host handler is installed and "
       "no raw-code region is reserved", (unsigned)num);
}

u16 hydra_impl_inw(u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ax = m->registers->ax;
  u16 save_dx = m->registers->dx;
  m->registers->dx = port;

  // in ax, dx; ret
  u8 machine_code[] = {0xed, 0xcb};
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  u16 ret = m->registers->ax;
  m->registers->ax = save_ax;
  m->registers->dx = save_dx;
  return ret;
}

void hydra_impl_outw(u16 port, u16 val)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ax = m->registers->ax;
  u16 save_dx = m->registers->dx;
  m->registers->ax = val;
  m->registers->dx = port;

  // out dx, ax; ret
  u8 machine_code[] = {0xef, 0xcb};
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ax = save_ax;
  m->registers->dx = save_dx;
}

/* String port I/O. Each helper sets the addressing/port registers, executes
 * the raw instruction prefixed with CLD (forcing the forward direction the
 * decompiler models, as the other string helpers do), then restores the
 * registers: the decompiler models the SI/DI/CX updates itself. */
void hydra_impl_insb(u16 es, u16 di, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_es = m->registers->es;
  u16 save_di = m->registers->di;
  u16 save_dx = m->registers->dx;
  m->registers->es = es;
  m->registers->di = di;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0x6c, 0xcb}; // cld; insb; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->es = save_es;
  m->registers->di = save_di;
  m->registers->dx = save_dx;
}

void hydra_impl_insw(u16 es, u16 di, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_es = m->registers->es;
  u16 save_di = m->registers->di;
  u16 save_dx = m->registers->dx;
  m->registers->es = es;
  m->registers->di = di;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0x6d, 0xcb}; // cld; insw; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->es = save_es;
  m->registers->di = save_di;
  m->registers->dx = save_dx;
}

void hydra_impl_outsb(u16 ds, u16 si, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ds = m->registers->ds;
  u16 save_si = m->registers->si;
  u16 save_dx = m->registers->dx;
  m->registers->ds = ds;
  m->registers->si = si;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0x6e, 0xcb}; // cld; outsb; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ds = save_ds;
  m->registers->si = save_si;
  m->registers->dx = save_dx;
}

void hydra_impl_outsw(u16 ds, u16 si, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ds = m->registers->ds;
  u16 save_si = m->registers->si;
  u16 save_dx = m->registers->dx;
  m->registers->ds = ds;
  m->registers->si = si;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0x6f, 0xcb}; // cld; outsw; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ds = save_ds;
  m->registers->si = save_si;
  m->registers->dx = save_dx;
}

void hydra_impl_rep_insb(u16 es, u16 di, u16 cx, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_es = m->registers->es;
  u16 save_di = m->registers->di;
  u16 save_cx = m->registers->cx;
  u16 save_dx = m->registers->dx;
  m->registers->es = es;
  m->registers->di = di;
  m->registers->cx = cx;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0xf3, 0x6c, 0xcb}; // cld; rep insb; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->es = save_es;
  m->registers->di = save_di;
  m->registers->cx = save_cx;
  m->registers->dx = save_dx;
}

void hydra_impl_rep_insw(u16 es, u16 di, u16 cx, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_es = m->registers->es;
  u16 save_di = m->registers->di;
  u16 save_cx = m->registers->cx;
  u16 save_dx = m->registers->dx;
  m->registers->es = es;
  m->registers->di = di;
  m->registers->cx = cx;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0xf3, 0x6d, 0xcb}; // cld; rep insw; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->es = save_es;
  m->registers->di = save_di;
  m->registers->cx = save_cx;
  m->registers->dx = save_dx;
}

void hydra_impl_rep_outsb(u16 ds, u16 si, u16 cx, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ds = m->registers->ds;
  u16 save_si = m->registers->si;
  u16 save_cx = m->registers->cx;
  u16 save_dx = m->registers->dx;
  m->registers->ds = ds;
  m->registers->si = si;
  m->registers->cx = cx;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0xf3, 0x6e, 0xcb}; // cld; rep outsb; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ds = save_ds;
  m->registers->si = save_si;
  m->registers->cx = save_cx;
  m->registers->dx = save_dx;
}

void hydra_impl_rep_outsw(u16 ds, u16 si, u16 cx, u16 port)
{
  hydra_exec_ctx_t *exec = execution_context_get(NULL);
  hydra_machine_t *m = &exec->machine;

  u16 save_ds = m->registers->ds;
  u16 save_si = m->registers->si;
  u16 save_cx = m->registers->cx;
  u16 save_dx = m->registers->dx;
  m->registers->ds = ds;
  m->registers->si = si;
  m->registers->cx = cx;
  m->registers->dx = port;

  u8 machine_code[] = {0xfc, 0xf3, 0x6f, 0xcb}; // cld; rep outsw; ret
  hydra_impl_raw_code(machine_code, ARRAY_SIZE(machine_code));

  m->registers->ds = save_ds;
  m->registers->si = save_si;
  m->registers->cx = save_cx;
  m->registers->dx = save_dx;
}

uint32_t hydra_impl_ptr_to_flataddr(hydra_machine_t *m, void *_ptr)
{
  uint8_t * ptr = (uint8_t*)_ptr;

  uint32_t min_addr = 0x8000;
  uint32_t max_addr = 0x9f000;
  uint8_t * min_ptr = m->hardware->mem_hostaddr(m->hardware->ctx, min_addr);
  uint8_t * max_ptr = min_ptr - min_addr + max_addr;
  if (!(min_ptr <= ptr && ptr < max_ptr)) FAIL("Invalid pointer in PTR_TO_ADDR: %p\n", ptr);

  return min_addr + (ptr - min_ptr);
}

addr_t hydra_impl_ptr_to_addr(hydra_machine_t *m, void *ptr)
{
  uint32_t addr = hydra_impl_ptr_to_flataddr(m, ptr);
  assert(addr <= 1<<20);

  addr_t ret = ADDR_MAKE(addr>>4, addr&15);
  return ret;
}

uint16_t hydra_impl_ptr_to_off(hydra_machine_t *m, void *ptr, uint16_t seg)
{
  uint32_t addr = hydra_impl_ptr_to_flataddr(m, ptr);
  assert(addr <= 1<<20);

  uint32_t seg_start = (uint32_t)seg * 16;
  uint32_t seg_end   = seg_start + (1<<16);

  if (!(seg_start <= addr && addr < seg_end)) {
    FAIL("Address 0x%08x is not in segment 0x%04x", addr, seg_start);
  }

  return (uint16_t)(addr - seg_start);
}

uint32_t hydra_impl_ptr_to_32(hydra_machine_t *m, void *ptr)
{
  addr_t s = hydra_impl_ptr_to_addr(m, ptr);
  return (uint32_t)addr_seg(s) << 16 | addr_off(s);
}
