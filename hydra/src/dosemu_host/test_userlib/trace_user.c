/*
 * test_userlib/trace_user.c - the user library the instrumented-execution
 * tool (issue #97) traces against.
 *
 * It is the usual Hydra user-metadata library (the function metadata and
 * the callstack confs of the testprog fixture) plus the native
 * implementations of the three hooked functions, and the entry point the
 * dosemu2 host calls to register them:
 *
 *   hydra_user_register_hooks()
 *
 * The native bodies mirror what the guest code does (mov ax,imm; ret), so
 * the traced run is the same program the integration test drives, seen
 * through the trace sink instead of through the test's assertions.
 */

#include "header.h"
#include "hydra_machine.h"
#include "addr.h"
#include "functions.h"
#include "callstack.h"
#include "hooks.h"
#include "callstubs.h"
#include "machine.h"

/* testprog.asm layout (org 100h); the same offsets the metadata uses. */
#define OFF_myfunc   0x013e   /* mov ax,0x1111; ret */
#define OFF_func2    0x0142   /* mov ax,0x2222; ret */
#define OFF_callthru 0x0146   /* mov ax,0x3333; ret */
#define OFF_helper2  0x014a   /* unhooked callthrough target */
#define OFF_jmploop  0x013c   /* jmp mainloop (mainloop tail) */

static hydra_function_def_t f_defs[] = {
  { "F_myfunc",   { { 0, 0x0000, OFF_myfunc   } } },
  { "F_func2",    { { 0, 0x0000, OFF_func2    } } },
  { "F_callthru", { { 0, 0x0000, OFF_callthru } } },
};

const hydra_function_metadata_t * hydra_user_functions(void)
{
  static hydra_function_metadata_t md;
  md.n_defs = ARRAY_SIZE(f_defs);
  md.defs = f_defs;
  return &md;
}

static hydra_callstack_conf_t cs_confs[] = {
  { HYDRA_CALLSTACK_CONF_TYPE_HANDLER, "HDLR_helper2", { { 0, 0x0000, OFF_helper2 } } },
  { HYDRA_CALLSTACK_CONF_TYPE_JUMPRET, "jmp_mainloop", { { 0, 0x0000, OFF_jmploop } } },
};

const hydra_callstack_metadata_t * hydra_user_callstack(void)
{
  static hydra_callstack_metadata_t md;
  md.n_confs = ARRAY_SIZE(cs_confs);
  md.confs = cs_confs;
  return &md;
}

/* The native bodies. The hook addresses are image-relative (segment 0),
 * the same space the metadata uses. */

/* The unhooked helper, called the way the decompiler calls a function it
 * has not reimplemented: through a call stub, so the trace records the
 * call boundary with its arguments and the value that came back. */
HYDRA_DEFINE_CALLSTUB(F_trace_helper2, u16, 0, ADDR_MAKE_EXT(0, 0x0000, OFF_helper2), 0)

HYDRA_FUNC(H_trace_myfunc)
{
  AX = 0x1111;
  RETURN_NEAR();
}

HYDRA_FUNC(H_trace_func2)
{
  AX = 0x2222;
  RETURN_NEAR();
}

HYDRA_FUNC(H_trace_callthru)
{
  /* A native->guest call into the unhooked helper: this is the seam the
   * trace records the arguments and the returned value at. */
  u32 r = F_trace_helper2(m);
  AX = (u16)r;
  RETURN_NEAR();
}

void hydra_user_register_hooks(void)
{
  hydra_impl_register("F_myfunc", H_trace_myfunc, 0);
  hydra_impl_register("F_func2", H_trace_func2, 0);
  hydra_impl_register("F_callthru", H_trace_callthru, 0);
}
