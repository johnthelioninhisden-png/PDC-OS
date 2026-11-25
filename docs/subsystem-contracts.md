# PDC-OS subsystem contracts

## Initialization order

Subsystem initialization is dependency-ordered:

1. serial and recovery
2. CPU/firmware discovery
3. display discovery
4. physical memory
5. scheduler
6. services and process tables
7. filesystem
8. userspace
9. interrupts and timer
10. terminal

A subsystem must publish no externally visible state before its prerequisite
subsystems are initialized. Optional devices are failures of the specific
device, not of the kernel, and use `recovery::InitError`.

## Ownership

- Physical frames are owned by the physical frame bitmap.
- Mapping references do not own physical frames.
- An address-space root owns the page tables it creates.
- A task owns only the scheduler slot and private kernel stack identified in
  the task metadata.
- Open files and shared memory have explicit PID ownership.
- Drivers own the device resources they acquire and must release them before
  deregistration.

## Error model

Generic kernel code uses `kernel::KernelError`; hardware initialization uses
`recovery::InitError` and reports a stable short name. Pointer, range, and
arithmetic failures are separate validation failures rather than panics.

## Architecture and HAL boundary

Architecture-independent code must call `hal::Cpu`,
`hal::AddressSpaceBackend`, `hal::TlbInvalidation`, and
`hal::CpuMemoryProtection` rather than reading CPU registers or executing
architecture-specific instructions. The x86-64 backend is implemented in
`src/arch/x86_64.rs`; future architecture backends implement the same traits.
