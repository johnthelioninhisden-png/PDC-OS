# PDC-OS

PDC-OS is a freestanding 64-bit monolithic kernel for x86-64.
Its bootstrap is NASM assembly, while kernel subsystems are Rust.

## Layout

- `arch/x86_64/` - 64-bit boot, GDT, IDT, and interrupt assembly
- `src/arch/` - architecture backends behind kernel contracts
- `src/hal.rs` - architecture-independent CPU, paging, interrupt, timer, and driver interfaces
- `src/kernel.rs` - kernel-wide error and invariant contracts
- `docs/` - subsystem, memory-layout, ownership, and dependency contracts
- `docs/architecture.md` - Stage 1 and Stage 2 architecture baseline
- `src/` - Rust kernel core
- `src/memory.rs` - physical frame allocator
- `src/memory.rs` - reclaimable frames, kernel heap, page tables, and address spaces
- `src/scheduler.rs` - preemptive round-robin task scheduler
- `src/interrupts.rs` - IDT, exception stubs, PIC/PIT, and IRQ-safe locks
- `src/drivers.rs` - reusable PIT, VGA, ATA PIO, and port I/O device primitives,
  including the kernel-owned 512x512 framebuffer console and VGA text fallback
- `src/fs.rs` - virtual filesystem, mount table, RAM filesystem, and ATA detection
- `src/serial.rs` - COM1 diagnostic output
- `src/os/` - terminal, PS/2 keyboard, syscalls boundary, and built-in commands
- `src/os/services.rs` - IPC, signals, shared resources, identities, and kernel-side permissions
- `src/os/userspace.rs` - userspace shell/process startup, core program registry, and package installation
- `linker.ld` - kernel memory layout
- `Makefile` - build and run commands

## Build

Check the host toolchain and emulator prerequisites first:

```sh
make doctor
```

```sh
make
```

The resulting ELF image is `build/pdc-os.kernel`.

## Release artifacts

Create a clean, offline reusable release bundle:

```sh
make release
```

The bundle is written to `build/release/` and contains the ELF, ISO, boot
configuration, SHA-256 checksums, version/toolchain manifest, every named
profile's serial smoke log, and `pdc-os-<version>.tar.gz`.

The package version is read from `Cargo.toml`; override it for a release build
with `make release VERSION=x.y.z`. `SOURCE_DATE_EPOCH` defaults to a fixed
epoch for repeatable metadata and ISO source mtimes. The build/ISO tool output
is recorded in the manifest, and the source revision is recorded when Git is
available.

Verify artifact checksums and boot completion, then repeat a clean build and
compare ELF/ISO bytes:

```sh
make verify-release
```

Both targets run the named hardware smoke suite. `make verify-release` also rebuilds from clean sources and reports ELF mismatch
or an invalid/failed-boot ISO instead of silently publishing it. ELF artifacts
are compared byte-for-byte. ISO volume UUID/catalog metadata may vary in
GRUB/xorriso releases, so ISO verification additionally checks structure and
boot smoke behavior and records the builder versions in the manifest.

## Run

On Fedora, install the emulator and GRUB ISO tools:

```sh
sudo dnf install qemu-system-x86-core grub2-tools-extra xorriso
```

The Makefile prefers installed system GRUB builders over same-named `PATH`
wrappers. If the builder has a non-standard name or path, set `GRUB_MKRESCUE`,
for example `make run GRUB_MKRESCUE=/usr/bin/grub2-mkrescue`.
For supported hardware, serial/headless and graphical workflows, troubleshooting,
and container setup, see [Notes/operations.md](Notes/operations.md).
Then build and boot the kernel in QEMU:

```sh
make run
```

`make run` packages the Multiboot kernel in a bootable ISO before starting QEMU
with a 512x512 guest framebuffer in a window.

## Test and diagnose boot

Run the headless QEMU smoke test to build the ISO, boot it for a bounded
interval, and verify serial-log markers for each major initialization stage:

```sh
make test
```

