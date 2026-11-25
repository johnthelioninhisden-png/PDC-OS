# Boot PDC-OS in QEMU

This guide covers building the kernel, packaging it as a GRUB bootable ISO, and
running it in QEMU.

## Requirements

The project uses Rust nightly with `rust-src`, NASM, GRUB's rescue-image
builder, xorriso, and QEMU. The repository's `rust-toolchain.toml` selects the
Rust toolchain; the Makefile uses `cargo -Zbuild-std`.

On Fedora, install the system packages with:

```sh
sudo dnf install nasm qemu-system-x86-core grub2-tools-extra xorriso
```

On Debian or Ubuntu, the corresponding packages are:

```sh
sudo apt install nasm qemu-system-x86 xorriso grub-pc-bin
```

The Makefile accepts either `grub2-mkrescue` (Fedora) or `grub-mkrescue`
(Debian/Ubuntu). Override `GRUB_MKRESCUE` if the executable has a different
name or path, for example:

```sh
make test GRUB_MKRESCUE=/usr/bin/grub2-mkrescue
```

The Makefile checks common system paths for QEMU, `xorriso`, and GRUB before
`PATH`, avoiding same-named user-local Flatpak wrappers when the system tools
are installed. If a wrapper is the only available command and reports a
missing `flatpak-spawn`, install the tools in the build environment
(`sudo dnf install qemu-system-x86-core grub2-tools-extra xorriso` on Fedora),
then rebuild. A missing host launcher is not a kernel or ISO configuration
error.

Confirm the needed commands are available:

```sh
command -v cargo nasm qemu-system-x86_64 xorriso
command -v grub2-mkrescue || command -v grub-mkrescue
```

## Build and run with a display

From the repository root, build and launch the kernel ISO:

```sh
make run
```

The ISO is written to `build/pdc-os.iso`. QEMU opens a 512x512 guest display
window and GRUB
immediately boots the default `PDC-OS` entry (`timeout=0`); there is no menu
selection step. Once startup finishes, use the kernel's framebuffer terminal.
The shell supports
keyboard input, process, memory, filesystem, performance, security, and
power/recovery commands.

`make run` uses QEMU's default display, so it needs a graphical desktop session.
If you are on a headless machine or only need to confirm that the kernel boots,
use the smoke test below instead.

To build without launching QEMU:

```sh
make build
```

## Run the headless boot smoke test

The smoke test boots the ISO without a display and checks that kernel startup
milestones appear on the serial console:

```sh
make test
```

The log is saved to `build/qemu-smoke.log`. Adjust the observation time or QEMU
executable as needed:

```sh
make test SMOKE_TIMEOUT=20
make test QEMU=/path/to/qemu-system-x86_64
```

The smoke test terminates QEMU after the configured interval. A successful
test prints `QEMU boot smoke test passed`.

## Hardware profiles

Run a named smoke profile directly or through the Make target:

```sh
make test-baseline
make test-smp
make test-mem128
make test-mem256
make test-mem1g
make test-ata
make test-ps2
make test-serial
make test-vga
```

The matrix is:

- **baseline**: 256 MiB RAM, 1 vCPU, standard PC machine, 512x512 framebuffer, COM1.
- **smp**: 256 MiB RAM and 2 vCPUs; experimental because PDC-OS scheduling is currently BSP-only.
- **mem128/mem256/mem1g**: memory-map shape and allocator boot coverage.
- **ata**: a temporary 1 MiB raw IDE image for primary-master ATA PIO detection.
- **ps2**: minimal PC devices with VGA and i8042; keyboard initialization must remain a fallback-safe path.
- **serial**: minimal headless PC with COM1 diagnostics.
- **vga**: explicit standard VGA adapter with framebuffer output.

Boot diagnostics include CPU vendor and required feature checks, memory-map
region counts, legacy-PIC selection, PS/2 initialization, COM1 availability,
and ATA detection. Unsupported or absent devices are reported as fallbacks,
not silently assumed to be ready.

## Troubleshooting

- **GRUB rescue builder is missing**: install the GRUB tools package. The
  Makefile accepts either `grub2-mkrescue` or `grub-mkrescue`.
- **`A working xorriso executable is required`**: install xorriso in the build
  environment (`sudo dnf install xorriso` on Fedora). A broken user-local
  Flatpak shim may appear to exist on `PATH` but cannot create the ISO.
- **`QEMU executable not found`**: install the x86-64 system emulator or set
  `QEMU` to its executable path.
- **Smoke test reports a missing boot marker**: inspect
  `build/qemu-smoke.log`. It contains the output received from COM1 and should
  help distinguish a bootloader problem from an early kernel failure.
- **QEMU cannot open a display**: use `make test` for a headless boot check, or
  run `make run` from a graphical desktop session.

To rebuild generated artifacts from scratch:

```sh
make clean
make test
```
