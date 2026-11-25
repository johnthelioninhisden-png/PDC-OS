//! x86-64 interrupt-context construction and restoration.
//!
//! Generic scheduling owns context lifetime; this module defines only the
//! x86-64 IRET frame layout and the synthetic kernel entry frame.

use crate::hal::ThreadContext as HalThreadContext;

pub const CONTEXT_REGISTER_COUNT: usize = 22;

#[derive(Clone, Copy)]
pub struct X86_64Context {
    registers: [u64; CONTEXT_REGISTER_COUNT],
    initialized: bool,
}

impl X86_64Context {
    pub const fn empty() -> Self {
        Self {
            registers: [0; CONTEXT_REGISTER_COUNT],
            initialized: false,
        }
    }

    /// Build the IRET frame used for the first entry of a kernel task.
    pub fn new_kernel_entry(entry: extern "C" fn() -> !, stack_top: usize) -> Self {
        let mut registers = [0u64; CONTEXT_REGISTER_COUNT];
        registers[17] = entry as usize as u64; // IRET RIP.
        registers[18] = 0x08;                  // Kernel code segment.
        registers[19] = 0x202;                 // RFLAGS: IF plus reserved bit.
        registers[20] = stack_top as u64;      // IRET RSP.
        registers[21] = 0x10;                  // Kernel data segment.
        Self {
            registers,
            initialized: true,
        }
    }

    pub fn registers(&mut self) -> &mut [u64; CONTEXT_REGISTER_COUNT] {
        &mut self.registers
    }
}

impl HalThreadContext for X86_64Context {
    fn save(&mut self, source: &[u64]) -> bool {
        if source.len() != CONTEXT_REGISTER_COUNT {
            return false;
        }
        self.registers.copy_from_slice(source);
        self.initialized = true;
        true
    }

    fn restore(&self, destination: &mut [u64]) -> bool {
        if !self.initialized || destination.len() != CONTEXT_REGISTER_COUNT {
            return false;
        }
        destination.copy_from_slice(&self.registers);
        true
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }
}
