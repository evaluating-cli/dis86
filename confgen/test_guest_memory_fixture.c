#include <assert.h>
#include <stdint.h>
#include <stdio.h>

typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
static u8 memory[16][65536];

static u8 mem_read8(u16 seg, u16 off) { return memory[seg & 15][off]; }
static u16 mem_read16(u16 seg, u16 off) {
  return (u16)(mem_read8(seg, off) | ((u16)mem_read8(seg, (u16)(off + 1)) << 8));
}
static u32 mem_read32(u16 seg, u16 off) {
  return (u32)mem_read16(seg, off) | ((u32)mem_read16(seg, (u16)(off + 2)) << 16);
}
static void mem_write8(u16 seg, u16 off, u8 v) { memory[seg & 15][off] = v; }
static void mem_write16(u16 seg, u16 off, u16 v) {
  mem_write8(seg, off, (u8)v); mem_write8(seg, (u16)(off + 1), (u8)(v >> 8));
}
static void mem_write32(u16 seg, u16 off, u32 v) {
  mem_write16(seg, off, (u16)v); mem_write16(seg, (u16)(off + 2), (u16)(v >> 16));
}

#define LOAD_8(seg, off) mem_read8((seg), (off))
#define LOAD_16(seg, off) mem_read16((seg), (off))
#define LOAD_32(seg, off) mem_read32((seg), (off))
#define STORE_8(seg, off, val) mem_write8((seg), (off), (u8)(val))
#define STORE_16(seg, off, val) mem_write16((seg), (off), (u16)(val))
#define STORE_32(seg, off, val) mem_write32((seg), (off), (u32)(val))

int main(void) {
  const u16 DS = 1, SS = 2, ES = 3;
  const u16 near_ds = 0x0100, near_ss = 0x0200, near_es = 0x0300;
  const u32 far_ptr = ((u32)4 << 16) | 0x0400;
  STORE_32(DS, 0x0050, far_ptr); /* packed guest far-pointer field */
  const u32 loaded_far = LOAD_32(DS, 0x0050);

  STORE_16(DS, near_ds, 0x1111);
  STORE_16(SS, near_ss, 0x2222);
  STORE_16(ES, near_es, 0x3333);
  STORE_16(loaded_far >> 16, (u16)loaded_far, 0x4444);
  assert(LOAD_16(DS, near_ds) == 0x1111);
  assert(LOAD_16(SS, near_ss) == 0x2222);
  assert(LOAD_16(ES, near_es) == 0x3333);
  assert(LOAD_16(loaded_far >> 16, (u16)loaded_far) == 0x4444);

  /* p[i] for u16 elements and nested row/column arrays. */
  u16 p = 0x1000;
  STORE_16(DS, (u16)(p + 3 * sizeof(u16)), 0x5555);
  assert(LOAD_16(DS, (u16)(p + 3 * sizeof(u16))) == 0x5555);
  u16 grid = 0x2000;
  STORE_16(DS, (u16)(grid + (1 * 4 + 2) * sizeof(u16)), 0x6666);
  assert(LOAD_16(DS, (u16)(grid + (1 * 4 + 2) * sizeof(u16))) == 0x6666);

  /* Nested struct field at a packed byte offset. */
  u16 records = 0x3000;
  const u16 record_size = 6, field_off = 4;
  STORE_16(DS, (u16)(records + record_size + field_off), 0x7777);
  assert(LOAD_16(DS, (u16)(records + record_size + field_off)) == 0x7777);
  puts("guest memory fixture passed");
}
