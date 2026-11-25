# PDC-OS Operations and Hardware Support

This document is the supported-configuration and recovery guide for repository
builds. PDC-OS is a bootable x86-64 kernel prototype with an in-kernel shell;
it is not a general-purpose hardware certification.

## Supported baseline

| Area | Supported or implemented behavior |
| --- | --- |
| CPU | x86-64 with PAE and long mode. CPUID vendor and PAE/long-mode features are reported on serial. |
| Firmware | Legacy BIOS/UEFI firmware through a GRUB Multiboot v1 entry. |
| Boot | 32-bit bootstrap, 64 MiB identity map, PAE, and 64-bit long mode. |
| Memory | 128 MiB minimum practical target; 256 MiB baseline; 1 GiB tested profile. Usable ranges come from the Multiboot memory map. |
| Storage | ATA PIO primary-master detection and a raw 1 MiB QEMU test image. ATA remains experimental. |
| Display | 512x512x32 RGB linear framebuffer with an 80x25 text console; VGA text is the fallback. |
| Keyboard | i8042/PS/2 set-1, modifiers, navigation keys, repeat, queue reporting, and foreground-console routing. |
| Interrupts | Legacy 8259 PIC and PIT. Two-vCPU execution remains single-threaded and experimental. |
| Diagnostics | COM1 at I/O base 0x3F8, VGA text mirror, serial boot-stage and panic context. |
| Development host | Linux with 64-bit binutils, NASM, Rust nightly plus rust-src, GRUB rescue tools, xorriso, Make, and QEMU x86-64. |

## Exact build prerequisites

The repository pins Rust nightly in `rust-toolchain.toml` and uses
`cargo build -Zbuild-std=core,compiler_builtins`. It also requires GNU Make,
64-bit GNU binutils `ld`, NASM, QEMU `qemu-system-x86_64`, xorriso, GRUB
`grub2-mkrescue` or `grub-mkrescue`, and GNU `timeout` for smoke tests.

Fedora:

```sh
sudo dnf install make binutils nasm rust rust-src \
  qemu-system-x86-core grub2-tools-extra xorriso
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
```

Debian or Ubuntu:

```sh
sudo apt update
sudo apt install make binutils nasm curl qemu-system-x86 xorriso grub-pc-bin
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
```

Validate the host before building:

```sh
make doctor
make build
```

Override host tools installed outside `PATH`:

```sh
make test QEMU=/usr/bin/qemu-system-x86_64 \
  GRUB_MKRESCUE=/usr/bin/grub2-mkrescue
```

## Container workflow

A container must contain Linux host tools and write access to the repository's
`build/` and `target/` directories. Podman example with Fedora:

```sh
podman run --rm -it \
  -v "$PWD:/src:Z" -w /src \
  registry.fedoraproject.org/fedora:40 bash
```

Inside the container:

```sh
dnf install -y make binutils nasm rust rust-src \
  qemu-system-x86-core grub2-tools-extra xorriso
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
make doctor
make test
```

Pin the container image and Rust version before reproducible release builds.
Container-only smoke tests need no host GUI.

## Build, graphical, and headless workflows

```sh
make doctor       # Verify host prerequisites.
make build        # Build build/pdc-os.kernel.
make              # Same build target.
make run          # Build ISO and open QEMU with its 512x512 framebuffer window.
make test         # Build ISO and run the baseline serial smoke test.
make benchmark    # Run smoke test and print Performance: metrics.
make clean        # Remove build/ and target/.
```

Interactive named profiles:

```sh
make run-baseline
make run-smp
make run-mem128
make run-mem256
make run-mem1g
make run-ata
make run-ps2
make run-serial
make run-vga
```

The serial-only profile emits to the terminal. `make test` captures COM1 in
`build/qemu-smoke.log` and terminates QEMU after `SMOKE_TIMEOUT` (10 seconds
by default).

## Boot phases and expected markers

The kernel reports these stages in order:

1. `PDC-OS boot stage: serial`
2. `PDC-OS boot stage: cpu`
3. `PDC-OS boot stage: memory`
4. `PDC-OS boot stage: scheduler`
5. `PDC-OS boot stage: interrupts`
6. `PDC-OS boot stage: filesystem`
7. `PDC-OS boot stage: userspace`
8. `PDC-OS boot stage: terminal`
9. `PDC-OS boot stage: complete`

The smoke test also requires hardware, display, memory, security, performance,
and completion lines. The last stage at watchdog expiry identifies the closest
active startup phase. A serial log should contain:

```text
PDC-OS boot: entering x86-64 kernel
PDC-OS kernel entry
Hardware: CPU vendor=
Hardware: interrupt-controller=legacy-PIC
Display mode: linear framebuffer (512x512), framebuffer enabled
PDC-OS boot complete
```

