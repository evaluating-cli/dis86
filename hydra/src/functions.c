#include "internal.h"
#include <dlfcn.h>

// Must be configured at init-time
static const hydra_function_metadata_t *md = NULL;

static int hydra_function_metadata_validate(const hydra_function_metadata_t *_md,
                                            const char *where)
{
  if (!_md)
    return -1;
  if (_md->n_defs && !_md->defs)
    return -1;

  for (size_t i = 0; i < _md->n_defs; i++) {
    const hydra_function_def_t *f = &_md->defs[i];
    if (!addr_is_overlay(f->addr))
      continue;

    u16 overlay_num = addr_overlay_num(f->addr);
    if (overlay_num >= HYDRA_OVERLAY_SEGMENT_COUNT) {
      fprintf(stderr,
              "%s: function %s uses overlay %u, but only 0..%u are supported\n",
              where, f->name ? f->name : "(unnamed)", overlay_num,
              (unsigned)(HYDRA_OVERLAY_SEGMENT_COUNT - 1));
      return -1;
    }
  }
  return 0;
}

void hydra_function_metadata_init(void)
{
  const hydra_function_metadata_t *(*user_fn)(void) = NULL;
  *(void**)&user_fn = dlsym(RTLD_DEFAULT, "hydra_user_functions");
  if (!user_fn) FAIL("Failed to find user metadata: hydra_user_functions()");

  const hydra_function_metadata_t *user_md = user_fn();
  if (hydra_function_metadata_validate(user_md,
                                       "hydra_function_metadata_init") != 0)
    FAIL("Invalid user function metadata");
  md = user_md;
}

// Inject a metadata provider at runtime (takes precedence over the
// dlsym-cached one). Used by hosts that load user metadata dynamically
// (e.g. the dosemu host loading a user library via dlopen).
int hydra_function_metadata_set(const hydra_function_metadata_t *_md)
{
  if (!_md) FAIL("hydra_function_metadata_set(NULL)");
  if (_md->n_defs && !_md->defs) FAIL("hydra_function_metadata_set: n_defs != 0 but defs is NULL");
  if (hydra_function_metadata_validate(_md, "hydra_function_metadata_set") != 0)
    return -1;
  md = _md;
  return 0;
}


const hydra_function_def_t * hydra_function_find(const char *name)
{
  for (size_t i = 0; i < md->n_defs; i++) {
    if (0 == strcmp(name, md->defs[i].name)) {
      return &md->defs[i];
    }
  }
  return NULL;
}

const char *hydra_function_name(addr_t s)
{
 u32 addr = addr_abs(s);
  for (size_t i = 0; i < md->n_defs; i++) {
    const hydra_function_def_t *f = &md->defs[i];
    if (addr_is_overlay(f->addr)) continue; // ignore overlays
    if (addr == addr_abs(f->addr)) {
      return f->name;
    }
  }
  return NULL;
}

bool hydra_function_addr(const char *name, addr_t *_out)
{
  for (size_t i = 0; i < md->n_defs; i++) {
    const hydra_function_def_t *f = &md->defs[i];
    if (0 == strcmp(name, f->name)) {
      if (_out) *_out = f->addr;
      return true;
    }
  }
  return false;
}
