//! Architecture-independent kernel contracts.
//!
//! Generic kernel code depends on these interfaces instead of directly using
//! CPU instructions or device-specific I/O.  The x86-64 backend implements
//! the same contracts now and future architectures can provide alternate
//! implementations.
//!
//! Contract details, including typed-address validation, partial-creation
//! cleanup, TLB visibility, and unsupported-operation behavior, are specified
//! in `docs/hal-contract.md`.

use crate::memory::{PageFlags, PhysicalAddress, VirtualAddress};
use crate::recovery::InitError;

pub trait Cpu {
    fn vendor(&self) -> &str;
    fn supports_pae(&self) -> bool;
    fn supports_long_mode(&self) -> bool;
}

pub trait AddressSpaceBackend {
    fn create(&mut self) -> Option<usize>;
    fn destroy(&mut self, address_space_id: usize) -> bool;
    fn map(
        &mut self,
        address_space_id: usize,
        virtual_address: VirtualAddress,
        physical_address: PhysicalAddress,
        flags: PageFlags,
    ) -> bool;
    fn unmap(&mut self, address_space_id: usize, virtual_address: VirtualAddress) -> bool;
    fn activate(&mut self, address_space_id: usize) -> bool;
}

pub trait TlbInvalidation {
    fn invalidate_page(&mut self, virtual_address: VirtualAddress);
    fn invalidate_address_space(&mut self);
}

pub trait CpuMemoryProtection {
    fn enable_no_execute(&mut self) -> bool;
    fn enable_write_protection(&mut self) -> bool;
}

pub trait InterruptController {
    fn initialize(&mut self) -> Result<(), InitError>;
    fn acknowledge(&mut self, vector: u64);
}

pub trait Timer {
    fn initialize(&mut self) -> Result<(), InitError>;
    fn tick_count(&self) -> usize;
}

/// Architecture-neutral register-context contract.
///
/// Architecture backends provide storage and switching mechanics, while the
/// scheduler decides which thread owns the context.
pub trait ThreadContext {
    fn save(&mut self, source: &[u64]) -> bool;
    fn restore(&self, destination: &mut [u64]) -> bool;
    fn is_initialized(&self) -> bool;
}

/// Architecture-neutral view of a single CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerCpu {
    pub cpu_id: usize,
    pub in_interrupt: bool,
    pub current_thread: usize,
}

pub trait DriverLifecycle {
    fn name(&self) -> &'static str;
    fn initialize(&mut self) -> Result<(), InitError>;
    fn shutdown(&mut self) -> Result<(), InitError>;
    fn ready(&self) -> bool;
}
