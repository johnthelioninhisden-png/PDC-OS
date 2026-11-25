#!/bin/sh
set -eu

image=${1:?usage: qemu-smoke-test.sh IMAGE LOG QEMU TIMEOUT_SECONDS}
log=${2:?usage: qemu-smoke-test.sh IMAGE LOG QEMU TIMEOUT_SECONDS}
qemu=${3:-qemu-system-x86_64}
timeout_seconds=${4:-10}
profile=${5:-baseline}

if ! command -v "$qemu" >/dev/null 2>&1; then
    echo "QEMU executable not found: $qemu" >&2
    exit 1
fi
if ! command -v timeout >/dev/null 2>&1; then
    echo "timeout command is required for the QEMU smoke test" >&2
    exit 1
fi
if [ ! -f "$image" ]; then
    echo "Kernel ISO not found: $image" >&2
    exit 1
fi

mkdir -p "$(dirname "$log")"
rm -f "$log"

case "$profile" in
    baseline) set -- -m 256M -smp 1 ;;
    smp) set -- -m 256M -smp 2 ;;
    mem128) set -- -m 128M -smp 1 ;;
    mem256) set -- -m 256M -smp 1 ;;
    mem1g) set -- -m 1G -smp 1 ;;
    ata) set -- -m 256M -smp 1 -drive file=build/qemu-ata.img,if=ide,format=raw ;;
    ps2) set -- -m 256M -smp 1 -nodefaults -machine pc -vga std ;;
    serial) set -- -m 256M -smp 1 -nodefaults -machine pc ;;
    vga) set -- -m 256M -smp 1 -vga std ;;
    *)
        echo "Unknown QEMU hardware profile: $profile" >&2
        exit 1
        ;;
esac

set +e
timeout --signal=TERM --kill-after=2s "${timeout_seconds}s" \
    "$qemu" "$@" -cdrom "$image" -display none \
    -serial "file:$log" -monitor none -no-reboot -no-shutdown
qemu_status=$?
set -e

if [ "$qemu_status" -ne 124 ] && [ "$qemu_status" -ne 137 ]; then
    echo "QEMU exited before the smoke-test timeout (status $qemu_status)" >&2
    [ ! -f "$log" ] || cat "$log" >&2
    exit 1
fi

if [ "$qemu_status" -eq 124 ] || [ "$qemu_status" -eq 137 ]; then
    if [ -f "$log" ]; then
        last_stage=$(grep "PDC-OS boot stage:" "$log" | tail -n 1 || true)
        [ -n "$last_stage" ] && echo "QEMU boot watchdog observed last stage: $last_stage" >&2
    fi
fi

for marker in \
    "PDC-OS boot: entering x86-64 kernel" \
    "PDC-OS kernel entry" \
    "PDC-OS boot stage: serial" \
    "PDC-OS boot stage: cpu" \
    "PDC-OS boot stage: memory" \
    "PDC-OS boot stage: scheduler" \
    "PDC-OS boot stage: interrupts" \
    "PDC-OS boot stage: filesystem" \
    "PDC-OS boot stage: userspace" \
    "PDC-OS boot stage: terminal" \
    "PDC-OS boot stage: complete" \
    "Hardware: CPU vendor=" \
    "Hardware: interrupt-controller=legacy-PIC" \
    "Display mode: linear framebuffer (1024x768), framebuffer enabled" \
    "Memory: usable=" \
    "Memory and scheduler initialized" \
    "Interrupts and PIT initialized" \
    "PDC-OS userspace startup complete" \
    "Security review: 10.7" \
    "Architecture verification: failure-paths" \
    "Architecture verification: Stage 1 cases=" \
    "PDC-OS boot complete" \
    "Performance:"
do
    if ! grep -Fq "$marker" "$log"; then
        echo "Smoke test did not observe expected boot marker: $marker" >&2
        [ ! -f "$log" ] || cat "$log" >&2
        exit 1
    fi
done

verification_summary=$(grep "Architecture verification: Stage 1 cases=" "$log" | tail -n 1 || true)
if [ -z "$verification_summary" ]; then
    echo "QEMU smoke test did not report the architecture verification summary" >&2
    exit 1
fi
case "$verification_summary" in
    *" failures=0"*) ;;
    *)
        echo "Architecture verification reported failures: $verification_summary" >&2
        cat "$log" >&2
        exit 1
        ;;
esac

if grep -Fq "KERNEL PANIC:" "$log" || grep -Fq "KERNEL EXCEPTION:" "$log"; then
    echo "Kernel reported a panic or exception during boot" >&2
    cat "$log" >&2
    exit 1
fi

echo "QEMU boot smoke test passed; serial log: $log"
