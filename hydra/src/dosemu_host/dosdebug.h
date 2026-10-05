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

// Return the dosemu2 pid this connection is bound to (0 if not connected).
pid_t dosdebug_get_pid(dosdebug_t *db);

// Disconnect (closes FIFOs).
void dosdebug_disconnect(dosdebug_t *db);

// Read all registers. Returns 0 on success, -1 on failure.
int dosdebug_read_regs(dosdebug_t *db, dosdebug_regs_t *regs);

// Write one register. reg_name is "AX","BX",...,"IP","CS",...,"FL".
// Returns 0 on success, -1 on failure.
int dosdebug_write_reg(dosdebug_t *db, const char *reg_name, uint16_t val);

// Write all registers from a regs struct (issues multiple write_reg calls).
int dosdebug_write_regs(dosdebug_t *db, const dosdebug_regs_t *regs);

// Diff-write: write only the registers that differ from *base, which must
// hold the live CPU state (the last parsed dump). FL is always written
// (dosemu forces IF/IOPL/bit1, so flags can never be diffed away).
// Verification is identical to dosdebug_write_regs: a full r0 read-back is
// compared against ALL registers, so a stale base fails loudly instead of
// corrupting silently.
int dosdebug_write_regs_diff(dosdebug_t *db, const dosdebug_regs_t *regs,
                             const dosdebug_regs_t *base);

// Set a breakpoint at seg:off. Returns breakpoint index >= 0, or -1 on failure.
int dosdebug_set_bp(dosdebug_t *db, uint16_t seg, uint16_t off);

// Clear breakpoint by index.
int dosdebug_clear_bp(dosdebug_t *db, int bp_index);

// Set/clear dosemu's non-patching breakpoint on a software interrupt. Unlike
// an INT3 code breakpoint this does not modify guest RAM or simx86 code bytes.
int dosdebug_set_bpint(dosdebug_t *db, uint8_t intno);
int dosdebug_clear_bpint(dosdebug_t *db, uint8_t intno);

// Continue execution (g). Returns immediately.
int dosdebug_go(dosdebug_t *db);

// Single-step (t). Executes one instruction (stepping over INTs).
// Drains pending output first, so the step's dump stays in sync with the
// command stream. Returns immediately; use dosdebug_wait_stop to read the result.
int dosdebug_step(dosdebug_t *db);

// Single-step into an INT (ti). For an INT instruction dosemu performs the
// real interrupt entry and stops at the handler, preserving the guest's
// stacked CS:IP/FLAGS frame. Returns immediately; use dosdebug_wait_stop.
int dosdebug_step_into(dosdebug_t *db);

// Discard everything pending on the dbgout stream (user buffer + kernel fd),
// without blocking. Used to keep the response stream synchronized.
void dosdebug_drain(dosdebug_t *db);

// Stop execution. Returns immediately.
int dosdebug_stop(dosdebug_t *db);

// Arm dosemu2's 'bpload' load breakpoint: hijacks the next INT21 EXEC
// (AH=4B00) into a load-don't-execute and stops the machine AT the loaded
// program's relocated entry (DS=ES=PSP, GP regs zeroed, TF set).
// Must be issued while stopped, before the EXEC runs. Returns 0 on success.
int dosdebug_bpload(dosdebug_t *db);

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
