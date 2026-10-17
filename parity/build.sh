#!/bin/bash
# build.sh - Gate 1 (decompile) + Gate 2 native side of the #96 parity lane.
#
#   Gate 1: assemble parity/fixture.asm, generate the dis86 config and the
#           Hydra appdata metadata from parity/annotations.py, decompile the
#           fixture with --codegen-hydra and fail on any UNIMPL.
#   Gate 2: build parity/native_runner against the standalone native host and
#           run it; assert the outbox matches the shared contract
#           (parity/outbox.h) and dump the compared snapshot.
#
# The golden side is parity/run_golden.sh (needs dosemu2); it compares its
# snapshot against the native.lanes produced here.
#
# Nothing in this script needs dosemu2.
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUT="$ROOT/build/parity"
DIS86="$ROOT/dis86/target/debug/dis86"

mkdir -p "$OUT"

# ---- 0. toolchain ----------------------------------------------------------

command -v nasm >/dev/null || { echo "FAIL: nasm not found" >&2; exit 2; }
command -v cc >/dev/null || { echo "FAIL: cc not found" >&2; exit 2; }
if [ ! -x "$DIS86" ]; then
    echo "building dis86 ..."
    (cd "$ROOT/dis86" && cargo build --locked --quiet)
fi

# ---- 1. fixture layout <-> outbox.h contract -------------------------------

asm_equ() { grep -E "^$1[[:space:]]+equ[[:space:]]+" "$ROOT/parity/fixture.asm" | head -1 | awk '{print $3}'; }
hdr_def() { grep -E "^#define $1[[:space:]]+" "$ROOT/parity/outbox.h" | head -1 | awk '{print $3}'; }

check_eq() {
    local what="$1" asm_val="$2" hdr_val="$3"
    if [ -z "$asm_val" ] || [ "$asm_val" != "$hdr_val" ]; then
        echo "FAIL: contract drift on $what: fixture.asm='$asm_val' outbox.h='$hdr_val'" >&2
        exit 1
    fi
}
check_eq OUTBOX_M        "$(asm_equ OUTBOX_M)"        "$(hdr_def OUTBOX_M)"
check_eq LANE_M          "$(asm_equ LANE_M)"          "$(hdr_def OUTBOX_LANE_M)"
check_eq PARK_M          "$(asm_equ PARK_M)"          "$(hdr_def OUTBOX_PARK_M)"
check_eq OUTBOX_VERSION  "$(asm_equ OUTBOX_VERSION)"  "$(hdr_def OUTBOX_VERSION)"
check_eq HDR_SIZE        "$(asm_equ HDR_SIZE)"        "$(hdr_def FIXTURE_HEADER_SIZE)"
check_eq MODULE_SIZE     "$(asm_equ MODULE_SIZE)"     "$(hdr_def FIXTURE_MODULE_SIZE)"

# ---- 2. assemble + decompile (Gate 1) --------------------------------------

nasm -f bin -w-zeroing -o "$OUT/fixture.exe" "$ROOT/parity/fixture.asm"

PYTHONPATH="$ROOT/confgen" python3 "$ROOT/confgen/generate.py" \
    "$ROOT/parity/annotations.py" --dis86-conf --output-path "$OUT/fixture.bsl"
PYTHONPATH="$ROOT/confgen" python3 "$ROOT/confgen/generate.py" \
    "$ROOT/parity/annotations.py" --appdata-hdr --output-path "$OUT/hydra_user_appdata.h"
PYTHONPATH="$ROOT/confgen" python3 "$ROOT/confgen/generate.py" \
    "$ROOT/parity/annotations.py" --appdata-src --output-path "$OUT/parity_appdata.c"
rm -rf "$ROOT/confgen"/hydra/gen/__pycache__ "$ROOT/confgen"/hydra/__pycache__ 2>/dev/null || true

"$DIS86" --config "$OUT/fixture.bsl" --binary-exe "$OUT/fixture.exe" \
    --codeseg-name parity_fixture --emit-code "$OUT/fixture_main.c" --codegen-hydra

if grep -q UNIMPL "$OUT/fixture_main.c"; then
    echo "FAIL: Gate 1 emitted UNIMPL instructions" >&2
    grep -n UNIMPL "$OUT/fixture_main.c" >&2
    exit 1
fi
for hook in main leaf; do
    n=$(grep -c "^HYDRA_FUNC(H_parity_$hook)" "$OUT/fixture_main.c" || true)
    [ "$n" = 1 ] || { echo "FAIL: expected one H_parity_$hook, found $n" >&2; exit 1; }
    grep -q "HYDRA_REGISTER(parity_$hook)" "$OUT/hydra_user_appdata.h" || {
        echo "FAIL: parity_$hook not registered in appdata" >&2; exit 1; }
done
grep -q "F_parity_leaf(m)" "$OUT/fixture_main.c" || {
    echo "FAIL: entry does not call the leaf callstub" >&2; exit 1; }
echo "Gate 1: PASS (2 hooks, no UNIMPL)"

# The include-free dis86 output gets the Hydra + appdata headers it expects.
cat > "$OUT/fixture_code.c" <<'EOF'
#include "hydra_user_appdata.h"
#include "fixture_main.c"
EOF

# ---- 3. native runner (Gate 2) ---------------------------------------------

cc -std=c11 -O2 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -I"$ROOT/hydra/src" -I"$ROOT/hydra/include" \
    -I"$ROOT/hydra/src/native_host" -I"$OUT" \
    "$ROOT"/hydra/src/*.c "$ROOT"/hydra/src/native_host/native_host.c \
    "$OUT/parity_appdata.c" "$OUT/fixture_code.c" "$ROOT/parity/native_runner.c" \
    -rdynamic -pthread -ldl -o "$OUT/native_runner"

"$OUT/native_runner" "$OUT/fixture.exe" "$OUT/native.lanes"
echo "Gate 2 native: PASS ($OUT/native.lanes)"
