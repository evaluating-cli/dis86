
typedef struct hydra_conf hydra_conf_t;
struct hydra_conf
{
  u16 code_load_offset;
  u16 data_section_seg;
  /* Absolute linear address of the raw-code scratch region. Each snippet of
   * guest opcode (hydra_impl_raw_code) is placed in a fresh 128-byte slot
   * here (monotonically increasing within one hook dispatch, reset at each
   * hook boundary via hydra_impl_raw_code_reset) so the simx86 JIT never
   * serves a stale cached translation. Must be 16-byte aligned RAM not used
   * by the guest. */
  u32 raw_code_offset;
};
