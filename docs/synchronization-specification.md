# PDC-OS synchronization specification

## Lock order

Observed lock order is documented as a partial order, not a global mutex:

1. The device registry, process, thread, filesystem, and scheduler tables use
   leaf `IrqLock` instances.
2. Public process cleanup is `threads -> processes`; process address-space
   replacement drops the process guard before page-table destruction.
3. Scheduler operations own task state and wait-queue transitions; IRQ handlers
   do not directly mutate scheduler task state.
4. Frame-bitmap and heap locks are allocator locks.  They are not allowed to
   call process, scheduler, filesystem, device, or terminal APIs.
5. VGA and serial locks are output sinks and may not call back into tables that
   may acquire output locks.

No code may acquire locks in the reverse direction of a known sibling order.
New lock edges require a review and an invariant/verification case.

## IRQ safety

`IrqLock<T>` saves the interrupt-enable state at acquisition and restores it at
guard drop.  `try_lock` is the non-blocking IRQ-context form.  A `None` result
means contention; it must not retry in a hot IRQ path or wait for a scheduler
yield.

Lock guards must remain bounded critical sections.  They must not perform
allocation, block, wake tasks, call arbitrary device work, or span subsystem
callbacks.

## Blocking and wakeup

For a blocking operation:

1. Mark the task `Blocked`.
2. Add exactly one allocation-free wait-queue entry under a stable reason.
3. Release every lock and yield through the scheduler.
4. Wakeup removes the wait-queue entry and requests `Ready`.
5. The scheduler or owning state machine performs the final task transition.

Wakeups publish task identity and do not acquire spin locks.  Invalid task
identity or duplicate queue membership is rejected rather than retried.

## Scheduling interactions

SMP remains outside the PDC-OS 0.2.0 execution model.  The 2-vCPU smoke profile
is boot-only and PDC-OS remains single-threaded for scheduling.  Per-CPU state
is nevertheless bounded to a CPU ID, interrupt nesting, and current thread so
future SMP work does not hide ownership in global variables.
