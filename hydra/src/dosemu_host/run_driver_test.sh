#!/bin/bash
# Run the Hydra-on-dosemu2 driver integration tests against fresh headless
# dosemu2 instances:
#   - .com  flavor: Phase 4/5/6 test (testprog.com, test_driver)
#   - .exe  flavor: Phase 7 Item C MZ/bpload test (testprog.exe,
#                   test_driver_exe)
# TESTPROG_FLAVOR=com|exe|both selects the run(s); default both.
set -u

BUILD=/home/p/dis86/hydra/build/src/dosemu_host
COM="$BUILD/testprog.com"
EXE="$BUILD/testprog.exe"
LAUNCH="$BUILD/launch.com"
TEST_COM="$BUILD/test_driver"
TEST_EXE="$BUILD/test_driver_exe"
LOG=/tmp/opencode/driver_test.log
CONF=/tmp/opencode/dosemu_mshm.conf
FLAVOR="${TESTPROG_FLAVOR:-both}"

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
mkdir -p $XDG_RUNTIME_DIR/dosemu2

run_flavor() {
    local guest="$1" testbin="$2"; shift 2
    local launch_args=("$@")

    pkill -9 -x dosemu2.bin 2>/dev/null
    pkill -9 -x dosemu 2>/dev/null
    sleep 1
    # stale debugger fifos from crashed instances can make the driver's
    # pid discovery latch onto a dead endpoint
    rm -f $XDG_RUNTIME_DIR/dosemu2/dosemu.dbgin.* \
          $XDG_RUNTIME_DIR/dosemu2/dosemu.dbgout.* 2>/dev/null

    local gname
    gname=$(basename "$guest")
    cp "$guest" "/tmp/opencode/$gname"
    [ -n "${LAUNCH:-}" ] && [ -f "$LAUNCH" ] && cp "$LAUNCH" /tmp/opencode/launch.com

    rm -f $LOG
    setsid bash -c "tail -f /dev/null | timeout 180 /usr/bin/dosemu -p -f $CONF -dumb -H1 ${launch_args[*]} -K /tmp/opencode </dev/null >$LOG 2>&1" </dev/null >/dev/null 2>&1 &
    echo "launcher pid: $!"

    local DOPID=""
    for i in $(seq 1 60); do
        DOPID=$(pgrep -x dosemu2.bin | head -1)
        [ -n "$DOPID" ] && break
        sleep 0.5
    done
    if [ -z "$DOPID" ]; then
        echo "FAIL: dosemu2 did not start"
        tail -20 $LOG
        return 2
    fi
    echo "=== dosemu2 pid: $DOPID ==="

    "$testbin" "$DOPID" "/tmp/opencode/$gname"
    local rc=$?

    pkill -9 -x dosemu2.bin 2>/dev/null
    pkill -9 -x dosemu 2>/dev/null
    return $rc
}

rc=0

case "$FLAVOR" in
com|both)
    echo "===== .com flavor ====="
    # The guest is launched by dosemu's autoexec (-E testprog.com); the
    # driver discovers it via its signature scan.
    run_flavor "$COM" "$TEST_COM" -E testprog.com
    rc=$?
    ;;
esac

case "$FLAVOR" in
exe|both)
    echo "===== .exe flavor (bpload) ====="
    # -E launch.com: dosemu autoexec-EXECs the launcher, which marks 0040:00F5
    # and spins; test_driver_exe waits for that marker over shared memory,
    # parks the machine and lets host_run() arm bpload so the loader stop
    # lands on the launcher's INT21 AH=4B00 -> MZ entry of testprog.exe.
    run_flavor "$EXE" "$TEST_EXE" -E launch.com
    rc2=$?
    [ $rc -eq 0 ] && rc=$rc2
    ;;
esac

exit $rc
