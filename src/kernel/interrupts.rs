//! Interrupt Descriptor Table, PIC remapping, and PIT timer setup.

use core::cell::UnsafeCell;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Interrupt-state-preserving spin lock for short critical sections.
///
/// Acquiring this lock disables IRQ delivery, preserves the prior RFLAGS.IF
/// state, and restores that state only when the guard is dropped.  The lock
/// must never be held across blocking operations or while acquiring a lock
/// with a lower place in the documented lock order.
pub struct IrqLock<T: ?Sized> {
    locked: AtomicBool,
    acquisitions: AtomicUsize,
    contended: AtomicUsize,
    value: UnsafeCell<T>,
}

pub struct IrqGuard<'a, T: ?Sized> {
    lock: &'a IrqLock<T>,
    rflags: u64,
}

impl<T> IrqLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            acquisitions: AtomicUsize::new(0),
            contended: AtomicUsize::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> IrqGuard<'_, T> {
        let rflags = save_interrupt_state();
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
        }
        loop {
            match self
                .locked
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => {
                    self.acquisitions.fetch_add(1, Ordering::Relaxed);
                    return IrqGuard { lock: self, rflags };
                }
                Err(_) => {
                    // Count observed contention, but keep the call bounded only
                    // by the lock holder; IRQ masking prevents recursive IRQ use.
                    self.contended.fetch_add(1, Ordering::Relaxed);
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Attempt acquisition once, preserving IRQ state on either outcome.
    /// This is suitable for IRQ handlers that must not spin.
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let rflags = save_interrupt_state();
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
        }
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.acquisitions.fetch_add(1, Ordering::Relaxed);
            Some(IrqGuard { lock: self, rflags })
        } else {
            // The failed try does not own the critical section, so restore the
            // caller's interrupt state immediately.
            restore_interrupt_state(rflags);
            None
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire)
    }

    pub fn statistics(&self) -> (usize, usize) {
        (
            self.acquisitions.load(Ordering::Relaxed),
            self.contended.load(Ordering::Relaxed),
        )
    }
}

impl<T: ?Sized> core::ops::Deref for IrqGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized> core::ops::DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T: ?Sized> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        restore_interrupt_state(self.rflags);
    }
}

unsafe impl<T: ?Sized + Send> Send for IrqLock<T> {}
unsafe impl<T: ?Sized + Send> Sync for IrqLock<T> {}

#[inline]
pub fn save_interrupt_state() -> u64 {
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }
    rflags
}

#[inline]
pub fn restore_interrupt_state(rflags: u64) {
    if rflags & (1 << 9) != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    } else {
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
        }
    }
}

#[inline]
pub fn interrupts_enabled() -> bool {
    save_interrupt_state() & (1 << 9) != 0
}

// Hardware format for one IDT gate. The packed layout must match the x86-64
// descriptor format exactly; handler addresses are split across three fields.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    // A zeroed descriptor is a safe initial state until its handler is installed.
    const fn empty() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set_handler(&mut self, offset: u64) {
        self.offset_low = offset as u16;
        self.selector = 0x08;
        self.ist = 0;
        self.flags = 0x8E;
        self.offset_mid = (offset >> 16) as u16;
        self.offset_high = (offset >> 32) as u32;
        self.reserved = 0;
    }

    fn set_handler_with_ist(&mut self, offset: u64, ist: u8) {
        // IST values 1 through 7 select TSS IST1 through IST7; zero disables IST.
        self.set_handler(offset);
        self.ist = ist;
    }

    fn set_user_handler(&mut self, offset: u64) {
        // Ring 3 may enter this gate, but the target code remains in ring 0.
        self.set_handler(offset);
        self.flags = 0xEE;
    }
}

// Align the table as required by the processor when loading the IDTR.
#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    const fn new() -> Self {
        Self {
            entries: [IdtEntry::empty(); 256],
        }
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

static mut IDT: Idt = Idt::new();
// The timer interrupt only increments this counter; Relaxed ordering is enough
// because it does not publish or synchronize any other memory.
static TICKS: AtomicUsize = AtomicUsize::new(0);

