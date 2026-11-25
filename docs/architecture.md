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
callers migrate to `arch::x86_64` and `userspace`.

## Kernel contracts

- `Cpu` describes boot-time CPU capabilities.
- `AddressSpaceBackend` owns address-space create, map, unmap, and activation
  operations.
- `TlbInvalidation` provides architecture-specific TLB behavior.
- `CpuMemoryProtection` provides NX and supervisor write-protection setup.
- `SubsystemLifecycle` and `KernelError` provide initialization and error
  boundaries for kernel subsystems.

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
