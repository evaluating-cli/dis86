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
    "$ROOT"/hydra/src/*.c \
    "$ROOT"/hydra/src/native_host/native_host.c \
    "$ROOT"/hydra/src/native_host/backend_terminal.c \
    "$ROOT"/hydra/src/native_host/int_dispatch.c \
    "$ROOT"/hydra/src/native_host/backend_host.c \
    "$OUT/parity_appdata.c" "$OUT/fixture_code.c" "$ROOT/parity/native_runner.c" \
    -rdynamic -pthread -ldl -o "$OUT/native_runner"

"$OUT/native_runner" "$OUT/fixture.exe" "$OUT/native.lanes"
echo "Gate 2 native: PASS ($OUT/native.lanes)"

# ---- 4. trace lane (Gate 3) ------------------------------------------------
#
# The instrumented-execution tool (#97) must be able to say what the
# machine did, and its correctness has to ride on the anchored fixture
# rather than on itself. So the fixture is run again with the trace sink
# open, and the observations the trace holds are checked against the
# fixture's known layout - the same layout the generated config states, so
# both sides read one contract.
#
# The expected addresses are read from the generated config (which comes
# from parity/annotations.py) and shifted by the code load segment the
# native runner uses. Nothing here is a second copy of the fixture's
# layout: a change to the annotations moves the expectation with it.

TRACE="$OUT/fixture.trace"
CODE_LOAD=1

"$OUT/native_runner" "$OUT/fixture.exe" "$OUT/native.lanes" --trace "$TRACE"

[ -s "$TRACE" ] || { echo "FAIL: Gate 3 produced no trace" >&2; exit 1; }

bsl_addr() { # <function name> -> the image-relative "seg:off" it starts at
    awk -v fn="$1" '$1 == fn { for (i = 1; i <= NF; i++) if ($i == "start") { print $(i+1); exit } }' \
        "$OUT/fixture.bsl"
}

abs_addr() { # <"seg:off"> -> the absolute guest address the trace records
    local seg="${1%%:*}" off="${1##*:}"
    printf "%04x:%04x" $(( 0x$seg + CODE_LOAD )) $(( 0x$off ))
}

MAIN_ABS=$(abs_addr "$(bsl_addr F_parity_main)")
LEAF_ABS=$(abs_addr "$(bsl_addr F_parity_leaf)")

[ -n "$MAIN_ABS" ] && [ -n "$LEAF_ABS" ] || {
    echo "FAIL: Gate 3 could not read the fixture's function addresses from the config" >&2
    exit 1
}

check_trace() { # <file> <label> <extended regex>
    grep -Eq -- "$3" "$1" && return 0
    echo "FAIL: Gate 3 $2" >&2
    echo "--- trace ---" >&2
    cat "$1" >&2
    exit 1
}

# The patterns are kept in variables so the mutation check below can run
# exactly the same check against a tampered trace.
LEAF_CALL_RE="^native_call from=[0-9a-f]{4}:[0-9a-f]{4} to=${LEAF_ABS} name=F_parity_leaf nargs=0\$"
LEAF_RET_RE="^native_ret from=[0-9a-f]{4}:[0-9a-f]{4} to=${LEAF_ABS} dx=[0-9a-f]{4} ax=[0-9a-f]{4}\$"

check_trace "$TRACE" "the trace has no version header" '^# hydra trace v1$'
check_trace "$TRACE" "the trace does not record the code load" "^# code_load=$(printf '%04x' $CODE_LOAD)\$"
check_trace "$TRACE" "the entry function was not dispatched" \
    "^dispatch enter cs=${MAIN_ABS%%:*} ip=${MAIN_ABS##*:} name=F_parity_main\$"
check_trace "$TRACE" "the call into the leaf was not recorded" "$LEAF_CALL_RE"
check_trace "$TRACE" "the value the leaf returned was not recorded" "$LEAF_RET_RE"

# The mutation check: the gate must fail when the observation is wrong, so
# that a green run means "the tool saw what the fixture says" and not
# merely "the tool ran".
MUT="$OUT/fixture.trace.mutated"
sed "s/to=${LEAF_ABS}/to=dead:beef/g" "$TRACE" > "$MUT"
if grep -Eq -- "$LEAF_CALL_RE" "$MUT"; then
    echo "FAIL: Gate 3 mutation check: a wrong leaf address still passes" >&2
    exit 1
fi
rm -f "$MUT"

echo "Gate 3 trace: PASS ($TRACE; main=$MAIN_ABS leaf=$LEAF_ABS)"
