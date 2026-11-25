#!/bin/sh
set -eu

image=${1:?usage: qemu-profile.sh IMAGE QEMU PROFILE}
qemu=${2:-qemu-system-x86_64}
profile=${3:-baseline}

if ! command -v "$qemu" >/dev/null 2>&1; then
    echo "QEMU executable not found: $qemu" >&2
    exit 1
fi
if [ ! -f "$image" ]; then
    echo "Kernel ISO not found: $image" >&2
    exit 1
fi

case "$profile" in
    baseline) set -- -m 256M -smp 1 ;;
    smp) set -- -m 256M -smp 2 ;;
    mem128) set -- -m 128M -smp 1 ;;
    mem256) set -- -m 256M -smp 1 ;;
    mem1g) set -- -m 1G -smp 1 ;;
    ata) set -- -m 256M -smp 1 -drive file=build/qemu-ata.img,if=ide,format=raw ;;
    ps2) set -- -m 256M -smp 1 -nodefaults -machine pc -vga std ;;
    serial) set -- -m 256M -smp 1 -nodefaults -machine pc -serial stdio ;;
    vga) set -- -m 256M -smp 1 -vga std ;;
    *)
        echo "Unknown QEMU hardware profile: $profile" >&2
        exit 1
        ;;
esac

# Keep the serial-only profile in the user's terminal. Graphical profiles show
# the guest's requested framebuffer in a window; Ctrl+Alt+F toggles fullscreen.
if [ "$profile" = serial ]; then
    "$qemu" "$@" -cdrom "$image" -monitor none
else
    "$qemu" "$@" -cdrom "$image" -display gtk,zoom-to-fit=on -monitor none
fi