The test requires `qemu-system-x86_64`, either `grub2-mkrescue` or
`grub-mkrescue`, `xorriso`, and the standard `timeout` command. See
[Notes/qemu-boot.md](Notes/qemu-boot.md) for installation, boot, and
troubleshooting instructions.
Override `QEMU` to select another QEMU executable, `GRUB_MKRESCUE` to select
the ISO builder, or `SMOKE_TIMEOUT` to change the boot observation interval,
for example `make test SMOKE_TIMEOUT=20`.
The captured COM1 output is written to `build/qemu-smoke.log`. Early bootstrap
failures, kernel exceptions, and panics are also reported on COM1.

### Security and reliability review

The 10.7 review adds bounded adversarial checks for unknown syscalls, invalid
pointer ranges, malformed ELF inputs, VFS traversal, oversized writes, mount
validation, and credential boundaries. The checks run during boot and are
repeatable with the `security` shell command. A booted system reports a
`Security review: 10.7` serial marker; failures are counted rather than
silently continued.

### Performance diagnostics

The `perf` shell command reports scheduler context-switch counts and TSC
latencies, IRQ wait samples, syscall latency, fixed disk-cache hit/miss counts,
serial/VGA output timing, frame-allocation churn, reclaim counts, page-table
switches, and kernel-heap fragmentation. `bench` runs a bounded 32-read
RAM/filesystem workload. Both commands are safe diagnostic operations and do
not alter the fixed in-kernel command set.

`make benchmark` repeats the baseline QEMU smoke test and prints the boot-time
`Performance:` line from COM1. Conservative baseline targets are:

| Metric | Baseline target |
| --- | --- |
| Boot to complete marker | Within 10 seconds in `make test-baseline` |
| Repeated context switch | No unbounded loop or allocation in the switch path |
| Physical frame churn | Equal or fewer reclaim failures across repeated `spawn/exit` tests |
| Disk cache | Repeated file reads should produce cache hits |
| Console | 80-column framebuffer output remains bounded and prompt-preserving |
| Soak stability | No panic/exception markers after repeated file, keyboard, and uptime operations |

The TSC values are cycle counts, not portable wall-clock seconds. QEMU's
baseline serial line is the before/after source of truth; capture it with
`make benchmark` and compare it across changes. The first complete measurements
are intentionally regression observations rather than fabricated percentages.

### Terminal

The 512x512 framebuffer terminal uses a dark background with color-highlighted
command names, directory entries, the prompt, and errors. Its compact bitmap
font retains the familiar 80-column layout. Bounded scrollback retains 512
lines; use Page Up/Page Down to browse and return to the live prompt. Backspace
edits an unfinished command, and Up/Down recall recent commands. `history`
lists them, and Ctrl+L clears the screen.
`head FILE` prints up to ten lines and `wc FILE` reports newline count and bytes.
Relative file paths resolve from `/`.

### Hardware profiles

The following named profiles use deterministic QEMU devices and arguments:

| Profile | Make smoke target | Status |
| --- | --- | --- |
| `baseline` | `make test-baseline` | Supported |
| `smp` (2 vCPU) | `make test-smp` | Experimental: kernel remains single-threaded |
| `mem128` | `make test-mem128` | Supported |
| `mem256` | `make test-mem256` | Supported |
| `mem1g` | `make test-mem1g` | Supported |
| `ata` | `make test-ata` | Experimental: primary-master ATA PIO |
| `ps2` | `make test-ps2` | Supported PS/2 input fallback |
| `serial` | `make test-serial` | Headless COM1-only diagnostics |
| `vga` | `make test-vga` | Supported VGA framebuffer output |

Interactive counterparts are `make run-baseline`, `make run-smp`,
`make run-mem128`, `make run-mem256`, `make run-mem1g`, `make run-ata`,
`make run-ps2`, `make run-serial`, and `make run-vga`. QEMU profiles report CPU
vendor/features, memory-map parsing, legacy-PIC use, PS/2 status, and ATA
mount status on COM1. The kernel requests a 512x512x32 RGB framebuffer from
GRUB and falls back to VGA text if the bootloader cannot provide it.

## Display support

The default display is a 512x512x32 RGB linear framebuffer requested through
the Multiboot v1 header. The kernel renders its 80x25 text console with a
compact bitmap font; the shell's `display` command reports the active mode.
If GRUB cannot provide the requested framebuffer, the kernel falls back to
legacy VGA text mode. Display operations remain in the kernel driver and are
not exposed through the userspace syscall ABI.
