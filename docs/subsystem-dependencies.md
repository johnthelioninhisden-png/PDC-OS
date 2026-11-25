# Subsystem dependencies

This map is a directed initialization and runtime dependency declaration.
An edge means the dependent subsystem may use the dependency's published
interface.  It does not imply nested ownership or a requirement to run in the
dependency's lock context.

| Subsystem | Direct dependencies | Initialization responsibility |
| --- | --- | --- |
| serial/recovery | x86 COM1 assembly boundary | No normal-subsystem dependencies; first boot stage |
| CPU/HAL | serial, recovery | CPU discovery, NX/WP setup, and architecture backend readiness |
| display/drivers | CPU/HAL, serial | Device discovery and display fallback |
| memory | boot memory map, HAL CPU/address-space traits | Physical frames, heap, and address-space roots |
| process | memory, process table synchronization | Process metadata and thread-owned stacks |
| scheduler | memory, process/thread metadata, per-CPU data | Runnable task ownership and context selection |
| interrupts | memory, HAL interrupt controller/timer, scheduler | IDT/PIC/PIT setup and bounded dispatch |
| device registry | serial/recovery, IRQ/interrupt contracts | Typed discovery, resources, lifecycle, and removal |
| filesystem | memory, VFS mount/table locks | Fixed-size nodes, cache, and VFS operations |
| services | process tables, interrupt-safe locks | Kernel service process and handle state |
| userspace/syscalls | process, memory, filesystem, terminal | ABI validation and in-kernel syscall dispatch |
| terminal | display, keyboard, interrupt-safe locks | Input queue and shell output |

## Runtime call-direction rules

- Architecture-neutral scheduler, process, syscall, filesystem, and security
  code may call HAL traits, but may not directly issue x86 ports or CPU-control
  instructions.
- Memory allocation and page-table code may not call scheduler, filesystem,
  process, terminal, or service APIs while an allocator lock is held.
- Interrupt dispatch may notify or queue work, but must not allocate, block,
  or synchronously invoke an interactive terminal or filesystem operation.
- Process cleanup owns thread cleanup first, then address-space cleanup.
- Scheduler wakeup publishes runnable task identity; the scheduler owns state
  transition and wait-queue removal.
- Device removal revokes resources and interrupts before the registry marks the
  device removed.
- Userspace dispatch validates pointer ranges and syscall state before calling
  filesystem or process operations.

Hardware-specific code must not be called from architecture-independent
scheduler, process, syscall, or filesystem implementation.

## Kernel-facing interface boundaries

| Interface | Owner | Contract |
| --- | --- | --- |
| `hal::Cpu` | architecture backend | CPU feature discovery |
| `hal::AddressSpaceBackend` | architecture backend | create, map, unmap, and activate address spaces |
| `hal::TlbInvalidation` | architecture backend | page/address-space invalidation |
| `hal::CpuMemoryProtection` | architecture backend | NX and supervisor write protection |
| `hal::InterruptController` / `Timer` | architecture backend | interrupt and timer lifecycle |
| `kernel::SubsystemLifecycle` | generic subsystem | named, one-time initialization |
| `drivers::Device` / `hal::DriverLifecycle` | device layer | discovery-independent init and shutdown |
| `fs::Vfs` | filesystem layer | normalized paths and fixed-size operations |
| `userspace::syscalls::dispatch_args` | syscall boundary | register ABI dispatch and pointer validation |

Architecture-independent code may depend on HAL traits, but may not include
x86 registers, port I/O, IDT/MSR/CR manipulation, or assembly stubs directly.
