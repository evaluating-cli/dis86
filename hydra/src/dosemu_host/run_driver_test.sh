#!/bin/bash
# Run the Hydra-on-dosemu2 driver integration tests against fresh headless
# dosemu2 instances:
#   - .exe: Phase 7 Item C MZ test (once)
#   - .com: Phase 4/5/6 integration test (twice consecutively)
#   - cap:  Phase 7 Item D HYDSNAP capture once, then restore the same
#           snapshot on two fresh instances
# TESTPROG_FLAVOR=com|exe|cap|both selects the run(s); default both (all).
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
BUILD="${HYDRA_DOSEMU_BUILD:-$SCRIPT_DIR/../../build/src/dosemu_host}"
COM="$BUILD/testprog.com"
EXE="$BUILD/testprog.exe"
LAUNCH="$BUILD/launch.com"
TEST_COM="$BUILD/test_driver"
TEST_EXE="$BUILD/test_driver_exe"
TEST_CAP="$BUILD/test_driver_cap"
LOG=/tmp/opencode/driver_test.log
CONF=/tmp/opencode/dosemu_mshm.conf
FLAVOR="${TESTPROG_FLAVOR:-both}"

case "$FLAVOR" in
    com|exe|cap|both) ;;
    *) echo "FAIL: TESTPROG_FLAVOR must be com|exe|cap|both (got '$FLAVOR')" >&2; exit 2 ;;
esac

for artifact in "$COM" "$EXE" "$LAUNCH" "$TEST_COM" "$TEST_EXE" "$TEST_CAP"; do
    if [ ! -e "$artifact" ]; then
        echo "FAIL: missing build artifact: $artifact" >&2
        exit 2
    fi
done

mkdir -p /tmp/opencode

# /tmp may be wiped between sessions; regenerate the config if absent.
if [ ! -f "$CONF" ]; then
    cat > "$CONF" <<'EOF'
$_cpu_vm = "emulated"
$_cpuemu = (1)
$_sound = (off)
$_layout = "us"
$_vbios_post = (off)
$_console = (0)
$_video = "vga"
$_hdimage = "+1"
$_mapping = "mapmshm"
EOF
fi

export XDG_RUNTIME_DIR=/tmp/opencode/runtime
mkdir -p "$XDG_RUNTIME_DIR/dosemu2"

cleanup() {
    pkill -9 -x dosemu2.bin 2>/dev/null || true
    pkill -9 -x dosemu 2>/dev/null || true
}
trap cleanup EXIT

run_flavor() {
    local guest="$1" testbin="$2"; shift 2
    local launch_args=("$@")

    cleanup
    sleep 1
    # Stale debugger FIFOs from crashed instances can make the driver's
    # pid discovery latch onto a dead endpoint.
    rm -f "$XDG_RUNTIME_DIR"/dosemu2/dosemu.dbgin.* \
          "$XDG_RUNTIME_DIR"/dosemu2/dosemu.dbgout.* 2>/dev/null || true

    local gname
    gname=$(basename "$guest")
    cp "$guest" "/tmp/opencode/$gname"
    cp "$LAUNCH" /tmp/opencode/launch.com

    rm -f "$LOG"
    setsid bash -c \
        "tail -f /dev/null | timeout 180 /usr/bin/dosemu -p -f '$CONF' -dumb -H1 ${launch_args[*]} -K /tmp/opencode </dev/null >'$LOG' 2>&1" \
        </dev/null >/dev/null 2>&1 &
    echo "launcher pid: $!"

    local DOPID=""
    local i
    for i in $(seq 1 60); do
        DOPID=$(pgrep -x dosemu2.bin | head -1 || true)
        [ -n "$DOPID" ] && break
        sleep 0.5
    done
    if [ -z "$DOPID" ]; then
        echo "FAIL: dosemu2 did not start"
        tail -20 "$LOG" 2>/dev/null || true
        return 2
    fi
    echo "=== dosemu2 pid: $DOPID ==="

    "$testbin" "$DOPID" "/tmp/opencode/$gname"
    local run_rc=$?

    cleanup
    return "$run_rc"
}

rc=0
record_rc() {
    local got="$1"
    if [ "$got" -ne 0 ] && [ "$rc" -eq 0 ]; then
        rc="$got"
    fi
}

case "$FLAVOR" in
exe|both)
    echo "===== .exe flavor (1/1) ====="
    # -E launch.com: dosemu autoexec-EXECs the launcher, which parks after
    # loading the MZ child; test_driver_exe validates and drives the entry.
    run_flavor "$EXE" "$TEST_EXE" -E launch.com
    record_rc $?
    ;;
esac

case "$FLAVOR" in
com|both)
    # The guest is launched by dosemu's autoexec (-E testprog.com); the
    # driver discovers it via its signature scan. Two consecutive fresh
    # instances are required to catch the historical release/breakpoint race.
    echo "===== .com flavor (1/2) ====="
    run_flavor "$COM" "$TEST_COM" -E testprog.com
    record_rc $?

    echo "===== .com flavor (2/2) ====="
    run_flavor "$COM" "$TEST_COM" -E testprog.com
    record_rc $?
    ;;
esac

case "$FLAVOR" in
cap|both)
    # Capture once. Both restores below use this exact snapshot and each
    # run_flavor call starts a fresh dosemu2 instance.
    echo "===== capture stage (1/1, HYDSNAP) ====="
    rm -f /tmp/opencode/cap_state.snap /tmp/opencode/cap_probes.txt
    CAP_MODE=cap run_flavor "$EXE" "$TEST_CAP" -E launch.com
    cap_rc=$?
    if [ ! -s /tmp/opencode/cap_state.snap ]; then
        echo "FAIL: no snapshot written"
        cap_rc=1
    fi
    record_rc "$cap_rc"

    if [ "$cap_rc" -eq 0 ]; then
        echo "===== restore stage (1/2, fresh instance) ====="
        CAP_MODE=restore run_flavor "$EXE" "$TEST_CAP" -E launch.com
        record_rc $?

        echo "===== restore stage (2/2, fresh instance, same snapshot) ====="
        CAP_MODE=restore run_flavor "$EXE" "$TEST_CAP" -E launch.com
        record_rc $?
    fi
    ;;
esac

exit "$rc"
