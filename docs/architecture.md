# PDC-OS architecture baseline

PDC-OS is a freestanding, modular monolithic kernel.  The x86-64 implementation
is the initial architecture backend; architecture-neutral kernel code depends
on HAL traits and must not manipulate CPU-specific registers or I/O ports.

## Repository layout

- `arch/x86_64/{boot,interrupts,paging,context,syscall,cpu}` contains
  architecture-specific x86-64 sources and assembly boundaries.
- `src/arch/` contains architecture backends.
- `src/hal/` contains architecture-independent hardware contracts.
- `src/kernel/` contains core, memory, scheduler, interrupt, recovery,
  performance, and security subsystems.
- `src/userspace/` contains shell, terminal, syscall, service, and program
  modules.
- `src/drivers/` and `src/fs/` contain device and filesystem implementations.

The compatibility modules `hardware` and `os` are aliases or re-exports while
callers migrate to `arch::x86_64` and `userspace`.  New kernel code must use
the canonical modules; compatibility names are migration surfaces, not new
implementation locations.

## Kernel contracts

- `Cpu` describes boot-time CPU capabilities.
- `AddressSpaceBackend` owns address-space create, map, unmap, and activation
  operations.
- `TlbInvalidation` provides architecture-specific TLB behavior.
- `CpuMemoryProtection` provides NX and supervisor write-protection setup.
- `SubsystemLifecycle` and `KernelError` provide initialization and error
  boundaries for kernel subsystems.

## Interface and ABI versioning

Kernel interfaces use the `InterfaceVersion` triplet:

- major: incompatible ABI or contract change;
- minor: backward-compatible addition;
- patch: implementation correction with no interface change.

The current baseline is `0.2.0`.  The int 0x80 syscall ABI is `0.2.0` and
syscall numbers are stable for the lifetime of major version 0.  New calls
must be appended rather than renumbered.  Removal requires a major-version
change or a clearly documented compatibility alias.

Syscall dispatch accepts RAX as the number and RBX/RCX/RDX as arguments.
Success is a non-negative value or call-defined count.  Failures are negative
errno values.  Pointer-shaped arguments must be checked for overflow and the
canonical userspace range before dereferencing.  The current implementation
is an in-kernel prototype, reached while still running in a protected kernel
context; no Ring 3 entry transition or per-process untrusted page-table policy
is established by this ABI yet.  Some calls (for example ELF execution and
ordinary file descriptors) remain partially implemented or use fixed kernel
services and are documented in [syscalls.rs](/home/valerious/PDC-OS/src/userspace/syscalls.rs).

## Stage 8 security baseline

The x86-64 backend enables the NX execute-disable feature through EFER.NXE
when CPUID reports support and enables CR0 write protection with a control-
register readback. The leaf-mapping policy applies PAGE_NX to read-only and
read-write mappings and leaves it clear only for explicit executable mappings.
Public mapping APIs reject misaligned addresses, null and out-of-domain
physical frames, cross-domain user/kernel flags, and executable-writable
combinations.

Pointer-shaped ABI arguments use checked range endpoints and the canonical
userspace region. ELF parsing checks table offsets, segment arithmetic,
page-end rounding, file ranges, and copy lengths before creating mappings.
The kernel heap rejects deallocation pointers outside the fixed heap and
layouts whose allocation end would overflow.

Unsafe Rust boundaries remain explicit and require the following invariants:

- Raw Multiboot and page-table pointers are bounded to the 4 GiB physical
  allocator domain or checked against a bootloader-provided table length.
- Page-table frames are allocator-owned, 4 KiB aligned, and initialized before
  a raw pointer is written or traversed.
- Heap allocation and deallocation operate only on the fixed static heap and
  must preserve `FreeBlock` metadata and block alignment.
- Scheduler and interrupt context copies cover a fixed 22-word register layout.
- Syscall slices and strings are formed only after canonical range/length
  validation; Ring 3 is not implemented, so the kernel cannot yet enforce
  process-specific mapping permissions or fault on an unmapped user page.
- Port I/O, CPUID, MSRs, control registers, TSS, and IDT assembly operations
  are limited to the x86-64 architecture backend or explicitly audited device
  drivers.

These boundaries document the current privilege-isolation limitations: PDC-OS
still runs the prototype syscall and shell in protected kernel context. A real
Ring 3 process entry, per-process user-page lookup, SMAP/SMEP policy where
available, userspace access faults, and audited capability transitions are
required before arbitrary userspace code can be considered isolated.

## Ownership

Physical frames are allocated and reclaimed by the frame bitmap. A mapping
references a physical frame but does not own it. An address space owns the PML4
and private page tables created for it. Mappings and address-space roots are
accounted explicitly and cleaned recursively.

## x86-64 virtual layout

- User mappings begin at `0x0000000000001000`.
- The user address-space boundary is `0x0000800000000000`.
- Kernel mappings begin at `0xffff800000000000`.
- The null page remains unavailable.
- 4 KiB pages are the mapping and ownership unit.

## Subsystem initialization

Initialization follows the dependency order documented in
`subsystem-contracts.md`: serial/recovery, CPU and firmware discovery,
display discovery, memory, scheduler, services, filesystem, userspace,
interrupts, and terminal.

Optional hardware failures are subsystem/device failures.  They must not be
converted into silent kernel success or into failure of unrelated subsystems.

## Stage 10 migration baseline

The architectural migration is complete for the PDC-OS 0.2.0 development
baseline.  Source ownership is organized as follows:

- `src/arch/` is the architecture-backend boundary.
- `src/hal/` owns architecture-neutral hardware contracts.
- `src/kernel/` owns generic kernel subsystems.
- `src/userspace/` owns shell, terminal, syscall, service, and program code.
- `src/drivers/` owns device discovery, lifecycle, and device implementations.
- `src/fs/` owns filesystem and mount implementation.

Existing boot, shell, diagnostic, and smoke behavior remains in place through
the canonical modules.  `hardware` and `os` remain compatibility aliases during
external-source migration.  Publishing/releasing the final development baseline
is intentionally a user-controlled repository action.

## Stage 11-15 baseline specifications

- The subsystem dependency map is documented in
  [subsystem-dependencies.md](/home/valerious/PDC-OS/docs/subsystem-dependencies.md).
- Resource ownership and cleanup responsibility are documented in
  [ownership-matrix.md](/home/valerious/PDC-OS/docs/ownership-matrix.md).
- Lock order, IRQ safety, blocking, wakeup, and scheduling interactions are
  defined in
  [synchronization-specification.md](/home/valerious/PDC-OS/docs/synchronization-specification.md).
- Architecture backend expectations are defined in
  [hal-contract.md](/home/valerious/PDC-OS/docs/hal-contract.md).
- Allocation exhaustion, malformed boot information, invalid mappings, partial
  address-space/process cleanup, wait-queue exhaustion, and repeated cleanup
  are exercised by the boot verification suite in
  [verification.rs](/home/valerious/PDC-OS/src/kernel/verification.rs).

## Open architectural decisions

The baseline leaves these items explicitly outside 0.2.0 scope:

- Ring 3 entry and per-process privilege isolation.
- SMP scheduler ownership and per-CPU context migration.
- Demand-paged userspace mappings and fault-driven allocator policy.
- Persistent disk installation and partition/filesystem tooling.
- Network, USB, and desktop-GPU production support.
- Alternate architecture backends beyond the HAL contracts.

These decisions are known limitations, not silently implied supported
features.  Protected execution work belongs to the 0.3.0+ roadmap.
