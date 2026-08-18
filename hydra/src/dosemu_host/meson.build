## Phase 3: dosemu2 host for the Hydra vtable.
## host.c implements hydra_user_init() + the vtable callbacks; the shared
## library is loaded into the test executable so dlsym(RTLD_DEFAULT) finds
## hydra_user_init / hydra_user_functions / hydra_user_callstack.

dosemu_host_src = files([
  'host.c',
  'dosdebug.c',
  'lowmem.c',
])

libhydra_dosemu = shared_library(
  'hydra_dosemu',
  dosemu_host_src,
  link_with : libhydra,
)

## Phase 1-2 unit tests (dosdebug client / lowmem bridge) and the Phase 3
## integration test.

test_dosdebug = executable(
  'test_dosdebug', 'test_dosdebug.c',
  link_with : libhydra_dosemu,
)

test_lowmem = executable(
  'test_lowmem', 'test_lowmem.c',
  link_with : libhydra_dosemu,
)

test_host = executable(
  'test_host', 'test_host.c',
  link_with : [libhydra, libhydra_dosemu],
)