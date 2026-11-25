#!/bin/sh
set -eu

release_dir=${1:?usage: verify-release.sh RELEASE_DIR ...}

if [ ! -d "$release_dir" ]; then
    echo "Release directory not found: $release_dir" >&2
    exit 1
fi

(
    cd "$release_dir"
    sha256sum -c SHA256SUMS
)

if [ ! -s "$release_dir/pdc-os.kernel" ]; then
    echo "Release ELF is missing or empty" >&2
    exit 1
fi
if [ ! -s "$release_dir/pdc-os.iso" ]; then
    echo "Release ISO is missing or empty" >&2
    exit 1
fi
if ! grep -Fq "PDC-OS boot complete" "$release_dir/qemu-smoke.log"; then
    echo "Baseline release smoke log did not complete" >&2
    exit 1
fi

printf 'PDC-OS release verification passed: %s\n' "$release_dir"
