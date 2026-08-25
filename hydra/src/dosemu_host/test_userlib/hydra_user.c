/*
 * test_userlib/hydra_user.c - synthetic Hydra user-metadata library for the
 * Phase 7 Item B integration test.
 *
 * Built as its own shared_library target and handed to the dosemu host via
 * the conf key lib=<path> (resolved BY HANDLE, see host.c). Exports the
 * provider functions hydra_user_functions()/hydra_user_callstack() in the
 * confgen appdata convention; all addresses are CODE_START_SEG-relative,
 * i.e. org-100h offsets into testprog.asm (kept in sync by an explicit
 * cross-check in test_driver.c).
 *
 * NOTE: this library must NOT export hydra_user_init - the dosemu host
 * owns that symbol on this platform (see host.c).
 */

#include "header.h"
#include "addr.h"
#include "functions.h"
#include "callstack.h"

/* testprog.asm layout (org 100h) */
/* Offsets must match the hardened testprog.asm layout (8 KiB guest-owned
 * raw-code reservation region embedded mid-image shifts everything +0x0D).
 * The cross-check in test_driver.c fails loudly if these drift again. */
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
  static hydra_function_metadata_t md[1];
  md->n_defs = ARRAY_SIZE(f_defs);
  md->defs = f_defs;
  return md;
}

static hydra_callstack_conf_t cs_confs[] = {
  /* handler-entry marker at helper2: unreachable as a dispatch stop in the
   * live run (helper2 is only ever consumed inside traces). */
  { HYDRA_CALLSTACK_CONF_TYPE_HANDLER, "HDLR_helper2", { { 0, 0x0000, OFF_helper2 } } },
  /* special jump-ret location at the mainloop tail jmp: likewise never a
   * dispatch stop; exercised directly by test_driver's callstack probe. */
  { HYDRA_CALLSTACK_CONF_TYPE_JUMPRET, "jmp_mainloop", { { 0, 0x0000, OFF_jmploop } } },
};

const hydra_callstack_metadata_t * hydra_user_callstack(void)
{
  static hydra_callstack_metadata_t md[1];
  md->n_confs = ARRAY_SIZE(cs_confs);
  md->confs = cs_confs;
  return md;
}
