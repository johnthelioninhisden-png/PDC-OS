RUST_TARGET := x86_64-unknown-none
# Cargo's release archive is linked with the assembly bootstrap and interrupt stubs.
BUILD_DIR := target/$(RUST_TARGET)/release
KERNEL_ELF := build/pdc-os.kernel
GRUB_CFG := boot/grub/grub.cfg
ISO_ROOT := build/iso-root
KERNEL_ISO := build/pdc-os.iso
# Prefer an installed system emulator over same-named PATH wrappers.
QEMU ?= $(shell \
	for tool in /usr/bin/qemu-system-x86_64 /usr/local/bin/qemu-system-x86_64; do \
		if test -x "$$tool"; then printf '%s' "$$tool"; exit 0; fi; \
	done; \
	command -v qemu-system-x86_64 2>/dev/null || printf '%s' qemu-system-x86_64)
# Prefer an installed system xorriso over same-named PATH wrappers.
XORRISO ?= $(shell \
	for tool in /usr/bin/xorriso /usr/local/bin/xorriso; do \
		if test -x "$$tool"; then printf '%s' "$$tool"; exit 0; fi; \
	done; \
	command -v xorriso 2>/dev/null || printf '%s' xorriso)
# Prefer installed system builders over PATH wrappers (for example, a broken
# ~/.local/bin Flatpak launcher); GRUB_MKRESCUE remains explicitly overridable.
GRUB_MKRESCUE ?= $(shell \
	for builder in /usr/bin/grub2-mkrescue /usr/sbin/grub2-mkrescue \
		/usr/bin/grub-mkrescue /usr/sbin/grub-mkrescue; do \
		if test -x "$$builder"; then printf '%s' "$$builder"; exit 0; fi; \
	done; \
	command -v grub2-mkrescue 2>/dev/null || \
	command -v grub-mkrescue 2>/dev/null || printf '%s' grub2-mkrescue)
SMOKE_TIMEOUT ?= 10
PROFILE ?= baseline
VERSION ?= $(shell sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n1)
SOURCE_DATE_EPOCH ?= 946684800
RELEASE_DIR := build/release

# Fixed locale/time settings keep generated boot metadata and text output
# independent of the developer's shell environment.
export LC_ALL=C
export TZ=UTC
export SOURCE_DATE_EPOCH

.PHONY: all build run run-baseline run-smp run-mem128 run-mem256 run-mem1g \
	run-ata run-ps2 run-serial run-vga \
	test test-baseline test-smp test-mem128 test-mem256 test-mem1g \
	test-ata test-ps2 test-serial test-vga smoke-test benchmark bench \
	release verify-release doctor clean

# Default target builds the bootable kernel image.
all: build

# Report missing host prerequisites before a build or smoke test obscures the
# first failure.  The check is intentionally read-only.
doctor:
	./scripts/check-prerequisites.sh "$(GRUB_MKRESCUE)" "$(QEMU)" "$(XORRISO)"

# Build each kernel component, then link them into the Multiboot ELF image.
build:
	mkdir -p build
	cargo build --release --target $(RUST_TARGET) -Zbuild-std=core,compiler_builtins
	nasm -f elf64 -o build/boot.o arch/x86_64/boot.asm
	nasm -f elf64 -o build/interrupts.o arch/x86_64/interrupts.asm
	ld -m elf_x86_64 -z max-page-size=0x1000 -T linker.ld -o $(KERNEL_ELF) build/boot.o build/interrupts.o $(BUILD_DIR)/libpdc_os.a

$(KERNEL_ISO): build $(GRUB_CFG)
# Assemble the GRUB ISO tree from the kernel image and boot menu configuration.
	@"$(XORRISO)" -version >/dev/null || { \
		echo "A working xorriso executable is required to build the boot ISO." >&2; \
		echo "Install it in this environment (Fedora: sudo dnf install xorriso; Debian/Ubuntu: sudo apt install xorriso)." >&2; \
		exit 1; \
	}
	mkdir -p $(ISO_ROOT)/boot/grub
	cp $(KERNEL_ELF) $(ISO_ROOT)/boot/pdc-os.kernel
	cp $(GRUB_CFG) $(ISO_ROOT)/boot/grub/grub.cfg
	find $(ISO_ROOT) -exec touch -h -d "@$(SOURCE_DATE_EPOCH)" {} +
	@command -v "$(GRUB_MKRESCUE)" >/dev/null || { \
		echo "GRUB ISO builder not found: $(GRUB_MKRESCUE)" >&2; \
		echo "Install GRUB's ISO tools (Fedora: sudo dnf install grub2-tools-extra; Debian/Ubuntu: sudo apt install grub-pc-bin), or set GRUB_MKRESCUE to the executable path." >&2; \
		exit 1; \
	}
	@if ! PATH="$(dir $(XORRISO)):$$PATH" "$(GRUB_MKRESCUE)" \
			--xorriso="$(XORRISO)" -o $@ $(ISO_ROOT) -- \
			-iso_nowtime "2000010100000000" \
			-volume_date uuid "2000010100000000"; then \
		echo "GRUB ISO creation failed using $(GRUB_MKRESCUE)." >&2; \
		echo "If this is a host/Flatpak shim, install GRUB ISO tools in this environment or set GRUB_MKRESCUE=/usr/bin/grub2-mkrescue (or the installed grub-mkrescue path)." >&2; \
		exit 1; \
	fi

