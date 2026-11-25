//! Kernel-wide invariant and initialization contracts.

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::recovery::{BootStage, InitError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    InvalidArgument,
    Unavailable,
    Exhausted,
    InvalidState,
    Initialization(InitError),
}

impl From<InitError> for KernelError {
    fn from(error: InitError) -> Self {
        Self::Initialization(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    Memory,
    Scheduler,
    Interrupts,
    Filesystem,
    Userspace,
}

pub trait SubsystemLifecycle {
    fn name(&self) -> &'static str;
    fn initialize(&mut self) -> Result<(), KernelError>;
}

static INVARIANT_CHECKS: AtomicUsize = AtomicUsize::new(0);
static INVARIANT_FAILURES: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn check(condition: bool) {
    INVARIANT_CHECKS.fetch_add(1, Ordering::Relaxed);
    if !condition {
        INVARIANT_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Run allocation-free invariants for the always-present kernel subsystems.
/// Callers report the stable result counters through boot diagnostics.
pub fn run_invariants(stage: BootStage) -> usize {
    check(crate::memory::usable_bytes() >= crate::memory::allocated_bytes());
    check(crate::memory::frame_size() == 4096);
    check(crate::scheduler::SCHEDULER.with(|scheduler| scheduler.task_count() <= 16));
    check(crate::interrupts::interrupts_enabled()
        || stage != BootStage::Complete);
    INVARIANT_FAILURES.load(Ordering::Relaxed)
}

pub fn invariant_checks() -> usize {
    INVARIANT_CHECKS.load(Ordering::Relaxed)
}
