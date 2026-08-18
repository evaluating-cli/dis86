#ifndef DOSDEBUG_H
#define DOSDEBUG_H

#include <stdint.h>
#include <stdbool.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct dosdebug dosdebug_t;

typedef struct {
    uint16_t ax, bx, cx, dx, si, di, bp, sp, ip, cs, ds, es, ss, flags;
} dosdebug_regs_t;

// Connect to a running dosemu2 instance. If pid == 0, auto-discover.
// Returns NULL on failure.
dosdebug_t *dosdebug_connect(pid_t pid);

// Disconnect (closes FIFOs).
void dosdebug_disconnect(dosdebug_t *db);

// Read all registers. Returns 0 on success, -1 on failure.
int dosdebug_read_regs(dosdebug_t *db, dosdebug_regs_t *regs);

// Write one register. reg_name is "AX","BX",...,"IP","CS",...,"FL".
// Returns 0 on success, -1 on failure.
int dosdebug_write_reg(dosdebug_t *db, const char *reg_name, uint16_t val);

// Write all registers from a regs struct (issues multiple write_reg calls).
int dosdebug_write_regs(dosdebug_t *db, const dosdebug_regs_t *regs);

// Set a breakpoint at seg:off. Returns breakpoint index >= 0, or -1 on failure.
int dosdebug_set_bp(dosdebug_t *db, uint16_t seg, uint16_t off);

// Clear breakpoint by index.
int dosdebug_clear_bp(dosdebug_t *db, int bp_index);

// Continue execution (g). Returns immediately.
int dosdebug_go(dosdebug_t *db);

// Stop execution. Returns immediately.
int dosdebug_stop(dosdebug_t *db);

// Wait for the machine to stop (breakpoint/exception). Reads the stop
// notification and parses registers. Returns 0 on success, -1 on timeout.
// Timeout in milliseconds; 0 = default (5 seconds).
int dosdebug_wait_stop(dosdebug_t *db, dosdebug_regs_t *regs, int timeout_ms);

// Read memory (d command). Reads up to 256 bytes at seg:off into buf.
// Returns number of bytes read, or -1 on failure.
int dosdebug_read_mem(dosdebug_t *db, uint16_t seg, uint16_t off, uint8_t *buf, int len);

// Write memory (m command). Writes len bytes from buf to seg:off.
// Returns 0 on success, -1 on failure.
int dosdebug_write_mem(dosdebug_t *db, uint16_t seg, uint16_t off, const uint8_t *buf, int len);

// Check if the debugger is connected and the dosemu2 process is alive.
bool dosdebug_is_alive(dosdebug_t *db);

#ifdef __cplusplus
}
#endif

#endif // DOSDEBUG_H