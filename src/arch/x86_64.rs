//! x86-64 architecture backend.

use crate::hal::{
    AddressSpaceBackend, Cpu, CpuMemoryProtection, TlbInvalidation,
};
use crate::memory::{PageFlags, PhysicalAddress, VirtualAddress};

impl Cpu for HardwareInfo {
    fn vendor(&self) -> &str {
        HardwareInfo::vendor(self)
    }

    fn supports_pae(&self) -> bool {
        self.pae
    }

    fn supports_long_mode(&self) -> bool {
        self.long_mode
    }
}

pub use crate::hardware::HardwareInfo;
pub use crate::hardware::detect;

pub struct X86_64;

impl AddressSpaceBackend for X86_64 {
    fn create(&mut self) -> Option<usize> {
        crate::memory::create_address_space()
    }

    fn destroy(&mut self, address_space_id: usize) -> bool {
        crate::memory::destroy_address_space(address_space_id)
    }

    fn map(
        &mut self,
        address_space_id: usize,
        virtual_address: VirtualAddress,
        physical_address: PhysicalAddress,
        flags: PageFlags,
    ) -> bool {
        crate::memory::map_typed_page(
            address_space_id,
            virtual_address,
            physical_address,
            flags,
        )
    }

    fn unmap(&mut self, address_space_id: usize, virtual_address: VirtualAddress) -> bool {
        crate::memory::unmap_typed_page(address_space_id, virtual_address)
    }

    fn activate(&mut self, address_space_id: usize) -> bool {
        crate::memory::activate_address_space(address_space_id)
    }
}

impl TlbInvalidation for X86_64 {
    fn invalidate_page(&mut self, virtual_address: VirtualAddress) {
        let address = virtual_address.as_usize();
        unsafe {
            core::arch::asm!(
                "invlpg [{address}]",
                address = in(reg) address,
                options(nostack, preserves_flags),
            );
        }
    }

    fn invalidate_address_space(&mut self) {
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {cr3}, cr3", cr3 = out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov cr3, {cr3}", cr3 = in(reg) cr3, options(nostack, preserves_flags));
        }
    }
}

impl CpuMemoryProtection for X86_64 {
    fn enable_no_execute(&mut self) -> bool {
        let mut supported = false;
        unsafe {
            let mut edx: u32;
            let mut ebx: u32;
            let mut eax: u32;
            let mut ecx: u32;
            core::arch::asm!(
                "push rbx",
                "mov eax, 0x80000001",
                "cpuid",
                "mov {ebx_out:e}, ebx",
                "mov {edx_out:e}, edx",
                "pop rbx",
                edx_out = out(reg) edx,
                ebx_out = out(reg) ebx,
                inout("eax") 0u32 => eax,
                out("ecx") ecx,
                options(nostack, nomem, preserves_flags),
            );
            supported = edx & (1 << 20) != 0;
            if supported {
                let mut low: u32;
                let mut high: u32;
                core::arch::asm!(
                    "rdmsr",
                    in("ecx") 0xc0000080u32,
                    lateout("eax") low,
                    lateout("edx") high,
                    options(nostack, nomem, preserves_flags),
                );
                low |= 1 << 11;
                core::arch::asm!(
                    "wrmsr",
                    in("ecx") 0xc0000080u32,
                    in("eax") low,
                    in("edx") high,
                    options(nostack, nomem, preserves_flags),
                );
            }
        }
        supported
    }

    fn enable_write_protection(&mut self) -> bool {
        unsafe {
            let cr0: u64;
            core::arch::asm!("mov {cr0}, cr0", cr0 = out(reg) cr0, options(nomem, nostack));
            core::arch::asm!(
                "mov cr0, {cr0}",
                cr0 = in(reg) cr0 | (1u64 << 16),
                options(nostack, preserves_flags),
            );
        }
        true
    }
}
