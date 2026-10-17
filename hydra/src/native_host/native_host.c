/* Native host: a second implementation of the hydra_machine_hardware_t
 * contract (hydra_machine.h), beside dosemu_host/. It is the standalone
 * substrate for executable ports and shares Hydra's discovery seam with the
 * dosemu host: api_impl.c resolves hydra_user_init via dlsym(RTLD_DEFAULT).
 *
 * Skeleton scope (issue #98): init/exec/notify/step_hook are core-owned here;
 * this file only fills the vtable. Memory is the linear guest-memory array
 * from native_host.h. Everything the skeleton does not model fails loud
 * (diagnostic + abort), so the first unmodeled device access stops the run
 * instead of silently faking hardware.
 *
 * Conf string: "native", with optional key/value pairs appended the same way
 * the dosemu host takes them, e.g. "native|code_load=0x0200|data_seg=0x1000".
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "internal.h"
#include "native_host.h"

extern char *HYDRA_CMDLINE_CONF;

const char *native_host_id(void)
{
  return "native host: linear guest-memory array (issue #98 skeleton)";
}

/********************************************************************/
/* conf parsing (why: mirror the dosemu host's key/value style; values
 * accept base-0 numbers so 0x... seg notation works). */

static uint16_t native_conf_u16(const char *conf, const char *key, uint16_t def)
{
  const char *hit = strstr(conf, key);
  if (!hit)
    return def;
  hit += strlen(key);
  char *end = NULL;
  unsigned long long val = strtoull(hit, &end, 0);
  if (end == hit || val > 0xffffull)
    FAIL("native host: conf key %s has no valid u16 value at ...%s", key, hit);
  return (uint16_t)val;
}

/********************************************************************/
/* Fail loud: every stub aborts with a diagnostic naming what is missing. */

static const char *native_conf_string(void)
{
  return HYDRA_CMDLINE_CONF ? HYDRA_CMDLINE_CONF : "";
}

static native_host_ctx_t *native_ctx_of(hydra_machine_ctx_t *ctx)
{
  native_host_ctx_t *nc = (native_host_ctx_t *)ctx;
  if (!nc)
    FAIL("native host: NULL machine context");
  return nc;
}

static void mem_bounds_check(u32 addr, size_t width)
{
  if ((u64)addr + width > NATIVE_HOST_MEM_SIZE)
    FAIL("native host: guest memory access out of bounds: linear=0x%08x width=%zu",
         addr, width);
}

/********************************************************************/
/* mem: bounds-checked accessors on the linear array; 16-bit ops compose
 * little-endian explicitly so the two byte lanes read deterministically. */

static uint8_t *native_mem_hostaddr(hydra_machine_ctx_t *ctx, uint32_t addr)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  mem_bounds_check(addr, 1);
  return &nc->mem[addr];
}

static uint8_t native_mem_read8(hydra_machine_ctx_t *ctx, uint32_t addr)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  mem_bounds_check(addr, 1);
  return nc->mem[addr];
}

static uint16_t native_mem_read16(hydra_machine_ctx_t *ctx, uint32_t addr)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  mem_bounds_check(addr, 2);
  return (uint16_t)(nc->mem[addr] | (nc->mem[addr + 1] << 8));
}

static void native_mem_write8(hydra_machine_ctx_t *ctx, uint32_t addr, uint8_t val)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  mem_bounds_check(addr, 1);
  nc->mem[addr] = val;
}

static void native_mem_write16(hydra_machine_ctx_t *ctx, uint32_t addr, uint16_t val)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  mem_bounds_check(addr, 2);
  nc->mem[addr] = (uint8_t)(val & 0xff);
  nc->mem[addr + 1] = (uint8_t)(val >> 8);
}

/********************************************************************/
/* io: fail loud on every port. Device routing is a later epic deliverable
 * (thin HAL first); until then no port has defined behavior. */

static uint8_t native_io_in8(hydra_machine_ctx_t *ctx, uint16_t port)
{
  (void)ctx;
  FAIL("native host: io_in8 has no device routing yet (port=0x%04x)", port);
}

