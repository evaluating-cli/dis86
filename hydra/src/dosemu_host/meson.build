## Phase 3: dosemu2 host for the Hydra vtable.
## host.c implements hydra_user_init() + the vtable callbacks; the shared
## library is loaded into the test executable so dlsym(RTLD_DEFAULT) finds
## hydra_user_init / hydra_user_functions / hydra_user_callstack.

dosemu_host_src = files([
  'host.c',
  'host_driver.c',
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

## Negative policy fixture: no dosemu process is required. It proves that the
## external backend rejects overlay hooks before releasing guest execution.
test_overlay_reject = executable(
  'test_overlay_reject', 'test_overlay_reject.c',
  link_with : [libhydra, libhydra_dosemu],
)

## Phase 4/5: guest program (nasm -> .COM) + driver integration test.

testprog_com = custom_target(
  'testprog.com',
  input : 'testprog.asm',
  output : 'testprog.com',
  command : ['nasm', '-f', 'bin', '-o', '@OUTPUT@', '@INPUT@'],
)

test_driver = executable(
  'test_driver', 'test_driver.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_com],
  c_args : ['-DTESTPROG_COM_PATH="' + testprog_com.full_path() + '"'],
)