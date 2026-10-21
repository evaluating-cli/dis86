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

/* The backend's teardown, run as an atexit handler so the terminal state is
 * restored on the way out no matter how the process ends.
 *
 * A guest that terminates no longer ends the process: the termination is
 * reported to the host driver, which finishes its own reporting and then
 * exits, so the driver's epilogue runs in the right order. This handler is
 * the safety net for the paths that do not get there — an abort, or a driver
 * that simply returns from main. The terminal backend puts stdin into
 * non-blocking mode and would otherwise leave the user's shell that way.
 * Aborts do not run atexit handlers, but those are crash paths anyway. */
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

  /* Shut the outgoing backend down, the way select() does. Without this,
   * installing a probe over the terminal backend leaves stdin in
   * non-blocking mode; the next select() then builds a second terminal
   * backend whose init saves those already-broken flags as the user's
   * original ones, so the atexit handler "restores" a non-blocking shell
   * instead of the one it found. */
  if (g_backend && g_backend->shutdown)
    g_backend->shutdown(g_backend_state);

  backend_register_atexit();
  g_backend = be;
  g_backend_state = state;
}

void hydra_native_backend_clear_probe(void)
{
  if (g_backend && g_backend->shutdown)
    g_backend->shutdown(g_backend_state);
  g_backend = NULL;
  g_backend_state = NULL;
}

/* The backend is chosen exactly once, in hydra_user_init(), before any guest
 * code can run. Nothing here mutates state: a dispatch from a hook thread can
 * no longer race with a first-use initialization. */
static const hydra_backend_t *active_backend(void)
{
  if (!g_backend)
    FAIL("native host: device dispatch before hydra_user_init() selected a "
         "backend (interrupts and port I/O both need one)");
  return g_backend;
}

const hydra_backend_t *hydra_native_backend_active(void **state_out)
{
  const hydra_backend_t *be = active_backend();
  if (state_out)
    *state_out = g_backend_state;
  return be;
}

const hydra_backend_t *hydra_native_backend_try_active(void **state_out)
{
  if (state_out)
    *state_out = g_backend_state;
  return g_backend;
}

void hydra_native_backend_present(void)
{
  /* No backend yet means nothing has been drawn. Presenting is a rendering
   * nicety, so this stays a no-op rather than the fail-loud contract the
   * interrupt path uses. */
  if (!g_backend || !g_backend->present)
    return;
  g_backend->present(g_backend_state);
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
    /* Guest termination. INT 21h AH=4Ch does not return, so the decompiled
     * hook must not continue either: the statements the decompiler emitted
     * after the INT are code the guest never meant to reach, and running them
     * would corrupt the world the guest left behind.
     *
     * The host's own NATIVE_INT_EXIT is a private code; the core's hook
     * convention is HYDRA_USER_INT_TERMINATE (see hydra_machine.h), which
     * the core turns into a TERMINATE result: the hook returns at the
     * interrupt and the exec loop reports the termination to the host
     * driver. The driver then finishes its reporting and exits with the
     * guest's code — which is what a playable port needs, and what the
     * process-wide exit() this replaces could not offer.
     *
     * The exit code is AL (DOS semantics); the core reads it from the
     * registers. */
    return HYDRA_USER_INT_TERMINATE;
  }
  return rc;
}
