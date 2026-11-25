//! Boot-stage and recovery diagnostics.
//!
//! This module provides a bounded, allocation-free recovery path shared by
//! initialization failures, exceptions, and the panic handler.

use crate::interrupts::save_interrupt_state;
use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootStage {
    Serial,
    Cpu,
    Memory,
    Scheduler,
    Interrupts,
    Drivers,
    Filesystem,
    Userspace,
    Terminal,
    Complete,
}

impl BootStage {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Serial => "serial",
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Scheduler => "scheduler",
            Self::Interrupts => "interrupts",
            Self::Drivers => "drivers",
            Self::Filesystem => "filesystem",
            Self::Userspace => "userspace",
            Self::Terminal => "terminal",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    TimedOut,
    NotPresent,
    TestFailed,
    InvalidResponse,
    Busy,
    Unsupported,
}

impl InitError {
    pub const fn name(self) -> &'static str {
        match self {
            Self::TimedOut => "timeout",
            Self::NotPresent => "not-present",
            Self::TestFailed => "self-test-failed",
            Self::InvalidResponse => "invalid-response",
            Self::Busy => "busy",
            Self::Unsupported => "unsupported",
        }
    }
}

static BOOT_STAGE: AtomicUsize = AtomicUsize::new(0);

pub fn set_boot_stage(stage: BootStage) {
    BOOT_STAGE.store(stage as usize, Ordering::Release);
    crate::serial::println(format_args!("PDC-OS boot stage: {}", stage.name()));
}

pub fn boot_stage() -> BootStage {
    match BOOT_STAGE.load(Ordering::Acquire) {
        1 => BootStage::Cpu,
        2 => BootStage::Memory,
        3 => BootStage::Scheduler,
        4 => BootStage::Interrupts,
        5 => BootStage::Drivers,
        6 => BootStage::Filesystem,
        7 => BootStage::Userspace,
        8 => BootStage::Terminal,
        9 => BootStage::Complete,
        _ => BootStage::Serial,
    }
}

pub fn report_init_failure(device: &str, error: InitError) {
    crate::serial::println(format_args!(
        "PDC-OS initialization failure: device={} error={} stage={}",
        device,
        error.name(),
        boot_stage().name()
    ));
    crate::println!("{} unavailable: {}", device, error.name());
}

pub fn report_system_context(prefix: &str) {
    let rflags = save_interrupt_state();
    let cr2: u64;
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nostack, nomem));
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, nomem));
    }
    crate::serial::println(format_args!(
        "{} context: stage={} CR2={:#x} CR3={:#x} RFLAGS={:#x} ticks={} tasks={} interrupts={}",
        prefix,
        boot_stage().name(),
        cr2,
        cr3,
        rflags,
        crate::interrupts::tick_count(),
        crate::scheduler::SCHEDULER.with(|scheduler| scheduler.task_count()),
        if rflags & (1 << 9) != 0 {
            "enabled"
        } else {
            "disabled"
        }
    ));
}

/// Reset through the emulated 0xCF9 reset control register.
pub fn reboot() -> ! {
    crate::serial::println(format_args!("PDC-OS reboot requested"));
    report_system_context("KERNEL REBOOT:");
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
        crate::drivers::io::outb(0xCF9, 0x02);
        crate::drivers::io::outb(0xCF9, 0x06);
        // PS/2 keyboard-controller reset is an older fallback for QEMU/PC.
        crate::drivers::io::outb(0x64, 0xFE);
    }
    halt("reboot did not complete");
}

/// ACPI-compatible emulated shutdown port used by QEMU, with bounded fallback.
pub fn shutdown() -> ! {
    crate::serial::println(format_args!("PDC-OS shutdown requested"));
    report_system_context("KERNEL SHUTDOWN:");
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
        // QEMU maps this 16-bit I/O port to ACPI poweroff.
        crate::drivers::io::outw(0x604, 0x2000);
    }
    halt("shutdown is unsupported or did not complete");
}

pub fn halt(reason: &str) -> ! {
    crate::serial::println(format_args!(
        "PDC-OS halted: {} stage={} ticks={}",
        reason,
        boot_stage().name(),
        crate::interrupts::tick_count()
    ));
    crate::println!("PDC-OS halted: {}", reason);
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
