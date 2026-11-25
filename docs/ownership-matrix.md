# PDC-OS 0.2.0 ownership matrix

Every resource has one owning subsystem and one cleanup path.  References,
mapping entries, and locks do not transfer ownership.

| Resource | Owner | Allocator or creator | Cleanup owner | Reclamation rule |
| --- | --- | --- | --- | --- |
| 4 KiB physical frame | physical frame bitmap | `memory::allocate_frame` | frame allocator or accounting owner | Zero is reserved; free only if currently allocated and aligned |
| Kernel heap block | heap allocator | `GlobalAlloc::alloc` | matching deallocation/layout operation | Layout and range must remain valid; metadata corruption is fatal |
| PML4/address-space root | address-space object | `memory::create_address_space` | `memory::destroy_address_space` | Clear slot first, walk private tables, then return the root frame |
| Page-table child frame | address space | page-table `next_table` path | address-space destruction | Reclaim only private page-table frames; never reclaim kernel/bootstrap tables |
| Virtual mapping | address-space mapping table | `memory::map_*_page` | `memory::unmap_*_page` or address-space destruction | Mapping count changes once per leaf; mapping never owns the physical frame |
| Process record | process table | `create_process*` | `destroy_process` | Destroy all threads, then address space, then clear process slot |
| Thread record/context slot | process table/scheduler | `create_thread`/`spawn` | `destroy_thread`/process cleanup | Clear state and scheduler reference before process reaping |
| Kernel stack frames | thread | thread creation | `destroy_thread` or `destroy_process` | Deallocate private stack and guard frames |
| Userspace stack mapping/frame | thread/process | `allocate_user_stack` | thread cleanup, then process cleanup | Unmap first, then deallocate the physical frame |
| Wait-queue entry | wait queue | `WaitQueue::wait` | `remove` or wake operation | Entry is allocation-free and at most one per task per queue |
| Scheduler task slot | scheduler | `spawn*` | `cleanup` | Cleanup only from scheduler/process-compatible lifecycle code |
| IRQ lock guard | guard owner | `IrqLock::lock`/`try_lock` | guard drop | Always restore the exact interrupt-state snapshot |
| Device descriptor/resource claim | device registry | `register_descriptor` | `remove` | Revoked resources and interrupts are cleared before `Removed` |
| Interrupt registration | device registry | `register_interrupt` | `unregister_interrupt`/removal | Owner-and-vector scoped removal is mandatory |
| Open file/cache entry | filesystem/VFS | VFS/FS operations | VFS removal or process cleanup | Fixed-size cache entries have explicit invalidation |
| Syscall/terminal handle state | userspace service owner | service/syscall initialization | subsystem shutdown/release path | No orphaned handle may survive process destruction |
| Per-CPU current-thread data | architecture/per-CPU backend | per-CPU initialization | backend teardown or reboot | Current task and interrupt nesting remain per-CPU, not process-owned |

## Cleanup sequence

1. Stop accepting new references or lifecycle transitions.
2. Remove wait-queue, interrupt, and terminal references first.
3. Destroy threads and their private stacks/userspace mappings.
4. Destroy the process address space and private page tables.
5. Clear process/device table ownership state last.
6. Report partial cleanup through diagnostics; never silently convert cleanup
   failure into success.
