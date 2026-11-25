#!/bin/sh
set -eu

version=${1:?usage: build-release.sh VERSION ELF ISO GRUB CONFIG RELEASE_DIR QEMU GRUB_MKRESCUE XORRISO TIMEOUT}
elf=${2:?usage: build-release.sh VERSION ELF ISO GRUB CONFIG RELEASE_DIR QEMU GRUB_MKRESCUE XORRISO TIMEOUT}
iso=${3:?usage: build-release.sh VERSION ELF ISO GRUB CONFIG RELEASE_DIR QEMU GRUB_MKRESCUE XORRISO TIMEOUT}
grub_cfg=${4:?usage: build-release.sh VERSION ELF ISO GRUB CONFIG RELEASE_DIR QEMU GRUB_MKRESCUE XORRISO TIMEOUT}
release_dir=${5:?usage: build-release.sh VERSION ELF ISO GRUB CONFIG RELEASE_DIR QEMU GRUB_MKRESCUE XORRISO TIMEOUT}
qemu=${6:-qemu-system-x86_64}
grub_mkrescue=${7:-grub-mkrescue}
xorriso=${8:-xorriso}
timeout_seconds=${9:-10}

source_epoch=${SOURCE_DATE_EPOCH:-946684800}

mkdir -p "$release_dir/boot/grub" "$release_dir/smoke"
cp "$elf" "$release_dir/pdc-os.kernel"
cp "$iso" "$release_dir/pdc-os.iso"

# Preserve a versioned copy of the exact menu configuration used in the ISO.
{
    printf '# PDC-OS release %s\n' "$version"
    cat "$grub_cfg"
} > "$release_dir/boot/grub/grub.cfg"

profiles="baseline smp mem128 mem256 mem1g ata ps2 serial vga"
mkdir -p build

for profile in $profiles; do
    log="$release_dir/smoke/$profile.log"
    if [ "$profile" = ata ]; then
        truncate -s 1M build/qemu-ata.img
    fi
    ./scripts/qemu-smoke-test.sh "$iso" "$log" "$qemu" "$timeout_seconds" "$profile"
done

# The baseline serial log is kept at the top level for conventional reuse.
cp "$release_dir/smoke/baseline.log" "$release_dir/qemu-smoke.log"

# Count implementation lines from the source directories and build recipes.
# This is a deterministic, auditable LOC measurement rather than generated-artifact LOC.
loc=$(
    {
        cat Makefile Cargo.toml rust-toolchain.toml linker.ld boot/grub/grub.cfg
        find arch src scripts -type f \
            \( -name '*.rs' -o -name '*.asm' -o -name '*.sh' \) \
            -exec cat {} +
    } | wc -l | tr -d ' '
)

rust_version=$(rustc --version 2>/dev/null | head -n1 || printf 'unknown')
cargo_version=$(cargo --version 2>/dev/null | head -n1 || printf 'unknown')
nasm_version=$(nasm -v 2>/dev/null | head -n1 || printf 'unknown')
ld_version=$(ld --version 2>/dev/null | head -n1 || printf 'unknown')
xorriso_version=$("$xorriso" -version 2>/dev/null | head -n1 || printf 'unknown')
grub_version=$("$grub_mkrescue" --version 2>/dev/null | head -n1 || printf 'unknown')
qemu_version=$("$qemu" --version 2>/dev/null | head -n1 || printf 'unknown')

if git rev-parse --verify HEAD >/dev/null 2>&1; then
    source_revision=$(git rev-parse HEAD)
else
    source_revision="not-a-git-repository"
fi

cat > "$release_dir/version-manifest.txt" <<EOF
PDC-OS release manifest
release_version=$version
package_name=pdc_os
source_revision=$source_revision
source_date_epoch=$source_epoch
rust_target=x86_64-unknown-none
profile=release
implementation_loc=$loc
rust=$rust_version
cargo=$cargo_version
nasm=$nasm_version
ld=$ld_version
grub_mkrescue=$grub_version
xorriso=$xorriso_version
qemu=$qemu_version
artifacts=pdc-os.kernel,pdc-os.iso,boot/grub/grub.cfg
smoke_profiles=$profiles
elf_reproducible=byte-for-byte-under-fixed-toolchain
iso_reproducible=toolchain-dependent-volume-uuid-catalog-metadata
EOF

# Build a byte-stable archive: sorted entries, fixed mtimes, and no gzip name.
tar --sort=name \
    --mtime="@$source_epoch" \
    --owner=0 --group=0 --numeric-owner \
    -C build -cf build/pdc-os-release.tar release
gzip -n -9 -c build/pdc-os-release.tar > "$release_dir/pdc-os-$version.tar.gz"
rm -f build/pdc-os-release.tar

# Generate checksums for every reusable file except this checksum index itself.
(
    cd "$release_dir"
    find . -type f ! -name SHA256SUMS -print |
        sort |
        while IFS= read -r file; do
            sha256sum "$file"
        done
) > "$release_dir/SHA256SUMS"

printf 'PDC-OS release %s prepared at %s\n' "$version" "$release_dir"
printf 'Implementation LOC: %s\n' "$loc"
