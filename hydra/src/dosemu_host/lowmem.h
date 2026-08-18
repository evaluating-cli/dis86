#ifndef LOWMEM_H
#define LOWMEM_H

#include <stdint.h>
#include <stdbool.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct lowmem lowmem_t;

// Connect to dosemu2's lowmem backing by scanning /proc/<pid>/fd/.
// pid = the dosemu2 process PID.
// Returns NULL on failure.
lowmem_t *lowmem_connect(pid_t pid);

// Disconnect (unmaps and closes fd).
void lowmem_disconnect(lowmem_t *lm);

// Get a raw pointer to guest memory at the given address.
// Returns NULL if addr is out of range.
// This is the mem_hostaddr implementation for Hydra's vtable.
uint8_t *lowmem_hostaddr(lowmem_t *lm, uint32_t addr);

// Read/write memory at a guest address.
// These are convenience wrappers around lowmem_hostaddr.
uint8_t lowmem_read8(lowmem_t *lm, uint32_t addr);
uint16_t lowmem_read16(lowmem_t *lm, uint32_t addr);
void lowmem_write8(lowmem_t *lm, uint32_t addr, uint8_t val);
void lowmem_write16(lowmem_t *lm, uint32_t addr, uint16_t val);

// Get the base pointer and size (for direct pointer arithmetic).
uint8_t *lowmem_base(lowmem_t *lm);
size_t lowmem_size(lowmem_t *lm);

// Check if the mapping is still valid (dosemu2 still alive).
bool lowmem_is_valid(lowmem_t *lm);

#ifdef __cplusplus
}
#endif

#endif // LOWMEM_H