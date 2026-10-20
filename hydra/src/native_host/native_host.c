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
#include <time.h>

#include "internal.h"
#include "native_host.h"
#include "hydra_backend.h"

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
/* io: port -> device routing (issue #95).
 *
 * Deliberately not part of the backend vtable: an entry there would push
 * port-number knowledge into every backend, including ones with no concept
 * of a port. The native host owns a small table instead. Each device either
 * reuses a backend method (the keyboard reads through poll_key/read_key) or
 * owns its own state (the PIT keeps a start timestamp).
 *
 * Unmapped ports still fail loud. That is the point of doing it this way:
 * routing keyboard and timer traffic must not become a path where an unknown
 * device quietly returns a plausible-looking value.
 *
 * Device state is process-wide, like raw_code_slot above: the host is a
 * singleton and there is no second guest to keep separate.
 */

typedef struct {
  const char *name;
  uint16_t    base;    /* first port */
  uint16_t    count;   /* consecutive ports covered */
  uint8_t   (*read8)(void *dev, const hydra_backend_t *be, void *be_st,
                     uint16_t port);
  void      (*write8)(void *dev, const hydra_backend_t *be, void *be_st,
                      uint16_t port, uint8_t val);
  void       *dev;     /* device-owned state */
} port_device_t;

/* --- keyboard (8042): 0x60 data, 0x64 status ---------------------- */

typedef struct {
  int warned_empty;    /* one-shot diagnostic, see kbd_read_data */
} kbd_dev_t;

static kbd_dev_t g_kbd;

/* Only bit 0 (output buffer full) is modelled, because that is what a
 * polling game tests. Bit 1 (input buffer full) stays clear so a game that
 * waits for the controller to be ready is never stuck. */
static uint8_t kbd_read_status(void *dev, const hydra_backend_t *be,
                               void *be_st, uint16_t port)
{
  (void)dev;
  (void)port;
  uint16_t key = 0;
  if (!be->poll_key)
    return 0;
  return be->poll_key(be_st, &key) ? 0x01u : 0x00u;
}

static uint8_t kbd_read_data(void *dev, const hydra_backend_t *be,
                             void *be_st, uint16_t port)
{
  (void)port;
  kbd_dev_t *k = (kbd_dev_t *)dev;
  uint16_t key = 0;

  /* Non-blocking on purpose. read_key() blocks until a key arrives, and a
   * game that reads 0x60 without checking 0x64 first would hang forever.
   * Report the empty read once rather than failing silently. */
  if (!be->poll_key || !be->poll_key(be_st, &key)) {
    if (!k->warned_empty) {
      k->warned_empty = 1;
      fprintf(stderr,
              "native host: port 0x60 read with no key pending; returning 0 "
              "(a guest that polls the data port without checking 0x64 will "
              "see no keys)\n");
    }
    return 0;
  }
  if (be->read_key)
    be->read_key(be_st, &key);
  /* Port 0x60 carries the scan code; the backend's key word is BIOS
   * encoded (high byte = scan, low byte = ASCII). */
  return (uint8_t)(key >> 8);
}

static void kbd_write(void *dev, const hydra_backend_t *be, void *be_st,
                      uint16_t port, uint8_t val)
{
  (void)dev;
  (void)be;
  (void)be_st;
  (void)port;
  (void)val;
  /* Controller commands are absorbed: the backend model has no 8042 to
   * configure, and failing here would stop every guest that sets the
   * keyboard up at boot. */
}

/* --- PIT (8253/8254): 0x40-0x42 counters, 0x43 control ------------ */

#define PIT_HZ 1193182ull

typedef struct {
  struct timespec start;
  uint16_t reload[3];
  uint8_t  read_hi[3];   /* next byte to return: 0 = low, 1 = high */
  uint8_t  write_hi[3];  /* low/high sequencing of a reload write */
  uint8_t  mode[3];      /* access mode from the control word */
} pit_dev_t;

static pit_dev_t g_pit;

static uint64_t pit_elapsed_ticks(const pit_dev_t *p)
{
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC, &now);
  uint64_t ns = (uint64_t)(now.tv_sec - p->start.tv_sec) * 1000000000ull
              + (uint64_t)(now.tv_nsec - p->start.tv_nsec);

  /* ns * PIT_HZ would overflow, so scale seconds and remainder apart. */
  uint64_t sec = ns / 1000000000ull;
  uint64_t rem = ns % 1000000000ull;
  return (sec % 4000000000ull) * PIT_HZ + rem * PIT_HZ / 1000000000ull;
}

static uint16_t pit_count(const pit_dev_t *p, int ch)
{
  uint32_t period = p->reload[ch] ? p->reload[ch] : 0x10000u;
  return (uint16_t)(period - (uint16_t)(pit_elapsed_ticks(p) % period));
}

static uint8_t pit_read(void *dev, const hydra_backend_t *be, void *be_st,
                        uint16_t port)
{
  (void)be;
  (void)be_st;
  pit_dev_t *p = (pit_dev_t *)dev;
  int ch = (int)(port - 0x40);
  uint16_t c = pit_count(p, ch);
  uint8_t byte = p->read_hi[ch] ? (uint8_t)(c >> 8) : (uint8_t)(c & 0xffu);
  p->read_hi[ch] ^= 1u;
  return byte;
}

