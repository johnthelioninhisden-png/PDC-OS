# PDC-OS architecture HAL contract

Generic kernel code calls HAL traits rather than architecture registers,
instructions, or I/O ports.  An architecture backend is responsible for
implementing the following contracts and preserving bounded, typed behavior.

## Trait requirements

| Trait | Required behavior |
| --- | --- |
| `Cpu` | Report vendor and PAE/long-mode feature state; unsupported features are false, not guessed |
| `AddressSpaceBackend::create` | Create one independently addressable root or return `None` |
| `AddressSpaceBackend::destroy` | Reclaim private address-space resources and return false for unknown IDs |
| `AddressSpaceBackend::map` | Validate typed addresses/flags and publish the leaf mapping atomically |
| `AddressSpaceBackend::unmap` | Remove only an existing leaf and invalidate its address as required |
| `AddressSpaceBackend::activate` | Activate an existing address space or return false |
| `TlbInvalidation` | Page invalidation and address-space-wide invalidation must be observable before return |
| `CpuMemoryProtection` | Return true only after NX/WP control-register readback confirms the bit |
| `InterruptController` | Initialize or return a typed `InitError`; acknowledge by architecture vector |
| `Timer` | Initialize or return a typed error; expose only non-decreasing tick identity |
| `ThreadContext` | Save/restore exactly bounded backend context storage; reject uninitialized context |
| `DriverLifecycle` | Initialize/shutdown predictably and publish readiness only after success |

## Backend obligations

- Address types remain architecture-neutral wrappers.  Backends must reject
  misaligned, null, out-of-domain, and cross-domain addresses.
- Mapping APIs must reject executable-writable combinations and kernel mappings
  with userspace-only flags.
- Every allocation failure and partial table creation must leave no published
  address-space object or frame leak.
- TLB invalidation must happen after leaf mutation.  Address-space-wide
  invalidation may reload the backend page-table control register.
- Interrupt masking and wakeup are not substituted for allocator or process
  ownership.  IRQ work remains bounded and non-blocking.
- A backend may provide compatibility shims, but generic code must not include
  architecture-specific registers, assembly, or port-I/O symbols.
- Unsupported operations must return `false`, `None`, or a typed kernel/init
  error.  They must not spin, reboot, or silently report success.
