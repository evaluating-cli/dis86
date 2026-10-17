#pragma once
#include <stddef.h>
#include <stdint.h>

#include "hydra_machine.h"

/* Linear guest-memory model: the guest's addressable space is one flat
 * array and a guest linear address equals its index in that array. This is
 * the plainest starting point for standalone executable ports; segment-aware
 * memory (preserved-segment semantics) remains a per-port option by building
 * on this vtable contract, not by replacing it. */

#define NATIVE_HOST_MEM_SIZE (1024u * 1024u)

typedef struct native_host_ctx native_host_ctx_t;
struct native_host_ctx {
  uint8_t mem[NATIVE_HOST_MEM_SIZE];

  /* Host-side copy of the guest registers. update_registers mirrors it into
   * the hydra_machine; direct edits through this field are test-visible. */
  hydra_machine_registers_t regs;
};

/* Direct (non-dlsym) entry point into the native host. Besides its value as
 * a version tag, referencing it is required for the host library to stay in
 * the test executable's DT_NEEDED list: --as-needed drops shared libraries
 * only reachable through the dlsym discovery seam. */
const char *native_host_id(void);
