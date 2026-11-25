# PDC-OS architecture baseline

PDC-OS is a freestanding, modular monolithic kernel.  The x86-64 implementation
is the initial architecture backend; architecture-neutral kernel code depends
on HAL traits and must not manipulate CPU-specific registers or I/O ports.

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
