#!/bin/bash
# Arena re-verification run (round 2): closed issues #64-#67 + new issues #77/#78.
#
# Usage: run.sh <dis86-binary> <main-checkout-dir>
# Evidence lands in .arena-verify2/out/.
set -uo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"
OUT="$REPO/.arena-verify2/out"
BIN="${1:?usage: run.sh <dis86-binary> <main-checkout>}"
MAIN="${2:?usage: run.sh <dis86-binary> <main-checkout>}"
mkdir -p "$OUT"

run() {  # run <name> <cmd...>  — records command, output, exit status
  local name="$1"; shift
  {
    printf '$ %s\n' "$*"
    "$@"
    printf 'exit=%s\n' "$?"
  } > "$OUT/$name.txt" 2>&1
}

show() {  # show <name> <file>
  {
    printf '\n===== %s =====\n' "$2"
    cat "$2"
  } >> "$OUT/$1.txt" 2>&1
}

summary() {
  {
    echo "main sha: $(git -C "$MAIN" rev-parse HEAD)"
    echo "dis86:    $(sha256sum "$BIN" | cut -c1-16)  ($BIN)"
    rustc --version; cargo --version
  } > "$OUT/00-env.txt"
}

# --------------------------------------------------------------- #64 --------
# Minimal IR fixture, inserted into a copy of main's opt.rs by the workflow.
run 64-fixture cargo test --manifest-path "$MAIN/dis86/Cargo.toml" --lib \
    --locked issue64_verify -- --nocapture

# --------------------------------------------------------------- #65 --------
run 65-analyze "$BIN" --config /tmp/repro/r65.bsl --binary-exe /tmp/repro/synth_65.exe --analyze
run 65-codeseg "$BIN" --config /tmp/repro/r65.bsl --binary-exe /tmp/repro/synth_65.exe \
    --codeseg-name CODE --emit-code /tmp/repro/65.c
run 65-fixed "$BIN" --config /tmp/repro/r65_fixed.bsl --binary-exe /tmp/repro/synth_65.exe \
    --codeseg-name CODE --emit-code /tmp/repro/65f.c
show 65-fixed /tmp/repro/65f.c

# --------------------------------------------------------------- #66 --------
run 66-emit "$BIN" --config /tmp/repro/r66.bsl --binary-exe /tmp/repro/synth_a.exe \
    --name main --emit-code /tmp/repro/66.c
show 66-emit /tmp/repro/66.c
run 66-ir "$BIN" --config /tmp/repro/r66.bsl --binary-exe /tmp/repro/synth_a.exe \
    --name main --emit-ir-final /tmp/repro/66.ir
show 66-ir /tmp/repro/66.ir
run 66-flags "$BIN" --config /tmp/repro/t66b.bsl --binary-raw /tmp/repro/t66b.bin \
    --name test --emit-code /tmp/repro/66b.c --codegen-hydra
show 66-flags /tmp/repro/66b.c
run 66-flags-ir "$BIN" --config /tmp/repro/t66b.bsl --binary-raw /tmp/repro/t66b.bin \
    --name test --emit-ir-final /tmp/repro/66b.ir
show 66-flags-ir /tmp/repro/66b.ir

# --------------------------------------------------------------- #67 --------
run 67-emit "$BIN" --config /tmp/repro/r67.bsl --binary-exe /tmp/repro/synth_b.exe \
    --name A --emit-code /tmp/repro/67.c
show 67-emit /tmp/repro/67.c
run 67-ctrlflow "$BIN" --config /tmp/repro/r67.bsl --binary-exe /tmp/repro/synth_b.exe \
    --name A --emit-ctrlflow /tmp/repro/67.cf
show 67-ctrlflow /tmp/repro/67.cf

# --------------------------------------------------------------- #77 --------
for case in t77a t77b t77c; do
  run "77-$case" "$BIN" --config "/tmp/repro/$case.bsl" --binary-raw "/tmp/repro/$case.bin" \
      --name test --emit-code "/tmp/repro/$case.c" --codegen-hydra
  show "77-$case" "/tmp/repro/$case.c"
  run "77-$case-ir" "$BIN" --config "/tmp/repro/$case.bsl" --binary-raw "/tmp/repro/$case.bin" \
      --name test --emit-ir-final "/tmp/repro/$case.ir"
  show "77-$case-ir" "/tmp/repro/$case.ir"
done
{
  echo "numeric check for the issue's two cases:"
  python3 -c "
print('  77a emitted (0x4A << 8) | 0x0607 =', hex((0x4A << 8) | 0x0607), ' expected 0x4A07')
print('  77b emitted ((u16)(0x1234 >> 16) << 8) | 0x56 =', hex(((0x1234 >> 16) << 8) | 0x56), ' expected 0x1256')
print('  77c emitted (u16)(0x1234 >> 16) =', hex((0x1234 >> 16) & 0xffff), ' expected 0x12')
"
} > "$OUT/77-numeric.txt" 2>&1

# --------------------------------------------------------------- #78 --------
for case in t78a t78b t78c t78d; do
  run "78-$case" "$BIN" --config "/tmp/repro/$case.bsl" --binary-raw "/tmp/repro/$case.bin" \
      --name test --emit-code "/tmp/repro/$case.c" --codegen-hydra
  show "78-$case" "/tmp/repro/$case.c"
  run "78-$case-ir" "$BIN" --config "/tmp/repro/$case.bsl" --binary-raw "/tmp/repro/$case.bin" \
      --name test --emit-ir-final "/tmp/repro/$case.ir"
  show "78-$case-ir" "/tmp/repro/$case.ir"
done

# ------------------------------------------------------- #69 status ---------
{
  echo "Unimpl sites left in ir_build.rs on main:"
  grep -n "Opcode::Unimpl" "$MAIN/dis86/src/decompile/ir_build.rs"
} > "$OUT/69-unimpl-sites.txt" 2>&1

# ------------------------------------------------------- test suite ---------
run tests cargo test --manifest-path "$MAIN/dis86/Cargo.toml" --locked --all-targets

summary
echo "evidence written to $OUT:"
ls -la "$OUT"
