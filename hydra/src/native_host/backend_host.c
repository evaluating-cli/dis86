/* Host-side glue between the core's INT primitive and the device HAL
 * (issue #95).
 *
 * This file defines the optional `hydra_user_int` hook that api_impl.c
 * resolves with dlsym. That keeps the core free of any compile-time
 * dependency on the native host: hosts that execute raw code (dosemu2)
 * never reach this code, and a host that ships no device layer simply
 * leaves the symbol absent, in which case hydra_impl_int fails loud.
 *
 * It also owns the active-backend registry, so the core never learns what a
 * backend is.
 */

#include <stdlib.h>

/* Deliberately not internal.h: this translation unit defines the function
 * named hydra_user_int, so it must not see any declaration of a pointer by
 * that name (see the note in internal.h). */

#include "header.h"
#include "hydra_machine.h"

#include "hydra_backend.h"
#include "native_host.h"

static const hydra_backend_t *g_backend = NULL;
static void *g_backend_state = NULL;

/* Guest termination ends the process from inside a hook (see hydra_impl_int),
 * so normal shutdown never reaches the host's own teardown. Restore terminal
 * state here instead: the terminal backend puts stdin into non-blocking mode
 * and would otherwise leave the user's shell that way. Aborts do not run
 * atexit handlers, but those are crash paths anyway. */
static void backend_atexit(void)
{
  if (!g_backend)
    return;
  if (g_backend->shutdown)
    g_backend->shutdown(g_backend_state);
  g_backend = NULL;
  g_backend_state = NULL;
}

/* Registered once. atexit() reports failure by return value, and this codebase
 * fails loud rather than silently dropping a cleanup it depends on. */
static void backend_register_atexit(void)
{
  static int atexit_registered = 0;
  if (atexit_registered)
    return;
  if (atexit(backend_atexit) != 0)
    FAIL("native host: could not register the backend atexit handler");
  atexit_registered = 1;
}

void hydra_native_backend_select(const char *name)
{
  if (g_backend) {
    if (g_backend->shutdown)
      g_backend->shutdown(g_backend_state);
    g_backend = NULL;
    g_backend_state = NULL;
  }

  const hydra_backend_t *be = hydra_backend_by_name(name);
  if (!be)
    FAIL("native host: no backend named '%s'", name ? name : "(null)");
  if (be->init(&g_backend_state, "") != 0)
    FAIL("native host: backend '%s' failed to initialize", be->name);

  backend_register_atexit();

  g_backend = be;
}

/* Test seam. backend_selftest.c links into the same test binary and uses this
 * to install a probe backend directly, so it can observe that the atexit
 * teardown really runs on the guest-termination path — the ordinary registry
 * has no way to name a backend that only exists in the test. No host calls
 * this, and it is the only way to select a backend outside the registry. */
void hydra_native_backend_install_probe(const hydra_backend_t *be, void *state)
{
  if (!be)
    FAIL("native host: install_probe called with no backend");

  backend_register_atexit();
  g_backend = be;
  g_backend_state = state;
}

/* The backend is chosen exactly once, in hydra_user_init(), before any guest
 * code can run. Nothing here mutates state: a dispatch from a hook thread can
 * no longer race with a first-use initialization. */
static const hydra_backend_t *active_backend(void)
{
  if (!g_backend)
    FAIL("native host: interrupt dispatched before hydra_user_init() selected "
         "a backend");
  return g_backend;
}

const hydra_backend_t *hydra_native_backend_active(void **state_out)
{
  const hydra_backend_t *be = active_backend();
  if (state_out)
    *state_out = g_backend_state;
  return be;
}

int hydra_user_int(hydra_machine_t *m, u8 num)
{
  /* Guest memory comes from the native host's linear array, but the
   * registers are read live from the core: the native host's own mirror is
   * only synced by update_registers, which may lag the executing code. */
  native_host_ctx_t *ctx = (native_host_ctx_t *)m->hardware->ctx;
  const hydra_backend_t *be = active_backend();

  int rc = native_int_dispatch(ctx, m->registers, num, be, g_backend_state);
  if (rc == NATIVE_INT_EXIT) {
    /* Guest termination. INT 21h AH=4Ch does not return, so ending the process
     * here is the only way to keep the decompiled hook from running whatever
     * follows the INT — code the guest never meant to reach. Same shape as
     * the capture mode's exit() in hydra_exec_run_special_modes(); the
     * atexit handler above restores terminal state on the way out.
     *
     * This runs on the hook's worker thread, so the host's main() never
     * regains control: anything a host must do after a guest terminates has
     * to be an atexit handler registered after hydra_machine_init (LIFO puts
     * it before the backend's). See the "guest termination" note in
     * hydra/README.md — making termination observable to the host instead is
     * the target design for issue #95 Tier 2.
     *
     * TODO: present() the final frame before exiting. be->present has no
     * caller yet, so nothing is lost today, but once the render loop exists
     * the guest's last frame would be dropped here. */
    exit((int)(m->registers->ax & 0xff));
  }
  return rc;
}
