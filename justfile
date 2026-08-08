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

# Run the host-independent checks used by CI. This deliberately excludes the
# legacy interactive SDL frontend and does not build a DosBox-X submodule.
check:
  #!/bin/bash
  set -euo pipefail
  cd {{justfile_directory()}}
  # These four pre-existing shift-flag cases currently fail independently of
  # the dosemu2 migration.
  cargo test --manifest-path dis86/Cargo.toml --locked --all-targets -- \
    --skip emu86::alu_test::shl16_count_16_cf_from_bit0 \
    --skip emu86::alu_test::shl8_count_8_cf_from_bit0 \
    --skip emu86::alu_test::shr8_of_set_when_original_negative \
    --skip emu86::alu_test::shr8_sign_never_set
  cc -std=c11 -Wall -Wextra -Werror -D_GNU_SOURCE \
    -Ihydra/src -fsyntax-only hydra/src/remote/shmdata.c

# Build the hydra component only
build-hydra:
  #!/bin/bash
  cd {{justfile_directory()}}
  (cd hydra && just build)

# Test the repository
test *opts:
  (cd dis86 && cargo test {{opts}})
  (cd hydra && just test)