static uint16_t native_io_in16(hydra_machine_ctx_t *ctx, uint16_t port)
{
  (void)ctx;
  FAIL("native host: io_in16 has no device routing yet (port=0x%04x)", port);
}

static void native_io_out8(hydra_machine_ctx_t *ctx, uint16_t port, uint8_t val)
{
  (void)ctx;
  (void)val;
  FAIL("native host: io_out8 has no device routing yet (port=0x%04x)", port);
}

static void native_io_out16(hydra_machine_ctx_t *ctx, uint16_t port, uint16_t val)
{
  (void)ctx;
  (void)val;
  FAIL("native host: io_out16 has no device routing yet (port=0x%04x)", port);
}

/********************************************************************/
/* state: labeled snapshot of the linear array + register copy. The label is
 * a file path, matching the capture/restore state_path semantics api_impl.c
 * already gives hooks; format: magic + version + sizes + registers + memory
 * (composited little-endian by hand, no struct layout dependency). */

#define NATIVE_HOST_SNAPSHOT_MAGIC 0x4e53534eu /* "NSSN" */
#define NATIVE_HOST_SNAPSHOT_VERSION 1

static void write_full(FILE *f, const void *buf, size_t sz, const char *what,
                       const char *path)
{
  if (fwrite(buf, 1, sz, f) != sz)
    FAIL("native host: state_save write failed (%s): %s", what, path);
}

static void native_state_save(hydra_machine_ctx_t *ctx, const char *path)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  if (!path)
    FAIL("native host: state_save requires a path");

  FILE *f = fopen(path, "wb");
  if (!f)
    FAIL("native host: state_save open failed: %s", path);

  u8 hdr[12];
  hdr[0] = (u8)(NATIVE_HOST_SNAPSHOT_MAGIC & 0xff);
  hdr[1] = (u8)((NATIVE_HOST_SNAPSHOT_MAGIC >> 8) & 0xff);
  hdr[2] = (u8)((NATIVE_HOST_SNAPSHOT_MAGIC >> 16) & 0xff);
  hdr[3] = (u8)((NATIVE_HOST_SNAPSHOT_MAGIC >> 24) & 0xff);
  hdr[4] = (u8)(NATIVE_HOST_SNAPSHOT_VERSION & 0xff);
  hdr[5] = 0;
  hdr[6] = (u8)(NATIVE_HOST_MEM_SIZE & 0xff);
  hdr[7] = (u8)((NATIVE_HOST_MEM_SIZE >> 8) & 0xff);
  hdr[8] = (u8)((NATIVE_HOST_MEM_SIZE >> 16) & 0xff);
  hdr[9] = (u8)((NATIVE_HOST_MEM_SIZE >> 24) & 0xff);
  hdr[10] = (u8)(sizeof(nc->regs) & 0xff);
  hdr[11] = 0;

  write_full(f, hdr, sizeof(hdr), "header", path);
  write_full(f, &nc->regs, sizeof(nc->regs), "registers", path);
  write_full(f, nc->mem, NATIVE_HOST_MEM_SIZE, "memory", path);

  if (fclose(f) != 0)
    FAIL("native host: state_save close failed: %s", path);
}

static void read_full(FILE *f, void *buf, size_t sz, const char *what,
                      const char *path)
{
  if (fread(buf, 1, sz, f) != sz)
    FAIL("native host: state_restore short read (%s): %s", what, path);
}

