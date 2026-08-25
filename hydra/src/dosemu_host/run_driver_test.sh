#!/bin/bash
# Run the Phase 4/5 driver integration test against a fresh headless dosemu2
# instance running testprog.com.
set -u

COM="${1:-/home/p/dis86/hydra/build/src/dosemu_host/testprog.com}"
TEST="${2:-/home/p/dis86/hydra/build/src/dosemu_host/test_driver}"
LOG=/tmp/opencode/driver_test.log
CONF=/tmp/opencode/dosemu_mshm.conf

pkill -9 -x dosemu2.bin 2>/dev/null
pkill -9 -x dosemu 2>/dev/null
sleep 1

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

cp "$COM" /tmp/opencode/testprog.com

export XDG_RUNTIME_DIR=/tmp/opencode/runtime
mkdir -p $XDG_RUNTIME_DIR/dosemu2

rm -f $LOG
setsid bash -c "tail -f /dev/null | timeout 180 /usr/bin/dosemu -p -f $CONF -dumb -H1 -E testprog.com -K /tmp/opencode </dev/null >$LOG 2>&1" </dev/null >/dev/null 2>&1 &
echo "launcher pid: $!"

for i in $(seq 1 60); do
    DOPID=$(pgrep -x dosemu2.bin | head -1)
    [ -n "$DOPID" ] && break
    sleep 0.5
done
if [ -z "$DOPID" ]; then
    echo "FAIL: dosemu2 did not start"
    tail -20 $LOG
    exit 2
fi
echo "=== dosemu2 pid: $DOPID ==="

"$TEST" "$DOPID" "$COM"
RC=$?

pkill -9 -x dosemu2.bin 2>/dev/null
pkill -9 -x dosemu 2>/dev/null
exit $RC