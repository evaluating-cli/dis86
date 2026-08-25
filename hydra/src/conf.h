
typedef struct hydra_conf hydra_conf_t;
struct hydra_conf
{
  u16 code_load_offset;
  u16 data_section_seg;
  /* Absolute linear address + byte size of a scratch region explicitly
   * reserved by the guest/launcher for Hydra raw-code execution. The dosemu2
   * host does not guess or commandeer a low-memory address. Each snippet uses
   * a fresh 128-byte slot within this reservation so simx86 cannot serve a
   * stale translation after external memfd writes. */
  u32 raw_code_offset;
  u32 raw_code_size;
};
