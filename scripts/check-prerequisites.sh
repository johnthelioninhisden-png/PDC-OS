#!/bin/sh
set -u

grub_builder=${1:-grub2-mkrescue}
qemu=${2:-qemu-system-x86_64}
xorriso=${3:-xorriso}
failures=0

check_command() {
    name=$1
    command_name=$2
    required=$3

    # Print successful tools as a single repeatable host-state line.
    if command -v "$command_name" >/dev/null 2>&1; then
        printf 'OK   %-12s %s\n' "$name" "$(command -v "$command_name")"
    else
        printf 'MISS %-12s %s\n' "$name" "$command_name"
        if [ "$required" = required ]; then
            failures=$((failures + 1))
        fi
    fi
}

printf 'PDC-OS host prerequisite check\n'
check_command rust cargo required
check_command rustc rustc required
check_command ld ld required
check_command nasm nasm required
check_command qemu "$qemu" required
check_command xorriso "$xorriso" required

if command -v "$grub_builder" >/dev/null 2>&1; then
    printf 'OK   %-12s %s\n' grub "$grub_builder"
else
    printf 'MISS %-12s %s\n' grub "$grub_builder"
    failures=$((failures + 1))
fi

check_command timeout timeout required
check_command sha256sum sha256sum required
check_command tar tar required

if [ "$failures" -ne 0 ]; then
    printf 'Prerequisite check failed with %d missing required tool(s).\n' "$failures" >&2
    exit 1
fi

# Validate the Rust pin and build-std sources after tool discovery succeeds.
if ! rustc --version >/dev/null 2>&1; then
    echo 'rustc is installed but could not report a version.' >&2
    exit 1
fi
sysroot=$(rustc --print sysroot 2>/dev/null || true)
if [ -z "$sysroot" ]; then
    echo 'rustc sysroot lookup failed.' >&2
    exit 1
fi
if [ ! -d "$sysroot/lib/rustlib/src/rust/library" ]; then
    echo 'Missing rust-src; install it with: rustup component add rust-src --toolchain nightly' >&2
    exit 1
fi
printf 'OK   rust-pin   rust-toolchain.toml nightly with rust-src\n'
printf 'Prerequisite check passed.\n'
