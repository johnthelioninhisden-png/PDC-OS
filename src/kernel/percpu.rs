//! Per-CPU data abstraction.
//!
//! PDC-OS is currently a single-CPU kernel.  This fixed CPU-0 record gives
//! interrupt and scheduler code a migration path to per-CPU state without
//! introducing an SMP feature before architecture synchronization is complete.

use crate::hal::PerCpu;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static CPU_INITIALIZED: AtomicBool = AtomicBool::new(false);
static CPU_ID: AtomicUsize = AtomicUsize::new(0);
static IN_INTERRUPT: AtomicBool = AtomicBool::new(false);
static CURRENT_THREAD: AtomicUsize = AtomicUsize::new(0);
static INTERRUPT_DEPTH: AtomicUsize = AtomicUsize::new(0);

pub fn init(cpu_id: usize) {
    CPU_ID.store(cpu_id, Ordering::Release);
    CURRENT_THREAD.store(0, Ordering::Release);
    IN_INTERRUPT.store(false, Ordering::Release);
    INTERRUPT_DEPTH.store(0, Ordering::Release);
    CPU_INITIALIZED.store(true, Ordering::Release);
}

pub fn enter_interrupt(thread_id: usize) {
    CURRENT_THREAD.store(thread_id, Ordering::Release);
    INTERRUPT_DEPTH.fetch_add(1, Ordering::AcqRel);
    IN_INTERRUPT.store(true, Ordering::Release);
}

pub fn leave_interrupt() {
    let depth = INTERRUPT_DEPTH.load(Ordering::Acquire);
    if depth > 0 {
        let remaining = INTERRUPT_DEPTH.fetch_sub(1, Ordering::AcqRel) - 1;
        if remaining == 0 {
            IN_INTERRUPT.store(false, Ordering::Release);
        }
    }
}

pub fn snapshot() -> PerCpu {
    PerCpu {
        cpu_id: CPU_ID.load(Ordering::Acquire),
        in_interrupt: IN_INTERRUPT.load(Ordering::Acquire),
        current_thread: CURRENT_THREAD.load(Ordering::Acquire),
    }
}

pub fn initialized() -> bool {
    CPU_INITIALIZED.load(Ordering::Acquire)
}
