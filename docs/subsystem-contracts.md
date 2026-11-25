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
- A process owns its address-space ID and all threads created for that process.
- A thread owns only its scheduler/context slot plus private kernel-stack and
  userspace-stack frames identified in the thread metadata.
- Generic thread-context storage is separate from x86-64 IRET-frame construction.
- Interrupt handlers receive a formal 22-slot interrupt frame.  Vector 8 uses
  TSS IST1; normal IRQ and exception dispatches publish bounded per-CPU
  interrupt ownership before leaving the interrupt context.
- Process and thread state transitions are explicit; destructive cleanup is
  performed by `destroy_thread` and `destroy_process`, never by scheduling code.
- Open files and shared memory have explicit PID ownership.
- Drivers own the device resources they acquire and must release them before
  deregistration.

## Driver lifecycle and discovery

- `DeviceDescriptor` is discovery-only metadata: device name, class, and
  claimed port, IRQ, or memory resources.
- Discovery publishes `Discovered`; a driver implementation then transitions
  the device through `Initialized` and `Active`. `remove` must revoke resources
  and interrupt registrations before publishing `Removed`.
- `Device::shutdown` is the removal contract for driver implementations.
- Resource claims are fixed-size registry data and overlap is rejected before
  initialization. A resource cannot be acquired twice by two devices.
- Interrupt work registers an IRQ vector, PIC line, sharing mode, and optional
  typed handler with `DEVICE_REGISTRY`. Exclusive IRQ vectors reject duplicate
  registrations; unregistering is owner-and-vector scoped.

## Synchronization contracts

- `IrqLock<T>` protects short, IRQ-sensitive critical sections. Acquisition
  disables IRQs and the guard restores the exact prior interrupt state.
- `try_lock` is the non-blocking form for IRQ-context work. Its `None`
  result means the lock remains owned by another execution context.
- Spin locks must not perform allocation, wake tasks, enter blocking code,
  or retain a guard across an unrelated subsystem call.
- A blocking operation is scheduler work: mark the task `Blocked`, queue it
  behind a stable wait reason, release every lock, and yield. Wakeup first
  removes the wait-queue entry, then transitions the task to `Ready`.
- Wakeups do not acquire spin locks; they publish only the identity of the
  runnable task and leave the scheduler lock to own the state transition.

## Lock inventory and order

At boot and in normal kernel code, the observed dependency order is:

1. No nested ownership may be acquired in a direction opposite to a sibling
   lock. Public device, process, thread, and scheduler locks are acquired only
   over bounded table operations.
2. Device registry, filesystem, process/thread, scheduler, and terminal tables
   use `IrqLock`; they are leaf data-structure locks.
3. Process/thread cleanup uses `threads -> processes`; process/address-space
   replacement drops the process guard before destroying address-space tables.
4. Memory frame/heap locks are allocator locks and must not call process,
   scheduler, filesystem, or terminal APIs while held.
5. The VGA and serial output locks are output sinks and must not call back into
   a subsystem that may take the output lock.

Current known lock inventory:

| Lock | Scope |
| --- | --- |
| `WRITER` | VGA text and scrollback |
| `SERIAL_LOCK` | COM1 output |
| `ALLOCATOR_LOCK` | Frame bitmap operations |
| `HEAP_LOCK` | Kernel heap |
| `PROCESS_TABLE` process/thread locks | Process ownership tables |
| `SCHEDULER` | Task/context table |
| `DEVICE_REGISTRY` | Device lifecycle table |
| `VFS` | Filesystem mount and node table |
| `KEY_QUEUE`, `MODIFIERS` | Keyboard state |
| `OPEN_HANDLES`, `PROCESSES`, `NEXT_FD` | Syscall userspace state |

## Error model

Generic kernel code uses `kernel::KernelError` and `kernel::ApiResult<T>`.
The enum distinguishes invalid arguments, unavailable resources, exhaustion,
invalid state, missing resources, permission faults, pointer faults,
unsupported operations, and hardware initialization failures.

Every error has a stable `name()` for diagnostics and `errno()` for the
negative-value syscall ABI.  Subsystem conversion is explicit: filesystem
`FsError` values convert through `From<FsError> for KernelError`, while
`InitError` converts through `From<InitError> for KernelError`.  Pointer,
range, and arithmetic failures are validation failures, never implicit panics.

## Architecture and HAL boundary

Architecture-independent code must call `hal::Cpu`,
`hal::AddressSpaceBackend`, `hal::TlbInvalidation`, and
`hal::CpuMemoryProtection` rather than reading CPU registers or executing
architecture-specific instructions. The x86-64 backend is implemented in
`src/arch/x86_64/`; future architecture backends implement the same traits.
