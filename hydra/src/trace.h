#pragma once

#include "addr.h"
#include "hydra_machine.h"
#include "typedefs.h"

/* The Hydra trace sink (issue #97).
 *
 * A structured, opt-in record of what the machine did: hook dispatches, the
 * guest call/return boundaries the callstack model sees, the native->guest
 * call seam (with the arguments the callee will read and the value it
 * returns), the interrupts that were serviced, and - on hosts that advance the
 * guest one instruction at a time - the individual guest instructions.
 *
 * The sink is OFF by default. Every emission helper returns immediately when
 * no sink is open, so an ordinary run neither changes behaviour nor pays for
 * the trace. Opening a sink is an explicit, runtime decision by the host that
 * wants the record.
 *
 * The file format is line oriented text, one event per line:
 *
 *   # hydra trace v1
 *   # code_load=0080
 *   dispatch enter cs=0080 ip=0013 name=F_myfunc
 *   dispatch leave cs=0080 ip=0013 result=resume
 *   call from=0080:0010 to=0080:0185 name=F_parity_leaf
 *   ret from=0080:0189 to=0080:0016
 *   native_call from=0080:0012 to=0080:0185 name=F_parity_leaf nargs=0
 *   native_ret from=0080:0012 to=0080:0185 dx=0000 ax=0001
 *   step cs=0080 ip=0134
 *   int num=21 ah=4c
 *
 * Rules (the writer and the reader in dis86 --trace-analyze both follow
 * them):
 *
 *   - A line starting with '#' is a comment. The first line of a trace is
 *     "# hydra trace v1"; a reader must reject any other version.
 *   - "# code_load=SSSS" states the guest load segment in effect from that
 *     point on. It is re-emitted whenever the load segment changes (the MZ
 *     path moves the code after the image is loaded), so a reader applies the
 *     most recent preceding line, not the first one.
 *   - Every address is an absolute guest seg:off, four hex digits each. A
 *     reader turns it into the image-relative space of a configuration table
 *     by subtracting the code load segment; addresses below it are outside
 *     the image (DOS or ROM).
 *   - "name=" appears only when Hydra's metadata resolves the address to a
 *     known function.
 *   - "nargs=" counts 16-bit argument words and is followed by "arg0=",
 *     "arg1=", ... for the words themselves.
 *   - "dx=" and "ax=" carry the value returned at a call boundary; a
 *     non-zero dx means the call returned 32 bits in DX:AX.
 */

typedef struct hydra_machine hydra_machine_t;

/* Open a trace file. Returns 0 on success, -1 if the file cannot be opened
 * (the message goes to stderr). Fails loudly on a NULL path. */
int hydra_trace_open(const char *path);

/* Open an in-memory sink. The recorded text is available through
 * hydra_trace_mem() until the sink is closed. Used by the tests. */
int hydra_trace_open_mem(void);

/* Close the sink (if any) and flush it. Safe to call when no sink is open. */
void hydra_trace_close(void);

/* Non-zero while a sink is open. */
int hydra_trace_enabled(void);

/* The recorded text of an in-memory sink, or NULL when the open sink is not a
 * memory sink. The buffer stays valid until the sink is closed. */
const char *hydra_trace_mem(size_t *_len);

/* Flush an open sink. */
void hydra_trace_flush(void);

/* The events. Each of these is a no-op when no sink is open. */

/* A hook was dispatched ("enter") or returned ("leave"). The result name is
 * the hydra_result type of the return ("resume", "jump", "call", "call_near",
 * "ret_near", "ret_far", "terminate"), or NULL. */
void hydra_trace_dispatch(const char *name, u16 cs, u16 ip,
                          const char *phase, const char *result);

/* A guest call boundary seen by the callstack model. */
void hydra_trace_call(const char *name, u16 from_seg, u16 from_off,
                      u16 to_seg, u16 to_off);

/* A guest return boundary seen by the callstack model. */
void hydra_trace_ret(const char *name, u16 from_seg, u16 from_off,
                     u16 to_seg, u16 to_off);

/* The native->guest call seam. The arguments are read back from the guest
 * stack (they are the words the callee will read, not the C-level values),
 * so this must be called after the arguments were pushed and before the call
 * was issued. */
void hydra_trace_native_call(hydra_machine_t *m, addr_t callee, int nargs);

/* The value returned at the native->guest call seam, in DX:AX. */
void hydra_trace_native_ret(hydra_machine_t *m, addr_t callee);

/* One guest instruction. Emitted by a host that advances the guest one
 * instruction at a time. */
void hydra_trace_step(u16 cs, u16 ip);

/* An interrupt that was serviced. */
void hydra_trace_int(u8 num, u8 ah);

/* The trace counters, without the dispatch count (the exec layer owns that
 * one). */
void hydra_trace_stats(hydra_run_stats_t *_out);