/// The fixed 22-slot layout shared by assembly entry and Rust dispatch.
///
/// Assembly pushes R15 through RAX, then the synthetic vector/error slots,
/// followed by the processor IRET frame.  These fields intentionally mirror
/// that stack layout and must not be reordered.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub const INTERRUPT_FRAME_SIZE: usize = core::mem::size_of::<InterruptFrame>();

// Port I/O is privileged and is only called during kernel setup or IRQ handling.
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nostack, nomem));
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nostack, nomem));
    value
}

// Declare each assembly entry point with the same C ABI used by the stubs.
macro_rules! extern_stub {
    ($name:ident) => {
        extern "C" {
            fn $name();
        }
    };
}

extern_stub!(isr_stub_0);
extern_stub!(isr_stub_1);
extern_stub!(isr_stub_2);
extern_stub!(isr_stub_3);
extern_stub!(isr_stub_4);
extern_stub!(isr_stub_5);
extern_stub!(isr_stub_6);
extern_stub!(isr_stub_7);
extern_stub!(isr_stub_8);
extern_stub!(isr_stub_9);
extern_stub!(isr_stub_10);
extern_stub!(isr_stub_11);
extern_stub!(isr_stub_12);
extern_stub!(isr_stub_13);
extern_stub!(isr_stub_14);
extern_stub!(isr_stub_15);
extern_stub!(isr_stub_16);
extern_stub!(isr_stub_17);
extern_stub!(isr_stub_18);
extern_stub!(isr_stub_19);
extern_stub!(isr_stub_20);
extern_stub!(isr_stub_21);
extern_stub!(isr_stub_22);
extern_stub!(isr_stub_23);
extern_stub!(isr_stub_24);
extern_stub!(isr_stub_25);
extern_stub!(isr_stub_26);
extern_stub!(isr_stub_27);
extern_stub!(isr_stub_28);
extern_stub!(isr_stub_29);
extern_stub!(isr_stub_30);
extern_stub!(isr_stub_31);
extern_stub!(isr_stub_32);
extern_stub!(isr_stub_33);
extern_stub!(isr_stub_34);
extern_stub!(isr_stub_35);
extern_stub!(isr_stub_36);
extern_stub!(isr_stub_37);
extern_stub!(isr_stub_38);
extern_stub!(isr_stub_39);
extern_stub!(isr_stub_40);
extern_stub!(isr_stub_41);
extern_stub!(isr_stub_42);
extern_stub!(isr_stub_43);
extern_stub!(isr_stub_44);
extern_stub!(isr_stub_45);
extern_stub!(isr_stub_46);
extern_stub!(isr_stub_47);
extern_stub!(isr_stub_48);
extern_stub!(isr_stub_49);
extern_stub!(isr_stub_50);
extern_stub!(isr_stub_51);
extern_stub!(isr_stub_52);
extern_stub!(isr_stub_53);
extern_stub!(isr_stub_54);
extern_stub!(isr_stub_55);
extern_stub!(isr_stub_56);
extern_stub!(isr_stub_57);
extern_stub!(isr_stub_58);
extern_stub!(isr_stub_59);
extern_stub!(isr_stub_60);
extern_stub!(isr_stub_61);
extern_stub!(isr_stub_62);
extern_stub!(isr_stub_63);
extern_stub!(isr_stub_128);

macro_rules! stub_address {
    ($name:ident) => {
        $name as *const () as usize as u64
    };
}

