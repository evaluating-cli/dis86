## Native host: hydra_machine_hardware_t implementation on a linear
## guest-memory array (native_host.h). Standalone substrate for executable
## ports; shares the core's dlsym discovery seam (api_impl.c) with
## dosemu_host, requiring no dosemu2 process to build or test.

libhydra_native = shared_library(
  'hydra_native',
  files('native_host.c'),
  link_with : libhydra,
)

test_native_host = executable(
  'test_native_host', 'test_native_host.c',
  link_with : [libhydra, libhydra_native],
)
