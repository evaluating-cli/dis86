## Phase 3: dosemu2 host for the Hydra vtable.
## host.c implements hydra_user_init() + the vtable callbacks; the shared
## library is loaded into the test executable so dlsym(RTLD_DEFAULT) finds
## hydra_user_init / hydra_user_functions / hydra_user_callstack.

## Keep the hardened FIFO client in dosdebug.c unchanged and append the tiny
## Option-E command wrappers in the same translation unit so they can reuse
## its private protocol helpers without widening that internal API.
dosdebug_combined = custom_target(
  'dosdebug_combined.c',
  input : ['dosdebug.c', 'dosdebug_commands.inc'],
  output : 'dosdebug_combined.c',
  command : ['cat', '@INPUT@'],
  capture : true,
)

dosemu_host_src = files('host.c', 'host_driver.c', 'lowmem.c') + [dosdebug_combined]

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

## Host-independent capacity regression: a dynamic overlay site counts against
## the same 64-entry dosdebug budget as static hooks.
test_overlay_budget = executable(
  'test_overlay_budget', 'test_overlay_budget.c',
  link_with : [libhydra, libhydra_dosemu],
)

## Host-independent coverage for the UNIMPL/UNIMPL_FLAGS markers
## (hydra/src/header.h): the decompiler emits these calls for unmodeled
## operations (UNIMPL) and imprecise flags values (UNIMPL_FLAGS). Both must
## accept operands in expression position; UNIMPL must also trap loudly in
## statement position, and UNIMPL_FLAGS must warn and return 0 in condition
## position. Header-inline only; no library needed.
test_unimpl_trap = executable(
  'test_unimpl_trap', 'test_unimpl_trap.c',
)

## The trace sink (issue #97): a core facility, so its test needs no
## dosemu2 either - a fake machine with flat RAM is enough.
test_trace = executable(
  'test_trace', 'test_trace.c',
  link_with : libhydra,
)

## Phase 4/5: guest program (nasm -> .COM) + driver integration test.

testprog_com = custom_target(
  'testprog.com',
  input : 'testprog.asm',
  output : 'testprog.com',
  command : ['nasm', '-f', 'bin', '-o', '@OUTPUT@', '@INPUT@'],
)

## Phase 7 Item B: synthetic user-metadata library, loaded by the dosemu
## host via the conf key lib=<path> (dlopen by handle; headers only, no
## link dependency on libhydra).
test_userlib = shared_library(
  'test_hydra_user', 'test_userlib' / 'hydra_user.c',
)

## The instrumented-execution tool (issue #97) traces a guest with the
## trace sink open. It drives the same host_run() loop the integration
## tests drive, so it needs the dosemu host and nothing else.
hydra_trace = executable(
  'hydra_trace', 'trace_tool.c',
  link_with : [libhydra, libhydra_dosemu],
)

## The user library the trace tool runs against: the testprog fixture's
## metadata plus the native bodies of its hooked functions, registered
## through hydra_user_register_hooks() (the dosemu host calls it once the
## metadata is in place).
test_trace_user = shared_library(
  'test_trace_user', 'test_userlib' / 'trace_user.c',
  link_with : libhydra,
)

test_driver = executable(
  'test_driver', 'test_driver.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_com],
  c_args : [
    '-DTESTPROG_COM_PATH="' + testprog_com.full_path() + '"',
    '-DTEST_USERLIB_PATH="' + test_userlib.full_path() + '"',
  ],
)

## Phase 7 Item C: MZ variant of the guest program (hand-rolled header in
## nasm, no linker), the real-mode EXEC launcher (keeps bpload's trampoline
## out of JIT-hot RVC stub code — see launch.asm) and the bpload-driven
## integration test.

testprog_exe = custom_target(
  'testprog.exe',
  input : 'testprog_exe.asm',
  output : 'testprog.exe',
  command : ['nasm', '-f', 'bin', '-w-zeroing', '-o', '@OUTPUT@',
             '@INPUT@'],
)

launch_com = custom_target(
  'launch.com',
  input : 'launch.asm',
  output : 'launch.com',
  command : ['nasm', '-f', 'bin', '-o', '@OUTPUT@', '@INPUT@'],
  build_by_default : true,
)

test_driver_exe = executable(
  'test_driver_exe', 'test_driver_exe.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_exe],
  c_args : ['-DTESTPROG_EXE_PATH="' + testprog_exe.full_path() + '"'],
)

## Focused MZ/FBOV end-to-end smoke: metadata is generated from the same
## annotation fixture covered by confgen's generator regression, then loaded
## by host.c through the normal lib=<path> path during live dispatch.
confgen_dir = meson.project_source_root() / '../confgen'
python3 = find_program('python3')
mz_ovl_annotations = confgen_dir / 'mz_overlay_fixture.py'
mz_ovl_metadata_hdr = custom_target(
  'mz_overlay_metadata_header',
  input : mz_ovl_annotations,
  output : 'hydra_user_appdata.h',
  command : [python3, confgen_dir / 'generate.py', '@INPUT@',
             '--appdata-hdr', '--output-path', '@OUTPUT@'],
)
mz_ovl_metadata_src = custom_target(
  'mz_overlay_metadata_source',
  input : mz_ovl_annotations,
  output : 'mz_overlay_user.c',
  command : [python3, confgen_dir / 'generate.py', '@INPUT@',
             '--appdata-src', '--output-path', '@OUTPUT@'],
)
test_mz_ovl_user = shared_library(
  'test_mz_overlay_user', [mz_ovl_metadata_src, mz_ovl_metadata_hdr],
  link_with : libhydra,
  include_directories : include_directories('../../include'),
)

testprog_mz_ovl = custom_target(
  'testprog_mz_ovl.exe',
  input : 'testprog_mz_ovl.asm',
  output : 'testprog_mz_ovl.exe',
  command : ['nasm', '-f', 'bin', '-w-zeroing', '-o', '@OUTPUT@', '@INPUT@'],
)
test_driver_mz_ovl = executable(
  'test_driver_mz_ovl', 'test_driver_mz_ovl.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_mz_ovl],
  c_args : [
    '-DTESTPROG_MZ_OVL_PATH="' + testprog_mz_ovl.full_path() + '"',
    '-DTEST_MZ_OVL_USERLIB_PATH="' + test_mz_ovl_user.full_path() + '"',
  ],
)

## Phase 7 Item D: HYDSNAP capture/restore across two dosemu instances.
test_driver_cap = executable(
  'test_driver_cap', 'test_driver_cap.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_exe],
  c_args : ['-DTESTPROG_EXE_PATH="' + testprog_exe.full_path() + '"'],
)

## Phase 7 Item E: overlay support - VROOMM-style page-in stub guest and
## the full page-in/decompiled-takeover integration test.
testprog_ovl_com = custom_target(
  'testprog_ovl.com',
  input : 'testprog_ovl.asm',
  output : 'testprog_ovl.com',
  command : ['nasm', '-f', 'bin', '-o', '@OUTPUT@', '@INPUT@'],
)

test_driver_ovl = executable(
  'test_driver_ovl', 'test_driver_ovl.c',
  link_with : [libhydra, libhydra_dosemu],
  sources : [testprog_ovl_com],
  c_args : ['-DTESTPROG_OVL_PATH="' + testprog_ovl_com.full_path() + '"'],
)
