//! Minimal x86-64 Task-State Segment and IST infrastructure.
//!
//! PDC-OS is currently single-CPU and does not enter Ring 3.  The TSS is
//! nevertheless initialized now so privilege transitions and future IST entries
//! have no bootstrap-time architectural dependency.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[repr(C, packed)]
pub struct TaskStateSegment {
    pub reserved0: u32,
    pub rsp0: u64,
    pub rsp1: u64,
    pub rsp2: u64,
    pub reserved1: u64,
    pub ist1: u64,
    pub ist2: u64,
    pub ist3: u64,
    pub ist4: u64,
    pub ist5: u64,
    pub ist6: u64,
    pub ist7: u64,
    pub reserved2: u64,
    pub reserved3: u16,
    pub iomap_base: u16,
}

#[repr(C, align(4096))]
struct TssStorage {
    bytes: [u8; 4096],
}

#[link_section = ".bss"]
static mut TSS_STORAGE: TssStorage = TssStorage { bytes: [0; 4096] };
static IST_FRAME: AtomicUsize = AtomicUsize::new(0);
static RSP0_FRAME: AtomicUsize = AtomicUsize::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

extern "C" {
    static mut gdt64: [u64; 5];
    static gdt_pointer: [u8; 10];
}

/// Install a TSS descriptor and load the task-state selector.
pub fn init() -> bool {
    if INITIALIZED.load(Ordering::Acquire) {
        return true;
    }

    let ist_frame = match crate::memory::allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };
    let rsp0_frame = match crate::memory::allocate_frame() {
        Some(frame) => frame,
        None => {
            let _ = crate::memory::deallocate_frame(ist_frame);
            return false;
        }
    };
    IST_FRAME.store(ist_frame, Ordering::Release);
    RSP0_FRAME.store(rsp0_frame, Ordering::Release);

    unsafe {
        let tss = core::ptr::addr_of_mut!(TSS_STORAGE) as *mut u8 as *mut TaskStateSegment;
        core::ptr::write_bytes(tss as *mut u8, 0, core::mem::size_of::<TaskStateSegment>());
        let ist_top = ist_frame + crate::memory::frame_size();
        let rsp0_top = rsp0_frame + crate::memory::frame_size();
        (*tss).rsp0 = rsp0_top as u64;
        (*tss).ist1 = ist_top as u64;
        (*tss).iomap_base = core::mem::size_of::<TaskStateSegment>() as u16;

        let base = tss as u64;
        let limit = core::mem::size_of::<TaskStateSegment>() as u64 - 1;
        // 64-bit TSS descriptor: low and high halves in GDT entries 3 and 4.
        gdt64[3] = (limit & 0xFFFF)
            | ((base & 0xFFFF) << 16)
            | (((base >> 16) & 0xFF) << 32)
            | (0x89u64 << 40)
            | (((limit >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);
        gdt64[4] = (base >> 32) & 0xFFFF_FFFF;

        let pointer = core::ptr::addr_of!(gdt_pointer) as *const u8;
        core::arch::asm!("lgdt [{}]", in(reg) pointer, options(nostack, preserves_flags));
        core::arch::asm!("ltr {0:x}", in(reg) 0x18u16, options(nostack, preserves_flags));
    }
    INITIALIZED.store(true, Ordering::Release);
    true
}

pub fn ist_frame() -> usize {
    IST_FRAME.load(Ordering::Relaxed)
}

pub fn initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}
