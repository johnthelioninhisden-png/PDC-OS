//! Bounded kernel architecture verification suite.
//!
//! Checks are allocation-bounded and run during boot so ownership and cleanup
//! failures are visible on serial/VGA instead of remaining latent.

use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
pub struct VerificationReport {
    pub cases: usize,
    pub failures: usize,
}

impl VerificationReport {
    pub fn passed(&self) -> usize {
        self.cases - self.failures
    }
}

static CASES: AtomicUsize = AtomicUsize::new(0);
static FAILURES: AtomicUsize = AtomicUsize::new(0);

fn check(condition: bool) {
    CASES.fetch_add(1, Ordering::Relaxed);
    if !condition {
        FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn run() -> VerificationReport {
    // Stage 10 migration checks keep release identity and compatibility
    // surfaces stable as the old source layout is retired incrementally.
    crate::serial::println(format_args!("Architecture verification: migration"));
    check(crate::kernel::core::OS_NAME == "PDC-OS");
    check(crate::kernel::core::OS_VERSION == "0.2.0");
    check(crate::kernel::core::OS_ARCH == "x86_64");
    check(crate::kernel::core::INTERFACE_VERSION.major == 0);
    check(crate::kernel::core::INTERFACE_VERSION.minor == 2);
    check(crate::kernel::core::INTERFACE_VERSION.patch == 0);

    // Stage 7 interface-contract checks: error types, ABI metadata, and
    // versioned subsystem identities must stay internally consistent.
    crate::serial::println(format_args!("Architecture verification: kernel-interface"));
    check(crate::kernel::core::INTERFACE_VERSION.major == 0);
    check(crate::kernel::core::INTERFACE_VERSION.minor == 2);
    check(crate::userspace::syscalls::ABI_VERSION == crate::kernel::core::INTERFACE_VERSION);
    check(
        crate::userspace::syscalls::syscall_info(crate::userspace::syscalls::SYS_WRITE)
            .map(|info| info.name == "write")
            .unwrap_or(false),
    );
    check(crate::userspace::syscalls::syscall_info(usize::MAX).is_none());
    check(crate::kernel::core::KernelError::NotFound.name() == "not-found");
    check(crate::kernel::core::KernelError::NotFound.errno() == crate::userspace::syscalls::ENOENT);

    crate::serial::println(format_args!("Architecture verification: address-types"));
    check(crate::memory::frame_size() == 4096);
    check(crate::memory::PhysicalAddress::new(4096).is_some());
    check(crate::memory::PhysicalAddress::new(4097).is_none());
    check(crate::memory::PhysicalAddress::new(0).is_none());
    check(crate::memory::VirtualAddress::new(crate::memory::USER_VIRTUAL_START).is_some());
    check(crate::memory::VirtualAddress::new(0).is_none());

    // Repeat frame allocation/deallocation to catch bitmap state leaks.
    crate::serial::println(format_args!("Architecture verification: frame-allocator"));
    for _ in 0..16 {
        let Some(frame) = crate::memory::allocate_frame() else {
            check(false);
            continue;
        };
        check(frame % crate::memory::frame_size() == 0);
        check(crate::memory::deallocate_frame(frame));
    }
    check(crate::memory::frame_exhaustion_probe());

    // Exercise heap reuse and an injected exhaustion result.
    crate::serial::println(format_args!("Architecture verification: heap-allocator"));
    check(crate::memory::heap_repeated_probe());
    check(crate::memory::heap_exhaustion_probe());

    // Reject wrong Multiboot identity and truncated/overflowed boot-info pointers.
    crate::serial::println(format_args!("Architecture verification: boot-information"));
    check(!crate::memory::validate_boot_info(0, 0));
    check(!crate::memory::validate_boot_info(0x2BAD_B002, 0));
    check(!crate::memory::validate_boot_info(0, 0x1000));
    check(!crate::memory::validate_boot_info(
        0x2BAD_B002,
        usize::MAX,
    ));

    // Stage 15 failure paths reject malformed callers before they can mutate
    // allocator or address-space state, and exercise cleanup after partial work.
    crate::serial::println(format_args!("Architecture verification: failure-paths"));
    check(!crate::memory::deallocate_frame(0));
    check(!crate::memory::deallocate_frame(4097));
    check(!crate::memory::deallocate_frame(crate::memory::PAGE_TABLE_ADDRESS_LIMIT));
    check(crate::memory::VirtualAddress::new(4097).is_none());
    check(crate::memory::PhysicalAddress::new(4097).is_none());
    check(!crate::memory::destroy_address_space(usize::MAX));
    check(!crate::memory::activate_address_space(usize::MAX));
    check(crate::memory::address_space_mapping_count(usize::MAX).is_none());

    // Exercise address-space creation, mapping, unmapping, and destruction.
    crate::serial::println(format_args!("Architecture verification: address-spaces"));
    for _ in 0..2 {
        let Some(space) = crate::memory::create_address_space() else {
            check(false);
            continue;
        };
        crate::serial::println(format_args!("Architecture verification: address created"));
        let Some(leaf) = crate::memory::allocate_typed_frame() else {
            check(false);
            let _ = crate::memory::destroy_address_space(space);
            continue;
        };
        let virtual_address = crate::memory::VirtualAddress::new(0x400000)
            .expect("verification virtual address is 4 KiB aligned");
        check(crate::memory::map_typed_page(
            space,
            virtual_address,
            leaf,
            crate::memory::PageFlags::UserReadWrite,
        ));
        check(crate::memory::address_space_mapping_count(space) == Some(1));
        check(crate::memory::destroy_address_space(space));
        check(crate::memory::address_space_mapping_count(space).is_none());
    }

    // Create a partially populated address space, remove one mapping, then
    // destroy it while private intermediate tables may still be live.
    if let Some(partial_space) = crate::memory::create_address_space() {
        if let Some(first_leaf) = crate::memory::allocate_typed_frame() {
            let first = crate::memory::VirtualAddress::new(0x500000)
                .expect("failure-path virtual address is page aligned");
            check(crate::memory::map_typed_page(
                partial_space,
                first,
                first_leaf,
                crate::memory::PageFlags::UserReadOnly,
            ));
            check(crate::memory::unmap_typed_page(partial_space, first));
            check(crate::memory::address_space_mapping_count(partial_space) == Some(0));
            check(crate::memory::deallocate_typed_frame(first_leaf));
            check(crate::memory::destroy_address_space(partial_space));
            check(crate::memory::address_space_mapping_count(partial_space).is_none());
        } else {
            check(false);
            let _ = crate::memory::destroy_address_space(partial_space);
        }
    } else {
        check(false);
    }

    check(crate::memory::VirtualAddress::new(0).is_none());

    // Exercise process/thread lifecycle, private stacks, and owned cleanup.
    crate::serial::println(format_args!("Architecture verification: process-threads"));
    if let Some(space) = crate::memory::create_address_space() {
        if let Some(process_id) =
            crate::kernel::process::PROCESS_TABLE.create_process_with_id(9001, 1, space, 0)
        {
            check(
                crate::kernel::process::PROCESS_TABLE
                    .set_process_state(process_id, crate::kernel::process::ProcessState::Running),
            );
            if let Some(thread_id) = crate::kernel::process::PROCESS_TABLE.create_thread(process_id)
            {
                check(
                    crate::kernel::process::PROCESS_TABLE
                        .set_thread_state(thread_id, crate::kernel::process::ThreadState::Ready),
                );
                check(
                    crate::kernel::process::PROCESS_TABLE
                        .set_thread_state(thread_id, crate::kernel::process::ThreadState::Running),
                );
                if let Some(thread) =
                    crate::kernel::process::PROCESS_TABLE.thread_snapshot(thread_id)
                {
                    check(thread.kernel_stack_base != 0);
                    check(
                        thread.kernel_stack_top
                            == thread.kernel_stack_base + crate::kernel::process::KERNEL_STACK_SIZE,
                    );
                    check(thread.kernel_stack_guard != 0);
                    check(thread.user_stack_base >= crate::memory::USER_VIRTUAL_START);
                    check(thread.user_stack_physical != 0);
                } else {
                    check(false);
                }
                check(
                    crate::kernel::process::PROCESS_TABLE
                        .set_thread_state(thread_id, crate::kernel::process::ThreadState::Exiting),
                );
                check(crate::kernel::process::PROCESS_TABLE.destroy_thread(thread_id));
            } else {
                check(false);
                let _ = crate::memory::destroy_address_space(space);
            }
            check(
                crate::kernel::process::PROCESS_TABLE
                    .set_process_state(process_id, crate::kernel::process::ProcessState::Exiting),
            );
            check(crate::kernel::process::PROCESS_TABLE.destroy_process(process_id));
        } else {
            check(false);
            let _ = crate::memory::destroy_address_space(space);
        }
    } else {
        check(false);
    }

    // Invalid process/thread operations and a process without threads must
    // remain safe, while cleanup must release its address-space owner.
    if let Some(failure_space) = crate::memory::create_address_space() {
        if let Some(failure_process) =
            crate::kernel::process::PROCESS_TABLE.create_process_with_id(
                9002,
                9001,
                failure_space,
                0,
            )
        {
            check(
                !crate::kernel::process::PROCESS_TABLE
                    .set_thread_state(usize::MAX, crate::kernel::process::ThreadState::Ready),
            );
            check(
                !crate::kernel::process::PROCESS_TABLE
                    .set_process_state(
                        usize::MAX,
                        crate::kernel::process::ProcessState::Running,
                    ),
            );
            check(!crate::kernel::process::PROCESS_TABLE.destroy_thread(usize::MAX));
            check(!crate::kernel::process::PROCESS_TABLE.destroy_process(usize::MAX));
            check(crate::kernel::process::PROCESS_TABLE.destroy_process(failure_process));
            check(!crate::kernel::process::PROCESS_TABLE.process_exists(failure_process));
        } else {
            check(false);
            let _ = crate::memory::destroy_address_space(failure_space);
        }
    } else {
        check(false);
    }

    // Test IRQ-disabled critical-section preservation.
    let interrupts_before = crate::interrupts::interrupts_enabled();
    let guard = crate::interrupts::IrqLock::new(0usize);
    let locked = guard.lock();
    check(!crate::interrupts::interrupts_enabled());
    drop(locked);
    check(crate::interrupts::interrupts_enabled() == interrupts_before);

    // Verify non-blocking IRQ-lock acquisition and lock statistics.
    let try_lock = crate::interrupts::IrqLock::new(17usize);
    let first = try_lock.try_lock();
    check(first.is_some());
    check(!try_lock.try_lock().is_some());
    check(try_lock.is_locked());
    check(try_lock.try_lock().is_none());
    drop(first);
    check(try_lock.try_lock().is_some());
    let (acquisitions, _) = try_lock.statistics();
    check(acquisitions >= 2);

    // Verify allocation-free wait queue registration and wake ordering.
    let wait_queue = crate::kernel::sync::WaitQueue::new();
    check(wait_queue.wait(41, crate::kernel::sync::WaitReason::Io));
    check(wait_queue.wait(42, crate::kernel::sync::WaitReason::Io));
    check(!wait_queue.wait(41, crate::kernel::sync::WaitReason::Device));
    check(wait_queue.contains(41));
    check(wait_queue.reason(41) == Some(crate::kernel::sync::WaitReason::Io));
    check(wait_queue.len() == 2);
    check(wait_queue.wake_one() == Some(41));
    check(!wait_queue.contains(41));
    check(wait_queue.remove(42));
    check(wait_queue.len() == 0);

    // Fill the fixed wait queue to verify bounded exhaustion and duplicate
    // registration behavior without allocating or corrupting waiter storage.
    let full_wait_queue = crate::kernel::sync::WaitQueue::new();
    for task_id in 100..116 {
        check(full_wait_queue.wait(task_id, crate::kernel::sync::WaitReason::Mutex));
    }
    check(full_wait_queue.len() == 16);
    check(!full_wait_queue.wait(116, crate::kernel::sync::WaitReason::Sleep));
    check(full_wait_queue.wake_all() == 16);
    check(full_wait_queue.len() == 0);

    // Verify scheduler block/wakeup transitions used by wait queues.
    crate::serial::println(format_args!("Architecture verification: synchronization"));
    crate::scheduler::SCHEDULER.with(|scheduler| {
        check(scheduler.block_task(1234) == false);
        check(scheduler.wake_task(1234) == false);
        check(scheduler.spawn_with_entry(8801, "verification/task", verification_task));
        check(scheduler.cleanup(8801));
        check(scheduler.task_count() <= 16);
    });

    // Verify typed device discovery, resource ownership, IRQ contracts, and removal.
    crate::serial::println(format_args!("Architecture verification: driver-registry"));
    let mut probe = crate::drivers::DeviceDescriptor::new(
        "verification probe",
        crate::drivers::DeviceClass::Platform,
    );
    probe.resources[0] = crate::drivers::HardwareResource::PortRange {
        base: 0x2E8,
        length: 8,
    };
    probe.resource_count = 1;
    check(crate::drivers::DEVICE_REGISTRY.register_descriptor(probe));
    check(
        crate::drivers::DEVICE_REGISTRY
            .descriptor("verification probe")
            .is_some(),
    );

    let mut duplicate = probe;
    duplicate.name = "verification duplicate";
    check(!crate::drivers::DEVICE_REGISTRY.register_descriptor(duplicate));
    check(crate::drivers::DEVICE_REGISTRY.initialize("verification probe"));
    check(crate::drivers::DEVICE_REGISTRY.activate("verification probe"));
    check(crate::drivers::DEVICE_REGISTRY.register_interrupt(
        "verification probe",
        44,
        12,
        crate::drivers::InterruptMode::Exclusive,
        None,
    ));
    check(crate::drivers::DEVICE_REGISTRY.dispatch_interrupt(44));
    check(crate::drivers::DEVICE_REGISTRY.unregister_interrupt("verification probe", 44));
    check(crate::drivers::DEVICE_REGISTRY.remove("verification probe"));
    check(
        crate::drivers::DEVICE_REGISTRY
            .descriptor("verification probe")
            .is_none(),
    );

    // Verify Stage 4 execution-infrastructure contracts.
    check(crate::arch::x86_64::tss::initialized());
    check(crate::arch::x86_64::tss::ist_frame() != 0);
    check(crate::kernel::percpu::initialized());
    check(crate::kernel::interrupts::INTERRUPT_FRAME_SIZE == 176);

    VerificationReport {
        cases: CASES.load(Ordering::Relaxed),
        failures: FAILURES.load(Ordering::Relaxed),
    }
}

extern "C" fn verification_task() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
