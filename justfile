#!/usr/bin/env just --justfile

# List all available recipies
list:
  just --list
  
# Build the repository
build: build-dis86 build-hydra

# Build the dis86 component only
build-dis86:
  #!/bin/bash
  cd {{justfile_directory()}}
  (cd dis86 && cargo build)
  mkdir -p build/bin
  cp dis86/target/debug/dis86 build/bin/
  cp dis86/target/debug/mzfile build/bin/

# Run the host-independent checks used by CI. Syntax-check both the Hydra core
# and the external dosemu2 host/tests so a green workflow covers the code added
# by the hosting PR without requiring a dosemu2 runtime. The native host unit
# test is the first linked-and-run Hydra test in this lane: it links the core
# with -rdynamic so the dlsym(RTLD_DEFAULT) seam resolves against the binary.
check:
  #!/bin/bash
  set -euo pipefail
  cd {{justfile_directory()}}
  PYTHONPATH=confgen python3 confgen/test_overlay_metadata.py
  PYTHONPATH=confgen python3 confgen/test_annotation_types.py
  cargo test --manifest-path dis86/Cargo.toml --locked --all-targets
  cc -std=c11 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -Ihydra/src -Ihydra/include -fsyntax-only hydra/src/*.c
  cc -std=c11 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -Ihydra/src -Ihydra/include -Ihydra/src/dosemu_host \
    -fsyntax-only hydra/src/dosemu_host/*.c
  cc -std=c11 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -Ihydra/src -Ihydra/include -Ihydra/src/native_host \
    -fsyntax-only hydra/src/native_host/*.c
  mkdir -p build
  cc -std=c11 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -Ihydra/src -Ihydra/include -Ihydra/src/native_host \
    hydra/src/*.c hydra/src/native_host/*.c \
    -rdynamic -pthread -ldl -o build/test_native_host
  ./build/test_native_host

# Build the hydra component only
build-hydra:
  #!/bin/bash
  cd {{justfile_directory()}}
  (cd hydra && just build)

# Test the repository
test *opts:
  (cd dis86 && cargo test {{opts}})
  (cd hydra && just test)
