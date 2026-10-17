#!/bin/bash
# run_golden.sh - Gate 2 golden side of the #96 parity lane.
#
# Boots the SAME parity/fixture.exe under headless dosemu2 (-E, mapmshm
# lowmem), waits for the fixture to publish its outbox in live guest memory,
# captures the snapshot, and byte-compares it with the native lane produced
# by parity/build.sh. A tampered snapshot must fail the comparison, so a
# broken cmp can never pass this gate.
#
# Requirements: dosemu2 built with mapmshm, /proc access to the guest's fd
# table, nasm/cc, and a completed `parity/build.sh` run.
#
# NOTE: do NOT add -H1 here. dosemu2's -H1 locks the guest before the
# bootsector until a dosdebug client attaches, so a free-running golden lane
# that never attaches would never boot. (See parity/README.md.)
#
# usage: run_golden.sh [--keep]
#   --keep   leave build/parity/golden.lanes and the dosemu log in place
#            (they are kept anyway; --keep only skips removing the tempdir)
# env: PARITY_TIMEOUT  seconds to wait for the outbox (default 90)
#      DOSEMU         dosemu2 launcher (default: dosemu from PATH)
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUT="$ROOT/build/parity"
TIMEOUT=${PARITY_TIMEOUT:-90}
DOSEMU=${DOSEMU:-$(command -v dosemu || true)}
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$OUT/fixture.exe" ] || fail "missing $OUT/fixture.exe (run parity/build.sh first)"
[ -f "$OUT/native.lanes" ] || fail "missing $OUT/native.lanes (run parity/build.sh first)"
[ -n "$DOSEMU" ] || fail "dosemu2 not found (set DOSEMU=...)"

# ---- 1. golden capture tool ------------------------------------------------

echo "building golden_capture ..."
cc -std=c11 -O2 -D_GNU_SOURCE -Dtypeof=__typeof__ \
    -I"$ROOT/hydra/src/dosemu_host" \
    "$ROOT/parity/golden_capture.c" \
    "$ROOT/hydra/src/dosemu_host/lowmem.c" \
    -o "$OUT/golden_capture"

# ---- 2. session workspace ---------------------------------------------------

RUN=$(mktemp -d /tmp/parity-golden.XXXXXX)
chmod 700 "$RUN"
CONF="$RUN/dosemu.conf"
LOG="$RUN/dosemu.log"
RUNTIME="$RUN/runtime"
mkdir -m 700 "$RUNTIME"
mkdir -p "$RUNTIME/dosemu2"
cp "$OUT/fixture.exe" "$RUN/fixture.exe"

cat > "$CONF" <<'EOF'
$_cpu_vm = "emulated"
$_cpu_vm_dpmi = "emulated"
$_cpuemu = (1)
$_sound = (off)
$_layout = "us"
$_vbios_post = (off)
$_console = (0)
$_video = "vga"
$_hdimage = "+1"
$_mapping = "mapmshm"
EOF

export XDG_RUNTIME_DIR="$RUNTIME"

SESSION_PID=
DOPID=
cleanup() {
    [ -n "$SESSION_PID" ] && kill -TERM -- "-$SESSION_PID" 2>/dev/null || true
    [ -n "$DOPID" ] && kill -TERM "$DOPID" 2>/dev/null || true
    if [ -n "$SESSION_PID" ]; then
        for _ in $(seq 1 20); do
            kill -0 -- "-$SESSION_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -KILL -- "-$SESSION_PID" 2>/dev/null || true
    fi
    cp "$LOG" "$OUT/golden-dosemu.log" 2>/dev/null || true
    if [ "$KEEP" = 0 ]; then
        rm -rf "$RUN"
    else
        echo "kept session dir: $RUN"
    fi
}
trap cleanup EXIT

# setsid + a never-writing stdin pipe mirrors hydra's dosemu host tests:
# dosemu must not own the caller's terminal. No -H1: free-run.
setsid bash -c "tail -f /dev/null | timeout $((TIMEOUT + 30)) '$DOSEMU' -p -f '$CONF' -dumb -E fixture.exe -K '$RUN' </dev/null >'$LOG' 2>&1" \
    </dev/null >/dev/null 2>&1 &
SESSION_PID=$!

for _ in $(seq 1 160); do
    for candidate in $(pgrep -x dosemu2.bin 2>/dev/null || true); do
        sid=$(ps -o sid= -p "$candidate" 2>/dev/null | tr -d ' ')
        if [ "$sid" = "$SESSION_PID" ]; then
            DOPID=$candidate
            break 2
        fi
    done
    sleep 0.25
done
[ -n "$DOPID" ] || { tail -20 "$LOG" 2>/dev/null || true; fail "dosemu2 did not start"; }
echo "dosemu2 pid: $DOPID (session $SESSION_PID)"

# ---- 3. capture + compare ---------------------------------------------------

"$OUT/golden_capture" "$DOPID" "$OUT/golden.lanes" "$TIMEOUT"
cmp "$OUT/native.lanes" "$OUT/golden.lanes" ||
    fail "native and golden outbox snapshots differ"

# Negative control: a single tampered byte must break the comparison.
cp "$OUT/golden.lanes" "$RUN/tampered.lanes"
printf '\x00' | dd of="$RUN/tampered.lanes" bs=1 seek=16 conv=notrunc status=none
if cmp -s "$OUT/native.lanes" "$RUN/tampered.lanes"; then
    fail "tamper probe: cmp accepted a corrupted snapshot"
fi

echo "Gate 2 golden: PASS (native == golden == tampered-rejected)"
