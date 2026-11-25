//! Conservative x86 hardware discovery for boot diagnostics.
//!
//! The kernel keeps discovery read-only and does not assume that a reported
//! CPU, firmware, or device feature is implemented beyond the bootstrap checks.

use core::arch::asm;

#[derive(Clone, Copy)]
pub struct HardwareInfo {
    pub vendor: [u8; 12],
    pub pae: bool,
    pub long_mode: bool,
}

impl HardwareInfo {
    pub fn vendor(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.vendor) }
    }
}

/// Detect only the CPU features that PDC-OS uses at the x86/long-mode boundary.
pub fn detect() -> HardwareInfo {
    let mut _eax: u32;
    let mut ebx: u32;
    let mut ecx: u32;
    let mut edx: u32;

    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            inout("eax") 0u32 => _eax,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem, preserves_flags)
        );
    }
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&ecx.to_le_bytes());

    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _eax,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem, preserves_flags)
        );
    }
    let pae = edx & (1 << 6) != 0;

    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000001u32 => _eax,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem, preserves_flags)
        );
    }
    let long_mode = edx & (1 << 29) != 0;

    HardwareInfo { vendor, pae, long_mode }
}