## Panic and exception interpretation

Panic and exception reports include boot stage, tick count, CR2/CR3, RFLAGS,
task count, interrupt state, memory state, and register/frame context where
available.

- `KERNEL PANIC:` - Rust panic or unrecoverable kernel stop.
- `KERNEL EXCEPTION:` - CPU exception or page fault.
- `PDC-OS initialization failure:` - typed bounded initialization failure.
- `PDC-OS halted:` - expected halt or failed reboot/shutdown fallback.
- `PDC-OS reboot requested:` - reboot control path requested.
- `PDC-OS shutdown requested:` - emulated ACPI poweroff requested.

CR2 is the faulting virtual address for page faults and CR3 is the active
page-table root. The preceding stage marker is the subsystem boundary at the
failure. Preserve and inspect the complete log:

```sh
cp build/qemu-smoke.log build/qemu-smoke.failure.log
make clean
make test SMOKE_TIMEOUT=20
```

Initialization failures use stable typed values: `timeout`, `not-present`,
`self-test-failed`, `invalid-response`, `busy`, or `unsupported`.

## Hardware support matrix

| Configuration | Status | Notes |
| --- | --- | --- |
| 1-vCPU, 256 MiB, PC, VGA, COM1 | Supported baseline | `make test-baseline`, `make run-baseline` |
| 128 MiB memory map | Supported | Memory-region shape test |
| 256 MiB memory map | Supported | Baseline memory shape |
| 1 GiB memory map | Supported | Allocator/memory-map test |
| PS/2 set-1 with VGA | Supported fallback | Keyboard and terminal event path |
| Legacy serial/headless | Supported diagnostics | VGA is not required for COM1 smoke logging |
| 512x512x32 framebuffer | Supported | Default display; VGA text fallback |
| Primary-master ATA PIO | Experimental | Raw 1 MiB QEMU image; no install workflow |
| 2-vCPU SMP | Experimental | Kernel and scheduler remain BSP-only |
| Framebuffer/GOP | Supported via GRUB Multiboot v1 | Kernel-owned display; no userspace access |
| Desktop GPU/network/Wi-Fi/USB | Unsupported | No production drivers or ABI |
| Real-disk installer | Unsupported | ATA is detected only; no partition workflow |
| UEFI direct boot | Unsupported | Use GRUB Multiboot for the supported path |

Physical hardware is not certified. Reset, shutdown, PS/2 behavior, ATA timing,
memory layout, and firmware maps must be treated as experimental outside QEMU.

## Safe recovery and rebuild

1. Stop interactive QEMU and preserve `build/qemu-smoke.log` or panic output.
2. Record the profile and Make-variable overrides.
3. Run `make doctor`.
4. Run `make clean && make test-baseline SMOKE_TIMEOUT=20`.
5. Test only the changed named profile after baseline recovery succeeds.
6. Rebuild the ISO instead of copying old files from `build/`.

The supported recovery path is a clean repository rebuild followed by serial
smoke testing. Do not replace boot files or disk images manually.

## Release, installation, and rollback

PDC-OS release bundles are reusable ISO distributions, not in-place installer
packages. The supported deployment target is the bootable ISO. Before using a
release, preserve the archive and verify it with:

```sh
cd build/release
sha256sum -c SHA256SUMS
```

The `version-manifest.txt` records the PDC-OS version, fixed source epoch,
Rust target, Rust/Cargo/NASM/binutils/GRUB/xorriso/QEMU versions, source
revision, implementation LOC, and smoke profile set. Offline reuse is available
through `pdc-os-<version>.tar.gz`.

To boot from release media:

1. Test the ISO in QEMU before writing removable or virtual media.
2. Boot it on the legacy-BIOS/GRUB path documented above.
3. Use the ISO's menu entry `PDC-OS`; the checked-in menu configuration is
   also copied in `boot/grub/grub.cfg`.

ATA installation and partition/filesystem formatting are intentionally
unsupported. The 1 MiB ATA image is only an experimental detection and
storage-driver test. Do not copy the ISO kernel directly onto a disk and expect
a bootable system.

Release verification workflow:

```sh
make release
make verify-release
cp build/release/pdc-os-*.tar.gz /secure/offline/media/
```

The verification target repeats a clean build and compares ELF bytes against
the preceding bundle, checks ISO structure, and reruns the baseline smoke
profile. The ELF comparison is byte-for-byte. GRUB/xorriso ISO UUID/catalog
metadata may differ between otherwise identical tool invocations; that builder
limitation is documented and the ISO is still boot-tested instead of marked
reproducible byte-for-byte. No release is a fallback for destructive disk
changes because PDC-OS has no disk installer; rollback means restoring or
rebooting the previous known-good ISO or media image. Unsupported hardware
remains listed in the hardware support matrix above.
