//! Kernel-wide invariant and initialization contracts.

use crate::recovery::{BootStage, InitError};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Stable ABI generation for kernel-owned interfaces.
///
/// The major value changes for incompatible ABI changes, the minor value for
/// backward-compatible additions, and the patch value for implementation fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

pub const INTERFACE_VERSION: InterfaceVersion = InterfaceVersion {
    major: 0,
    minor: 2,
    patch: 0,
};

/// PDC-OS release identity shared by the shell, VFS, and diagnostics.
///
/// Keep these values centralized so `uname`, `/etc/os-release`, release
/// manifests, and boot diagnostics cannot drift to different baselines.
pub const OS_NAME: &str = "PDC-OS";
pub const OS_VERSION: &str = "0.2.0";
pub const OS_ARCH: &str = "x86_64";
pub const OS_RELEASE: &str = "NAME=PDC-OS\nVERSION=0.2.0\nARCH=x86_64\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    InvalidArgument,
    Unavailable,
    Exhausted,
    InvalidState,
    NotFound,
    PermissionDenied,
    Fault,
    Unsupported,
    Initialization(InitError),
}

impl From<InitError> for KernelError {
    fn from(error: InitError) -> Self {
        Self::Initialization(error)
    }
}

impl From<crate::fs::FsError> for KernelError {
    fn from(error: crate::fs::FsError) -> Self {
        match error {
            crate::fs::FsError::NotFound => Self::NotFound,
            crate::fs::FsError::NotDirectory
            | crate::fs::FsError::NotFile
            | crate::fs::FsError::NameTooLong
            | crate::fs::FsError::NotMounted => Self::InvalidArgument,
            crate::fs::FsError::Exists => Self::InvalidState,
            crate::fs::FsError::TableFull => Self::Exhausted,
            crate::fs::FsError::TooLarge => Self::InvalidArgument,
        }
    }
}

impl KernelError {
    /// Stable short name suitable for diagnostics and fixed-format logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid-argument",
            Self::Unavailable => "unavailable",
            Self::Exhausted => "exhausted",
            Self::InvalidState => "invalid-state",
            Self::NotFound => "not-found",
            Self::PermissionDenied => "permission-denied",
            Self::Fault => "fault",
            Self::Unsupported => "unsupported",
            Self::Initialization(error) => error.name(),
        }
    }

    /// Convert a kernel error to the current negative syscall errno convention.
    pub const fn errno(self) -> isize {
        match self {
            Self::InvalidArgument => -22,
            Self::Unavailable => -16,
            Self::Exhausted => -12,
            Self::InvalidState => -16,
            Self::NotFound => -2,
            Self::PermissionDenied => -1,
            Self::Fault => -14,
            Self::Unsupported => -38,
            Self::Initialization(_) => -5,
        }
    }
}

/// Common result type for subsystem-facing kernel APIs.
pub type ApiResult<T> = Result<T, KernelError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    Memory,
    Scheduler,
    Interrupts,
    Process,
    Filesystem,
    Userspace,
    Drivers,
    Security,
    Recovery,
}

impl Subsystem {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Scheduler => "scheduler",
            Self::Interrupts => "interrupts",
            Self::Process => "process",
            Self::Filesystem => "filesystem",
            Self::Userspace => "userspace",
            Self::Drivers => "drivers",
            Self::Security => "security",
            Self::Recovery => "recovery",
        }
    }
}

/// Base contract for subsystem APIs and initialization implementations.
pub trait SubsystemApi {
    fn subsystem(&self) -> Subsystem;
    fn interface_version(&self) -> InterfaceVersion {
        INTERFACE_VERSION
    }
}

pub trait SubsystemLifecycle: SubsystemApi {
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
    check(crate::kernel::percpu::initialized());
    check(crate::kernel::process::PROCESS_TABLE.process_count() <= 16);
    check(crate::kernel::process::PROCESS_TABLE.thread_count() <= 32);
    check(crate::interrupts::interrupts_enabled() || stage != BootStage::Complete);
    INVARIANT_FAILURES.load(Ordering::Relaxed)
}

pub fn invariant_checks() -> usize {
    INVARIANT_CHECKS.load(Ordering::Relaxed)
}
