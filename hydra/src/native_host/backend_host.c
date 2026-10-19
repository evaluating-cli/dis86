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

/* Deliberately not internal.h: this translation unit defines the function
 * named hydra_user_int, so it must not see any declaration of a pointer by
 * that name (see the note in internal.h). */
#include "header.h"
#include "hydra_machine.h"

#include "hydra_backend.h"
#include "native_host.h"

static const hydra_backend_t *g_backend = NULL;
static void *g_backend_state = NULL;

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

  g_backend = be;
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
  if (rc == NATIVE_INT_EXIT)
    return 1;   /* termination requested; see the return convention in internal.h */
  return rc;
}
