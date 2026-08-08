#include "internal.h"

static u16 overlay_segments[64] = {};
static bool overlay_valid[64] = {false};

void hydra_overlay_segment_set(u16 overlay_num, u16 segment)
{
  assert(overlay_num < ARRAY_SIZE(overlay_segments));
  overlay_segments[overlay_num] = segment;
  overlay_valid[overlay_num] = true;
}

void hydra_overlay_segment_clear(u16 overlay_num)
{
  assert(overlay_num < ARRAY_SIZE(overlay_segments));
  overlay_valid[overlay_num] = false;
}

u16 hydra_overlay_segment_lookup(u16 overlay_num)
{
  assert(overlay_num < ARRAY_SIZE(overlay_segments));
  if (!overlay_valid[overlay_num]) FAIL("Overlay %u has no known physical segment", overlay_num);
  return overlay_segments[overlay_num];
}

addr_t hydra_overlay_segment_remap_from_physical(addr_t addr)
{
  // Only remap non-overlay addrs into overlay addrs
  if (addr_is_overlay(addr)) {
    return addr;
  }

  for (size_t i = 0; i < ARRAY_SIZE(overlay_segments); i++) {
    if (!overlay_valid[i]) continue;
    if (overlay_segments[i] == addr_seg(addr)) {
      return ADDR_MAKE_EXT(1, (u16)i, addr_off(addr));
    }
  }

  return addr;
}
