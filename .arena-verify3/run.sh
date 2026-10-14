#!/bin/bash
# Arena verification run (round 3): #69 closure + new issues #85/#86.
#
# Usage: run.sh <dis86-binary> <main-checkout-dir>
# Evidence lands in .arena-verify3/out/.
set -uo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"
OUT="$REPO/.arena-verify3/out"
BIN="${1:?usage: run.sh <dis86-binary> <main-checkout>}"
MAIN="${2:?usage: run.sh <dis86-binary> <main-checkout>}"
mkdir -p "$OUT"
R=/tmp/repro

run() {  # run <name> <cmd...> — records command, output, exit status
  local name="$1"; shift
  {
    printf '$ %s\n' "$*"
    "$@"
    printf 'exit=%s\n' "$?"
  } > "$OUT/$name.txt" 2>&1
}

emit() {  # emit <name> <case> — codegen a raw case; record exit + whether the
          # output file was produced, then inline it.
  local name="$1" case="$2"
  rm -f "$R/$case.c"
  {
    printf '$ dis86 --config %s.bsl --binary-raw %s.bin --name test '
    printf -- '--emit-code %s.c --codegen-hydra\n' "$case"
    "$BIN" --config "$R/$case.bsl" --binary-raw "$R/$case.bin" --name test \
        --emit-code "$R/$case.c" --codegen-hydra 2>&1
    local rc=$?
    printf 'exit=%s\n' "$rc"
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

listcases() {  # listcases <file> — first column of a case-list file
  cut -d: -f1 "$1"
}

summary() {
  {
    echo "main sha: $(git -C "$MAIN" rev-parse HEAD)"
    echo "main subj: $(git -C "$MAIN" log -1 --format=%s)"
    echo "dis86:    $(sha256sum "$BIN" | cut -c1-16)  ($BIN)"
    rustc --version; cargo --version
  } > "$OUT/00-env.txt"
}

# ------------------------------------------------------------------ #85 -----
for case in $(listcases "$R/85-cases.txt"); do
  emit "85-$case" "$case"
done

# Batch: good + panicking sibling in one codeseg.
rm -f "$R/t85batch.c"
{
  printf '$ dis86 --config t85batch.bsl --binary-raw t85batch.bin '
  printf -- '--codeseg-name CODE --emit-code t85batch.c --codegen-hydra\n'
  "$BIN" --config "$R/t85batch.bsl" --binary-raw "$R/t85batch.bin" \
      --codeseg-name CODE --emit-code "$R/t85batch.c" --codegen-hydra 2>&1
  printf 'exit=%s\n' "$?"
  if [ -f "$R/t85batch.c" ]; then
    printf 'OUTPUT FILE: present (%s bytes)\n' "$(wc -c < "$R/t85batch.c")"
    printf -- '----- t85batch.c -----\n'
    cat "$R/t85batch.c"
  else
    printf 'OUTPUT FILE: MISSING — the good function was discarded too\n'
  fi
} > "$OUT/85-batch.txt" 2>&1

# ------------------------------------------------------------------ #86 -----
for case in $(listcases "$R/86-cases.txt"); do
  emit "86-$case" "$case"
done
for case in d1-cld-movsb d2-std-cld-movsb d7-std-cld-repmovsb d11-std-cld-outsb \
            d16-std-on-side-path; do
  ir "86-$case-ir" "$case"
done

# ------------------------------------------------------------------ #69 -----
for case in $(listcases "$R/69-cases.txt"); do
  emit "69-$case" "$case"
done
for case in r-rcl-stc r-rcl-nocf r-adc8-clc r-adc16-stc r-xblock-carry r-repcmpsb \
            r-repscasb r-shl-adc; do
  ir "69-$case-ir" "$case"
done


# ----------------------------------------------------------- round 3b -------
for case in $(listcases "$R/3b-cases.txt"); do
  emit "3b-$case" "$case"
done
for case in p-scasb-di p-cmpsb-di p-lodsb-si p-stosb-di p-movsb-then-std \
            p-std-cld-repstosb p-std-cld-replodsb; do
  ir "3b-$case-ir" "$case"
done

rm -f "$R/p-batch-df.c"
{
  printf '$ dis86 --config p-batch-df.bsl --binary-raw p-batch-df.bin '
  printf -- '--codeseg-name CODE --emit-code p-batch-df.c --codegen-hydra\n'
  "$BIN" --config "$R/p-batch-df.bsl" --binary-raw "$R/p-batch-df.bin" \
      --codeseg-name CODE --emit-code "$R/p-batch-df.c" --codegen-hydra 2>&1
  printf 'exit=%s\n' "$?"
  [ -f "$R/p-batch-df.c" ] && { printf -- '----- p-batch-df.c -----\n'; cat "$R/p-batch-df.c"; }
} > "$OUT/3b-batch-df.txt" 2>&1

# ------------------------------------------------------- static inventory ----
{
  echo "=== Opcode::Unimpl sites in ir_build.rs (main) ==="
  grep -n "Opcode::Unimpl" "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== panic! sites in ir_build.rs (main) ==="
  grep -n "panic!" "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== df_may_be_set sites (main) ==="
  grep -n "df_may_be_set" "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== opcode_may_set_df (main) ==="
  sed -n '/fn opcode_may_set_df/,/^}/p' "$MAIN/dis86/src/decompile/ir_build.rs"
  echo
  echo "=== UNIMPL macro (hydra support) ==="
  grep -rn "define UNIMPL\|UNIMPL(" "$MAIN/confgen/hydra/gen/" 2>/dev/null | head -20
} > "$OUT/static-inventory.txt" 2>&1

# ------------------------------------------------------- test suite ----------
run tests cargo test --manifest-path "$MAIN/dis86/Cargo.toml" --locked --all-targets

summary
echo "evidence written to $OUT:"
ls "$OUT"