static void pit_write_control(void *dev, const hydra_backend_t *be,
                              void *be_st, uint16_t port, uint8_t val)
{
  (void)be;
  (void)be_st;
  (void)port;
  pit_dev_t *p = (pit_dev_t *)dev;
  uint8_t ch = (uint8_t)((val >> 6) & 3u);

  if (ch == 3)                     /* read-back command: not modelled */
    return;
  p->mode[ch] = (uint8_t)((val >> 4) & 3u);
  p->read_hi[ch] = 0;
  p->write_hi[ch] = 0;             /* mode 0 = counter latch, also resets */
}

static void pit_write_counter(void *dev, const hydra_backend_t *be,
                              void *be_st, uint16_t port, uint8_t val)
{
  (void)be;
  (void)be_st;
  pit_dev_t *p = (pit_dev_t *)dev;
  int ch = (int)(port - 0x40);
  uint8_t mode = p->mode[ch] ? p->mode[ch] : 3u;

  if (mode == 1) {                          /* low byte only */
    p->reload[ch] = val;
    clock_gettime(CLOCK_MONOTONIC, &p->start);
  } else if (mode == 2) {                   /* high byte only */
    p->reload[ch] = (uint16_t)(val << 8);
    clock_gettime(CLOCK_MONOTONIC, &p->start);
  } else if (p->write_hi[ch] == 0) {        /* low, then high */
    p->reload[ch] = (uint16_t)((p->reload[ch] & 0xff00u) | val);
    p->write_hi[ch] = 1;
  } else {
    p->reload[ch] = (uint16_t)((p->reload[ch] & 0x00ffu)
                               | ((uint16_t)val << 8));
    p->write_hi[ch] = 0;
    clock_gettime(CLOCK_MONOTONIC, &p->start);
  }
}

/* --- the map ------------------------------------------------------ */

static const port_device_t PORT_TABLE[] = {
  /* counters first: 0x43 is inside 0x40..0x42 only if count is 4, and the
   * control register must not answer a read, so it is a separate entry. */
  { "pit-counter-0..2", 0x40, 3, pit_read,          pit_write_counter, &g_pit },
  { "pit-control",      0x43, 1, NULL,              pit_write_control, &g_pit },
  { "keyboard-data",    0x60, 1, kbd_read_data,     kbd_write,         &g_kbd },
  { "keyboard-status",  0x64, 1, kbd_read_status,   kbd_write,         &g_kbd },
};

static const port_device_t *port_lookup(uint16_t port)
{
  for (size_t i = 0; i < sizeof(PORT_TABLE) / sizeof(PORT_TABLE[0]); i++) {
    const port_device_t *d = &PORT_TABLE[i];
    if (port >= d->base && port < d->base + d->count)
      return d;
  }
  return NULL;
}

static void port_devices_init(void)
{
  memset(&g_kbd, 0, sizeof(g_kbd));
  memset(&g_pit, 0, sizeof(g_pit));
  /* A zero reload means "full period" to a real 8253, which pit_count()
   * already maps to 0x10000, so leaving it zero is the correct default. */
  clock_gettime(CLOCK_MONOTONIC, &g_pit.start);
}

static uint8_t native_io_in8(hydra_machine_ctx_t *ctx, uint16_t port)
{
  (void)ctx;
  const port_device_t *d = port_lookup(port);
  if (!d)
    FAIL("native host: io_in8 has no device routing yet (port=0x%04x)", port);
  if (!d->read8)
    FAIL("native host: port 0x%04x (%s) is write-only", port, d->name);

  void *be_st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&be_st);
  return d->read8(d->dev, be, be_st, port);
}

static void native_io_out8(hydra_machine_ctx_t *ctx, uint16_t port, uint8_t val)
{
  (void)ctx;
  const port_device_t *d = port_lookup(port);
  if (!d)
    FAIL("native host: io_out8 has no device routing yet (port=0x%04x)", port);
  if (!d->write8)
    FAIL("native host: port 0x%04x (%s) is read-only", port, d->name);

  void *be_st = NULL;
  const hydra_backend_t *be = hydra_native_backend_active(&be_st);
  d->write8(d->dev, be, be_st, port, val);
}

/* A 16-bit access to an 8-bit device is two bus cycles on real hardware, so
 * route it that way rather than inventing a word-level protocol. An unmapped
 * half still fails loud, which keeps io_in16/io_out16 honest. */
static uint16_t native_io_in16(hydra_machine_ctx_t *ctx, uint16_t port)
{
  uint8_t lo = native_io_in8(ctx, port);
  uint8_t hi = native_io_in8(ctx, (uint16_t)(port + 1));
  return (uint16_t)(lo | ((uint16_t)hi << 8));
}

static void native_io_out16(hydra_machine_ctx_t *ctx, uint16_t port, uint16_t val)
{
  native_io_out8(ctx, port, (uint8_t)(val & 0xffu));
  native_io_out8(ctx, (uint16_t)(port + 1), (uint8_t)(val >> 8));
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

  /* Select the device backend here, once, before any guest code can run.
   * Interrupt dispatch then only reads the registry instead of initializing
   * it from a hook thread (see active_backend() in backend_host.c). No
   * backend-selection knob exists yet, so this always picks the default. */
  hydra_native_backend_select(NULL);

  /* Port devices have to be reset here too, or the PIT's start timestamp
   * would be whatever the previous run left behind. */
  port_devices_init();
}
