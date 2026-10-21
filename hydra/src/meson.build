
HYDRA_SRC = files([
  'conf.c',
  'api_impl.c',
  'hooks.c',
  'callstack.c',
  'functions.c',
  'datasection.c',
  'exec.c',
  'machine.c',
  'overlay.c',
  'addr.c',
  'dump.c',
  'trace.c',
])

libhydra = shared_library(
  'hydra', HYDRA_SRC,
)

subdir('dosemu_host')
subdir('native_host')