static void native_state_restore(hydra_machine_ctx_t *ctx, const char *path)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  if (!path)
    FAIL("native host: state_restore requires a path");

  FILE *f = fopen(path, "rb");
  if (!f)
    FAIL("native host: state_restore open failed: %s", path);

  u8 hdr[12];
  read_full(f, hdr, sizeof(hdr), "header", path);
  u32 magic = (u32)hdr[0] | ((u32)hdr[1] << 8) | ((u32)hdr[2] << 16) |
              ((u32)hdr[3] << 24);
  u32 version = (u32)hdr[4] | ((u32)hdr[5] << 8);
  u32 mem_size = (u32)hdr[6] | ((u32)hdr[7] << 8) | ((u32)hdr[8] << 16) |
                 ((u32)hdr[9] << 24);
  u32 regs_size = (u32)hdr[10] | ((u32)hdr[11] << 8);
  if (magic != NATIVE_HOST_SNAPSHOT_MAGIC)
    FAIL("native host: state_restore bad magic: %s", path);
  if (version != NATIVE_HOST_SNAPSHOT_VERSION)
    FAIL("native host: state_restore version %u unsupported: %s", version, path);
  if (mem_size != NATIVE_HOST_MEM_SIZE)
    FAIL("native host: state_restore memory size %u mismatched: %s", mem_size,
         path);
  if (regs_size != sizeof(nc->regs))
    FAIL("native host: state_restore registers size %u mismatched: %s", regs_size,
         path);

  read_full(f, &nc->regs, sizeof(nc->regs), "registers", path);
  read_full(f, nc->mem, NATIVE_HOST_MEM_SIZE, "memory", path);

  if (fclose(f) != 0)
    FAIL("native host: state_restore close failed: %s", path);
}

/********************************************************************/
/* update_registers: mirror the host-side register copy into the machine. */

static void native_update_registers(hydra_machine_ctx_t *ctx,
                                    hydra_machine_registers_t *registers)
{
  native_host_ctx_t *nc = native_ctx_of(ctx);
  *registers = nc->regs;
}

/********************************************************************/
/* Metadata stubs: api_impl.c binds hydra_user_functions /            */
/* hydra_user_callstack via dlsym(RTLD_DEFAULT) and aborts when absent,   */
/* so the native host must own empty-metadata copies of both (same     */
/* rationale as the dosemu host: hosts that load a metadata .so later   */
/* can inject richer tables through the *_metadata_set functions).     */
/* Both are weak: a linked-in real table (e.g. confgen-generated       */
/* appdata in a native runner) must override the empty defaults        */
/* instead of colliding with them at link time.                        */

__attribute__((weak))
const hydra_function_metadata_t *hydra_user_functions(void)
{
  static const hydra_function_metadata_t md = {0, NULL};
  return &md;
}

__attribute__((weak))
const hydra_callstack_metadata_t *hydra_user_callstack(void)
{
  static const hydra_callstack_metadata_t md = {0, NULL};
  return &md;
}

/********************************************************************/
/* hydra_user_init: the entry point Hydra discovers via dlsym (api_impl.c).
 * Same signature as the dosemu host: (conf, hw, audio) - not (hw, audio,
 * conf). Default conf is the dosemu host's: code_load=0, data_seg=0; both
 * must be set by the user init or api_impl.c aborts, so they are always
 * assigned here. Raw-code reservation is never guessed (dosemu host rule). */

void hydra_user_init(hydra_conf_t *conf,
                     hydra_machine_hardware_t *hw,
                     hydra_machine_audio_t *audio)
{
  (void)audio;

  const char *confstr = native_conf_string();
  native_host_ctx_t *ctx = calloc(1, sizeof(*ctx));
  if (!ctx)
    FAIL("native host: out of memory");

  conf->raw_code_offset = 0;
  conf->raw_code_size = 0;
  conf->code_load_offset = native_conf_u16(confstr, "code_load=", 0);
  conf->data_section_seg = native_conf_u16(confstr, "data_seg=", 0);

  hw->ctx = (hydra_machine_ctx_t *)ctx;
  hw->mem_hostaddr = native_mem_hostaddr;
  hw->mem_read8 = native_mem_read8;
  hw->mem_read16 = native_mem_read16;
  hw->mem_write8 = native_mem_write8;
  hw->mem_write16 = native_mem_write16;
  hw->io_in8 = native_io_in8;
  hw->io_in16 = native_io_in16;
  hw->io_out8 = native_io_out8;
  hw->io_out16 = native_io_out16;
  hw->state_save = native_state_save;
  hw->state_restore = native_state_restore;
  hw->update_registers = native_update_registers;
}
