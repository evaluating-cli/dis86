#!/bin/bash
# Round-5 probes: the three carry-over findings from rounds 3/4.
#   A. app.rs — is "one failure discards the whole run's output" still reachable?
#   B. #86   — does `df_clear` reset to unknown at block boundaries?
#   C. #78   — `append_regargs` annotation order; return-path `retn AX DX` order.
#
# Usage: run.sh <dis86-binary> <main-checkout-dir>
set -uo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"
OUT="$REPO/.arena-verify5/out"
BIN="${1:?usage: run.sh <dis86-binary> <main-checkout>}"
MAIN="${2:?usage: run.sh <dis86-binary> <main-checkout>}"
mkdir -p "$OUT"
R=/tmp/repro

emit() {  # emit <name> <case> [extra args...]
  local name="$1" case="$2"; shift 2
  rm -f "$R/$case.c"
  {
    printf '$ dis86 --config %s.bsl --binary-raw %s.bin --name test ' "$case" "$case"
    printf -- '--emit-code %s.c --codegen-hydra %s\n' "$case" "$*"
    "$BIN" --config "$R/$case.bsl" --binary-raw "$R/$case.bin" --name test \
        --emit-code "$R/$case.c" --codegen-hydra "$@" 2>&1
    printf 'exit=%s\n' "$?"
    if [ -f "$R/$case.c" ]; then
      printf 'OUTPUT FILE: present (%s bytes)\n' "$(wc -c < "$R/$case.c")"
      printf -- '----- %s.c -----\n' "$case"
      cat "$R/$case.c"
    else
      printf 'OUTPUT FILE: MISSING (nothing written)\n'
    fi
  } > "$OUT/$name.txt" 2>&1
}

ir() {  # ir <name> <case>
  local name="$1" case="$2"
  rm -f "$R/$case.ir"
  {
    printf '$ dis86 ... --name test --emit-ir-final %s.ir\n' "$case"
    "$BIN" --config "$R/$case.bsl" --binary-raw "$R/$case.bin" --name test \
        --emit-ir-final "$R/$case.ir" 2>&1
    printf 'exit=%s\n' "$?"
    [ -f "$R/$case.ir" ] && { printf -- '----- %s.ir -----\n' "$case"; cat "$R/$case.ir"; }
  } > "$OUT/$name.txt" 2>&1
}

codeseg() {  # codeseg <name> <case> — whole-codeseg batch run
  local name="$1" case="$2"
  rm -f "$R/$case.c"
  {
    printf '$ dis86 --config %s.bsl --binary-raw %s.bin ' "$case" "$case"
    printf -- '--codeseg-name CODE --emit-code %s.c --codegen-hydra\n' "$case"
    "$BIN" --config "$R/$case.bsl" --binary-raw "$R/$case.bin" \
        --codeseg-name CODE --emit-code "$R/$case.c" --codegen-hydra 2>&1
    printf 'exit=%s\n' "$?"
    if [ -f "$R/$case.c" ]; then
      printf 'OUTPUT FILE: present (%s bytes)\n' "$(wc -c < "$R/$case.c")"
      printf -- '----- %s.c -----\n' "$case"
      cat "$R/$case.c"
    else
      printf 'OUTPUT FILE: MISSING — every function discarded, including the good one\n'
    fi
  } > "$OUT/$name.txt" 2>&1
}

single() {  # single <name> <case> <fn> — one function by name (control)
  local name="$1" case="$2" fn="$3"
  rm -f "$R/$case.$fn.c"
  {
    printf '$ dis86 --config %s.bsl --binary-raw %s.bin --name %s ' "$case" "$case" "$fn"
    printf -- '--emit-code %s.%s.c --codegen-hydra\n' "$case" "$fn"
    "$BIN" --config "$R/$case.bsl" --binary-raw "$R/$case.bin" --name "$fn" \
        --emit-code "$R/$case.$fn.c" --codegen-hydra 2>&1
    printf 'exit=%s\n' "$?"
    if [ -f "$R/$case.$fn.c" ]; then
      printf 'OUTPUT FILE: present (%s bytes)\n' "$(wc -c < "$R/$case.$fn.c")"
      cat "$R/$case.$fn.c"
    else
      printf 'OUTPUT FILE: MISSING\n'
    fi
  } > "$OUT/$name.txt" 2>&1
}

listcases() { cut -d: -f1 "$1"; }

# ------------------------------------------------------------------- A ------
# The good function alone (control), then the mixed codeseg (one bad sibling).
single "A1a-good-alone"      a1-mixed good
codeseg "A1b-mixed-codeseg"  a1-mixed
single "A1c-inval-alone"     a1-mixed inval
single "A2a-good-alone"      a2-mixed-panic good
codeseg "A2b-mixed-codeseg"  a2-mixed-panic
single "A2c-indirect-alone"  a2-mixed-panic indirect

# ------------------------------------------------------------------- B ------
for case in $(listcases "$R/r5-df-cases.txt"); do
  emit "B-$case" "$case"
  ir "B-$case-ir" "$case"
done

for case in $(listcases "$R/r5-df-cases2.txt"); do
  emit "B2-$case" "$case"
  ir "B2-$case-ir" "$case"
done

# ------------------------------------------------------------------- C ------
emit "C1-call-regargs"      t78d
ir   "C1-call-regargs-ir"   t78d
emit "C2-call-regargs-use"  c2-call-regargs-use
ir   "C2-call-regargs-use-ir" c2-call-regargs-use
emit "C5-regargs-reversed"  c5-regargs-reversed
ir   "C5-regargs-reversed-ir" c5-regargs-reversed
emit "C3-ret-dxax"          c3-ret-dxax
ir   "C3-ret-dxax-ir"       c3-ret-dxax
emit "C4-ret-dxax-ctl"      c4-ret-dxax-ctl
ir   "C4-ret-dxax-ctl-ir"   c4-ret-dxax-ctl
# INT path (already fixed in #88) re-run as the control for C.
emit "C6-int-control"       t78a
ir   "C6-int-control-ir"    t78a

# ------------------------------------------------------- static inventory ----
{
  echo "=== append_regargs and its callers ==="
  grep -n "append_regargs\|order_guest_writes" "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== return_vals / RetNear / Gen::ret (return-path order) ==="
  sed -n '/fn return_vals/,/^  }/p' "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== df_clear assignments ==="
  grep -n "df_clear\s*=" "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== switch_blk ==="
  sed -n '/fn switch_blk/,/^  }/p' "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== app.rs failure handling ==="
  sed -n '/let mut failures = 0;/,/^  0$/p' "$MAIN/dis86/src/app.rs"
} > "$OUT/static-inventory.txt" 2>&1

# ------------------------------------------------------- test suite ----------
{
  printf '$ cargo test --locked --all-targets\n'
  cargo test --manifest-path "$MAIN/dis86/Cargo.toml" --locked --all-targets 2>&1
  printf 'exit=%s\n' "$?"
} > "$OUT/tests.txt" 2>&1

{
  echo "main sha: $(git -C "$MAIN" rev-parse HEAD)"
  echo "main subj: $(git -C "$MAIN" log -1 --format=%s)"
  rustc --version; cargo --version
} > "$OUT/00-env.txt"

echo "evidence written to $OUT:"
ls "$OUT"
