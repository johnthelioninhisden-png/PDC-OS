//! 10.7 security and reliability invariants.
//! 
//! These checks are deliberately bounded and allocation-free.  They exercise
//! adversarial syscall numbers, pointer ranges, malformed ELF headers, VFS
//! traversal, oversized writes, and fixed-resource limits at boot and through
//! the `security` diagnostic command.

#[derive(Clone, Copy)]
pub struct SecurityReport {
    pub cases: usize,
    pub passed: usize,
}

impl SecurityReport {
    pub fn failed(&self) -> usize {
        self.cases.saturating_sub(self.passed)
    }
}

fn check(report: &mut SecurityReport, condition: bool) {
    let case = report.cases + 1;
    report.cases += 1;
    if condition {
        report.passed += 1;
    } else {
        crate::serial::println(format_args!(
            "Security review: 10.7 case {} failed",
            case
        ));
    }
}

/// Run the bounded adversarial review suite.  Failures are reported as stable
/// test counts; they never attempt to continue with an untested subsystem.
pub fn run_adversarial_tests() -> SecurityReport {
    let mut report = SecurityReport { cases: 0, passed: 0 };

    // Unknown and reserved syscall numbers must have one stable ABI result.
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(usize::MAX, 0, 0, 0)
            == crate::os::syscalls::ENOSYS,
    );
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(9999, usize::MAX, usize::MAX, 0)
            == crate::os::syscalls::ENOSYS,
    );

    // Pointer arithmetic and zero-length behavior must never reach the kernel.
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(crate::os::syscalls::SYS_WRITE, 0, 1, 0)
            != 0,
    );
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(
            crate::os::syscalls::SYS_READ,
            3,
            0,
            usize::MAX,
        ) == crate::os::syscalls::EFAULT,
    );
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(
            crate::os::syscalls::SYS_EXEC_ELF,
            0,
            4 * 1024 * 1024 + 1,
            0,
        ) == crate::os::syscalls::EFAULT
            || crate::os::syscalls::dispatch_args(
                crate::os::syscalls::SYS_EXEC_ELF,
                0,
                4 * 1024 * 1024 + 1,
                0,
            ) == crate::os::syscalls::EINVAL,
    );

    // Path validation rejects traversal and malformed separators.
    check(&mut report, crate::fs::Vfs::normalize("/bin/../etc") .is_none());
    check(&mut report, crate::fs::Vfs::normalize("") .is_none());
    check(&mut report, crate::fs::Vfs::normalize("relative") .is_none());
    check(&mut report, crate::fs::Vfs::normalize("/bin") .is_some());

    // Fixed-size VFS writes and mount operations return typed errors.
    let oversized = [0u8; crate::fs::FILE_LIMIT + 1];
    crate::fs::with(|vfs| {
        check(
            &mut report,
            vfs.write("/README", &oversized)
                == Err(crate::fs::FsError::TooLarge),
        );
        check(
            &mut report,
            vfs.mount("not-a-filesystem", "/tmp")
                == Err(crate::fs::FsError::NotFound),
        );
    });

    // Credential and wait operations remain stable at identity boundaries.
    check(
        &mut report,
        crate::os::syscalls::dispatch_args(
            crate::os::syscalls::SYS_SETUID,
            usize::MAX,
            0,
            0,
        ) == crate::os::syscalls::EINVAL
            || crate::os::syscalls::dispatch_args(
                crate::os::syscalls::SYS_SETUID,
                usize::MAX,
                0,
                0,
            ) == crate::os::syscalls::EPERM,
    );

    report
}