/// Build and load the IDT, remap the legacy PIC, and start the PIT timer.
pub fn init() {
    unsafe {
        let idt = &raw mut IDT;
        // Vectors 0-63 have dedicated assembly stubs. The remaining entries
        // temporarily reuse the timer stub until exception handling is expanded.
        let handlers: [u64; 64] = [
            stub_address!(isr_stub_0),
            stub_address!(isr_stub_1),
            stub_address!(isr_stub_2),
            stub_address!(isr_stub_3),
            stub_address!(isr_stub_4),
            stub_address!(isr_stub_5),
            stub_address!(isr_stub_6),
            stub_address!(isr_stub_7),
            stub_address!(isr_stub_8),
            stub_address!(isr_stub_9),
            stub_address!(isr_stub_10),
            stub_address!(isr_stub_11),
            stub_address!(isr_stub_12),
            stub_address!(isr_stub_13),
            stub_address!(isr_stub_14),
            stub_address!(isr_stub_15),
            stub_address!(isr_stub_16),
            stub_address!(isr_stub_17),
            stub_address!(isr_stub_18),
            stub_address!(isr_stub_19),
            stub_address!(isr_stub_20),
            stub_address!(isr_stub_21),
            stub_address!(isr_stub_22),
            stub_address!(isr_stub_23),
            stub_address!(isr_stub_24),
            stub_address!(isr_stub_25),
            stub_address!(isr_stub_26),
            stub_address!(isr_stub_27),
            stub_address!(isr_stub_28),
            stub_address!(isr_stub_29),
            stub_address!(isr_stub_30),
            stub_address!(isr_stub_31),
            stub_address!(isr_stub_32),
            stub_address!(isr_stub_33),
            stub_address!(isr_stub_34),
            stub_address!(isr_stub_35),
            stub_address!(isr_stub_36),
            stub_address!(isr_stub_37),
            stub_address!(isr_stub_38),
            stub_address!(isr_stub_39),
            stub_address!(isr_stub_40),
            stub_address!(isr_stub_41),
            stub_address!(isr_stub_42),
            stub_address!(isr_stub_43),
            stub_address!(isr_stub_44),
            stub_address!(isr_stub_45),
            stub_address!(isr_stub_46),
            stub_address!(isr_stub_47),
            stub_address!(isr_stub_48),
            stub_address!(isr_stub_49),
            stub_address!(isr_stub_50),
            stub_address!(isr_stub_51),
            stub_address!(isr_stub_52),
            stub_address!(isr_stub_53),
            stub_address!(isr_stub_54),
            stub_address!(isr_stub_55),
            stub_address!(isr_stub_56),
            stub_address!(isr_stub_57),
            stub_address!(isr_stub_58),
            stub_address!(isr_stub_59),
            stub_address!(isr_stub_60),
            stub_address!(isr_stub_61),
            stub_address!(isr_stub_62),
            stub_address!(isr_stub_63),
        ];
        for (index, entry) in (*idt).entries.iter_mut().enumerate() {
            let offset = handlers.get(index).copied().unwrap_or(handlers[32]);
            if index == 8 {
                // Double fault has a dedicated IST stack so a compromised kernel
                // stack cannot prevent the processor from reporting the fault.
                entry.set_handler_with_ist(offset, 1);
            } else {
                entry.set_handler(offset);
            }
        }

        // Expose only the deliberately selected int 0x80 syscall gate to ring 3.
        (*idt).entries[128].set_user_handler(stub_address!(isr_stub_128));

        // Move IRQs away from CPU exception vectors, then program PIT channel 0
        // for approximately 1 kHz and unmask timer and keyboard IRQs.
        remap_pic();
        crate::drivers::DEVICE_REGISTRY.initialize("8259 PIC");
        crate::drivers::DEVICE_REGISTRY.activate("8259 PIC");
        // Device setup is delegated to the PIT driver; IRQ setup remains in
        // the interrupt controller subsystem.
        let mut pit = crate::drivers::Pit::new();
        match pit.initialize_result() {
            Ok(_) => {
                crate::drivers::DEVICE_REGISTRY.initialize("i8253 PIT");
                crate::drivers::DEVICE_REGISTRY.activate("i8253 PIT");
                let _ = crate::drivers::DEVICE_REGISTRY.register_interrupt(
                    "i8253 PIT",
                    32,
                    0,
                    crate::drivers::InterruptMode::Exclusive,
                    None,
                );
                crate::serial::println(format_args!(
                    "PIT initialization complete: channel0=~1000Hz"
                ));
            },
            Err(error) => crate::recovery::report_init_failure("PIT", error),
        }
        // Initialize PS/2 before enabling IRQ 1.  A timeout or controller
        // failure is reported but does not prevent the serial/text fallbacks.
        match crate::userspace::keyboard::init_result() {
            Ok(_) => {
                crate::drivers::DEVICE_REGISTRY.initialize("PS/2 keyboard");
                crate::drivers::DEVICE_REGISTRY.activate("PS/2 keyboard");
                let _ = crate::drivers::DEVICE_REGISTRY.register_interrupt(
                    "PS/2 keyboard",
                    33,
                    1,
                    crate::drivers::InterruptMode::Exclusive,
                    None,
                );
                crate::serial::println(format_args!("PS/2 keyboard initialization complete"));
            },
            Err(error) => crate::recovery::report_init_failure("PS/2 keyboard", error),
        }
        // IRQ0 and IRQ1 are the only mask bits enabled in this prototype.
        outb(0x21, inb(0x21) & !0x03);
        outb(0xA1, 0xFF);

        let pointer = IdtPointer {
            limit: (size_of::<Idt>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(nostack, preserves_flags));
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

// ICW values configure the master/slave PIC cascade and vector offsets. Save and
// restore masks so existing IRQ enablement is preserved across remapping.
unsafe fn remap_pic() {
    let mask1 = inb(0x21);
    let mask2 = inb(0xA1);
    outb(0x20, 0x11);
    outb(0xA0, 0x11);
    outb(0x21, 32);
    outb(0xA1, 40);
    outb(0x21, 0x04);
    outb(0xA1, 0x02);
    outb(0x21, 0x01);
    outb(0xA1, 0x01);
    outb(0x21, mask1);
    outb(0xA1, mask2);
}

const fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "Divide error",
        1 => "Debug",
        2 => "Non-maskable interrupt",
        3 => "Breakpoint",
        4 => "Overflow",
        5 => "Bound range exceeded",
        6 => "Invalid opcode",
        7 => "Device not available",
        8 => "Double fault",
        10 => "Invalid TSS",
        11 => "Segment not present",
        12 => "Stack-segment fault",
        13 => "General protection fault",
        14 => "Page fault",
        16 => "x87 floating-point exception",
        17 => "Alignment check",
        18 => "Machine check",
        19 => "SIMD floating-point exception",
        20 => "Virtualization exception",
        21 => "Control protection exception",
        28 => "Hypervisor injection exception",
        29 => "VMM communication exception",
        30 => "Security exception",
        _ => "Reserved exception",
    }
}

/// Send the correct legacy PIC acknowledgement for a hardware IRQ vector.
pub fn acknowledge_pic(vector: u64) {
    unsafe {
        if (40..48).contains(&vector) {
            // Check-in ISR to distinguish a real IRQ15 from a spurious IRQ15.
            if vector == 47 {
                outb(0xA0, 0x0B);
                if inb(0xA0) & 0x80 != 0 {
                    outb(0x20, 0x20);
                    return;
                }
            }
            outb(0xA0, 0x20);
            outb(0x20, 0x20);
        } else if (32..40).contains(&vector) {
            // IRQ7 can be spurious; only acknowledge it if its ISR bit is set.
            if vector == 39 {
                outb(0x20, 0x0B);
                if inb(0x20) & 0x80 == 0 {
                    return;
                }
            }
            outb(0x20, 0x20);
        }
    }
}

#[no_mangle]
pub extern "C" fn interrupt_dispatch(
    vector: u64,
    error_code: u64,
    cr2: u64,
    interrupt_rsp: *mut InterruptFrame,
) -> *mut u64 {
    let frame = unsafe { &mut *interrupt_rsp };
    let (task_id, _, _) = crate::scheduler::SCHEDULER.with(|scheduler| scheduler.diagnostic_task());
    // Publish interrupt ownership before dispatch.  Leave happens for all
    // paths that return to assembly; exception paths halt the CPU.
    crate::kernel::percpu::enter_interrupt(task_id);

    if vector == 128 {
        let syscall_start = crate::performance::timestamp();
        // The syscall frame follows the shared assembly register layout:
        // RAX, RBX, RCX, and RDX carry number and arguments; RAX receives result.
        let number = frame.rax as usize;
        let arg1 = frame.rbx as usize;
        let arg2 = frame.rcx as usize;
        let arg3 = frame.rdx as usize;
        let result = crate::userspace::syscalls::dispatch_args(number, arg1, arg2, arg3);
        crate::performance::record_syscall(syscall_start);
        frame.rax = result as u64;
        crate::kernel::percpu::leave_interrupt();
        return interrupt_rsp.cast::<u64>();
    }
    if vector < 32 {
        // Preserve a bounded diagnostic snapshot: register frame, fault address,
        // task identity, boot stage, and allocator state are all printable here.
        let rax = frame.rax;
        let rbx = frame.rbx;
        let rcx = frame.rcx;
        let rdx = frame.rdx;
        let rsi = frame.rsi;
        let rdi = frame.rdi;
        let rbp = frame.rbp;
        let rip = frame.rip;
        let rflags = frame.rflags;
        let rsp = frame.rsp;
        let (task_id, task_name, task_state) =
            crate::scheduler::SCHEDULER.with(|scheduler| scheduler.diagnostic_task());
        unsafe {
            core::arch::asm!("cli", options(nomem, nostack));
        }
        if vector == 14 {
            crate::serial::println(format_args!(
                "KERNEL EXCEPTION: vector {} ({}) error={:#x} CR2={:#x}",
                vector,
                exception_name(vector),
                error_code,
                cr2
            ));
            crate::println!(
                "KERNEL EXCEPTION: vector {} ({}) error={:#x} CR2={:#x}",
                vector,
                exception_name(vector),
                error_code,
                cr2
            );
        } else {
            crate::serial::println(format_args!(
                "KERNEL EXCEPTION: vector {} ({}) error={:#x}",
                vector,
                exception_name(vector),
                error_code
            ));
            crate::println!(
                "KERNEL EXCEPTION: vector {} ({}) error={:#x}",
                vector,
                exception_name(vector),
                error_code
            );
        }
        crate::serial::println(format_args!(
            "EXCEPTION CONTEXT: RIP={:#x} RSP={:#x} RFLAGS={:#x} RAX={:#x} RBX={:#x}",
            rip, rsp, rflags, rax, rbx
        ));
        crate::serial::println(format_args!(
            "EXCEPTION CONTEXT: RCX={:#x} RDX={:#x} RSI={:#x} RDI={:#x} RBP={:#x}",
            rcx, rdx, rsi, rdi, rbp
        ));
        crate::serial::println(format_args!(
            "EXCEPTION CONTEXT: stage={} task_id={} task={:?} name={} ticks={} memory={}KiB",
            crate::recovery::boot_stage().name(),
            task_id,
            task_state,
            task_name,
            tick_count(),
            crate::memory::allocated_bytes() / 1024
        ));
        crate::println!(
            "EXCEPTION CONTEXT: RIP={:#x} RSP={:#x} stage={} task={}",
            rip,
            rsp,
            crate::recovery::boot_stage().name(),
            task_name
        );
        loop {
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack));
            }
        }
    }

    // IRQ0 drives scheduler ticks; IRQ1 feeds the current shell directly. The
    // PIC must receive an EOI for handled hardware interrupts or it will not
    // deliver subsequent requests. A future design can queue events for tasks.
    if vector == 32 {
        let irq_start = crate::performance::timestamp();
        TICKS.fetch_add(1, Ordering::Relaxed);
        let switched_rsp = crate::scheduler::SCHEDULER
            .with(|scheduler| scheduler.tick_context(interrupt_rsp.cast::<u64>()));
        acknowledge_pic(vector);
        crate::performance::record_irq(irq_start);
        crate::kernel::percpu::leave_interrupt();
        switched_rsp
    } else if vector == 33 {
        // PS/2 IRQ work is bounded to reading and queueing a key event.
        crate::userspace::keyboard::handle_irq();
        acknowledge_pic(vector);
        crate::kernel::percpu::leave_interrupt();
        interrupt_rsp.cast::<u64>()
    } else if vector >= 32 {
        acknowledge_pic(vector);
        crate::kernel::percpu::leave_interrupt();
        interrupt_rsp.cast::<u64>()
    } else {
        interrupt_rsp.cast::<u64>()
    }
}

/// Return the number of timer interrupts observed since initialization.
pub fn tick_count() -> usize {
    TICKS.load(Ordering::Relaxed)
}