run: $(KERNEL_ISO)
# Run only when QEMU is installed; report a clear error otherwise.
	@if command -v "$(QEMU)" >/dev/null; then \
		"$(QEMU)" -m 256M -cdrom $(KERNEL_ISO) \
			-display gtk,zoom-to-fit=on; \
	else \
		echo "QEMU executable not found: $(QEMU)"; exit 1; \
	fi

# Named QEMU hardware profiles. SMP is an emulated-vCPU boot profile; PDC-OS
# currently remains single-threaded and reports this in the support matrix.
run-baseline: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" baseline
run-smp: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" smp
run-mem128: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" mem128
run-mem256: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" mem256
run-mem1g: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" mem1g
run-ata: $(KERNEL_ISO)
	mkdir -p build
	truncate -s 1M build/qemu-ata.img
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" ata
run-ps2: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" ps2
run-serial: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" serial
run-vga: $(KERNEL_ISO)
	./scripts/qemu-profile.sh "$(KERNEL_ISO)" "$(QEMU)" vga

test: smoke-test

smoke-test: $(KERNEL_ISO)
	./scripts/qemu-smoke-test.sh "$(KERNEL_ISO)" "build/qemu-smoke.log" "$(QEMU)" "$(SMOKE_TIMEOUT)" "$(PROFILE)"

test-baseline:
	$(MAKE) smoke-test PROFILE=baseline
test-smp:
	$(MAKE) smoke-test PROFILE=smp
test-mem128:
	$(MAKE) smoke-test PROFILE=mem128
test-mem256:
	$(MAKE) smoke-test PROFILE=mem256
test-mem1g:
	$(MAKE) smoke-test PROFILE=mem1g
test-ata:
	mkdir -p build
	truncate -s 1M build/qemu-ata.img
	PROFILE=ata $(MAKE) smoke-test
test-ps2:
	$(MAKE) smoke-test PROFILE=ps2
test-serial:
	$(MAKE) smoke-test PROFILE=serial
test-vga:
	$(MAKE) smoke-test PROFILE=vga

benchmark bench: smoke-test
	@grep -F "Performance:" build/qemu-smoke.log

# Produce an offline release bundle after a clean build and every named
# hardware smoke profile.  The SOURCE_DATE_EPOCH default is deliberately
# fixed; override it only when the caller has a documented source epoch.
release: doctor
	@rm -rf build target build/release
	$(MAKE) build
	$(MAKE) $(KERNEL_ISO)
	./scripts/build-release.sh "$(VERSION)" "$(KERNEL_ELF)" "$(KERNEL_ISO)" \
		"$(GRUB_CFG)" "$(RELEASE_DIR)" "$(QEMU)" "$(GRUB_MKRESCUE)" \
		"$(XORRISO)" "$(SMOKE_TIMEOUT)"

# Rebuild the release in a second clean output directory and compare the ELF
# byte-for-byte.  GRUB/xorriso may vary boot-media UUID or catalog metadata
# across invocations, so the ISO is structurally checked and boot-smoke-tested.
verify-release: release
	@tmp="/tmp/pdc-os-release-verify-$$$$"; mkdir -p "$$tmp"; \
	cp "$(RELEASE_DIR)/pdc-os.kernel" "$$tmp/pdc-os.kernel"; \
	cp "$(RELEASE_DIR)/pdc-os.iso" "$$tmp/pdc-os.iso"; \
	./scripts/verify-release.sh "$(RELEASE_DIR)"; \
	$(MAKE) clean; \
	$(MAKE) build; \
	$(MAKE) $(KERNEL_ISO); \
	cmp "$$tmp/pdc-os.kernel" "$(KERNEL_ELF)" || { echo "ELF release reproducibility mismatch" >&2; exit 1; }; \
	"$(XORRISO)" -indev "$(KERNEL_ISO)" -toc >/dev/null 2>&1 || { echo "ISO structure verification failed" >&2; exit 1; }; \
	./scripts/qemu-smoke-test.sh "$(KERNEL_ISO)" "$$tmp/qemu-smoke.log" "$(QEMU)" "$(SMOKE_TIMEOUT)" "$(PROFILE)"; \
	rm -f "$$tmp/pdc-os.kernel" "$$tmp/pdc-os.iso" "$$tmp/qemu-smoke.log"; rmdir "$$tmp"; \
	$(MAKE) release; \
	echo "PDC-OS ELF reproducibility and ISO structural/smoke verification passed"

clean:
# Build outputs are confined to these generated directories.
	rm -rf build target
