//! PDC-OS monolithic kernel core.
//!
//! This crate is intentionally freestanding.  The Rust standard library is
//! not available while the kernel is booting.

#![no_std]
#![no_main]

use core::fmt;
use core::panic::PanicInfo;
use crate::hal::CpuMemoryProtection;

// Keep kernel subsystems as separate modules so hardware setup, scheduling,
// diagnostics, and the early userspace can evolve independently.
pub mod serial;
pub mod drivers;
pub mod fs;
pub mod hardware;
pub mod hal;
pub mod arch;
pub mod kernel;
pub mod userspace;

pub use kernel::{
    interrupts,
    memory,
    scheduler,
    recovery,
    performance,
    security,
};
pub use userspace as os;

const VGA_WIDTH: usize = drivers::VgaText::MAX_WIDTH;
const VGA_HEIGHT: usize = drivers::VgaText::MAX_HEIGHT;
const VGA_COLOR: u8 = drivers::VgaText::CELL_COLOR as u8;
const SCROLLBACK_LINES: usize = 512;
const BLANK_CELL: u16 = (VGA_COLOR as u16) << 8 | b' ' as u16;

// Tracks the cursor for the kernel text-console writer.
struct Writer {
    row: usize,
    column: usize,
    color: u8,
    scrollback: [[u16; VGA_WIDTH]; SCROLLBACK_LINES],
    scrollback_count: usize,
    scrollback_head: usize,
    scroll_offset: usize,
    live_screen: [[u16; VGA_WIDTH]; VGA_HEIGHT],
    viewing_scrollback: bool,
}

impl Writer {
    pub(crate) fn clear_screen(&mut self) {
        // Delegate hardware clearing to the reusable VGA display driver.
        crate::drivers::VgaText::new().clear();
        self.row = 0;
        self.column = 0;
        self.color = VGA_COLOR;
        self.scrollback_count = 0;
        self.scrollback_head = 0;
        self.scroll_offset = 0;
        self.viewing_scrollback = false;
    }

    fn set_color(&mut self, color: u8) {
        self.color = color;
    }

    fn capture_live_screen(&mut self) {
        let display = crate::drivers::VgaText::new();
        for row in 0..display.height {
            for column in 0..display.width {
                self.live_screen[row][column] = display.read_cell(row, column);
            }
        }
        self.viewing_scrollback = true;
    }

    fn restore_live_screen(&mut self) {
        let mut display = crate::drivers::VgaText::new();
        for row in 0..display.height {
            for column in 0..display.width {
                display.write_cell(row, column, self.live_screen[row][column]);
            }
        }
        self.viewing_scrollback = false;
        self.scroll_offset = 0;
    }

    fn render_scrollback(&mut self) {
        let mut display = crate::drivers::VgaText::new();
        let display_height = display.height;
        let display_width = display.width;
        let first_line = self.scrollback_count.saturating_sub(self.scroll_offset);
        for screen_row in 0..display_height {
            let line = first_line + screen_row;
            for column in 0..display_width {
                let cell = if line < self.scrollback_count {
                    let oldest = (self.scrollback_head + SCROLLBACK_LINES - self.scrollback_count)
                        % SCROLLBACK_LINES;
                    self.scrollback[(oldest + line) % SCROLLBACK_LINES][column]
                } else {
                    self.live_screen[line - self.scrollback_count][column]
                };
                display.write_cell(screen_row, column, cell);
            }
        }
    }

    fn scroll_page_up(&mut self) {
        if self.scrollback_count == 0 || self.scroll_offset == self.scrollback_count {
            return;
        }
        if !self.viewing_scrollback {
            self.capture_live_screen();
        }
        let page_size = crate::drivers::VgaText::new()
            .height
            .saturating_sub(1)
            .max(1);
        self.scroll_offset = (self.scroll_offset + page_size).min(self.scrollback_count);
        self.render_scrollback();
    }

    fn scroll_page_down(&mut self) {
        if self.scroll_offset == 0 {
            return;
        }
        let page_size = crate::drivers::VgaText::new()
            .height
            .saturating_sub(1)
            .max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
        if self.scroll_offset == 0 {
            self.restore_live_screen();
        } else {
            self.render_scrollback();
        }
    }

    fn follow_live_output(&mut self) {
        if self.viewing_scrollback {
            self.restore_live_screen();
        }
    }

    fn newline(&mut self) {
        let display_height = crate::drivers::VgaText::new().height;
        self.row += 1;
        self.column = 0;
        if self.row >= display_height {
            self.scroll();
        }
    }

    fn scroll(&mut self) {
        // Move each visible row upward, then blank the final row.
        let mut display = crate::drivers::VgaText::new();
        for column in 0..display.width {
            self.scrollback[self.scrollback_head][column] = display.read_cell(0, column);
        }
        self.scrollback_head = (self.scrollback_head + 1) % SCROLLBACK_LINES;
        self.scrollback_count = (self.scrollback_count + 1).min(SCROLLBACK_LINES);
        for row in 1..display.height {
            for column in 0..display.width {
                let value = display.read_cell(row, column);
                display.write_cell(row - 1, column, value);
            }
        }
        self.row = display.height - 1;
        for column in 0..display.width {
            display.write_cell(self.row, column, (self.color as u16) << 8 | b' ' as u16);
        }
    }

    fn write_byte(&mut self, byte: u8) {
        self.follow_live_output();
        // Handle terminal control bytes before writing printable byte values.
        match byte {
            b'\n' => self.newline(),
            b'\r' => {
                // Carriage return is useful to redraw a prompt or line in place.
                self.column = 0;
            }
            b'\t' => {
                // Use fixed eight-column tab stops without allocating or parsing.
                self.column = (self.column + 8) & !7;
                if self.column >= crate::drivers::VgaText::new().width {
                    self.newline();
                }
            }
            0x08 => {
                // A backspace moves one column left; the terminal writes a
                // space after it, which erases the character from VGA memory.
                if self.column > 0 {
                    self.column -= 1;
                } else if self.row > 0 {
                    // Crossing an automatic wrap boundary must move to the final
                    // column of the previous row instead of staying at column zero.
                    self.row -= 1;
                    self.column = crate::drivers::VgaText::new().width - 1;
                }
            }
            byte => {
                let mut display = crate::drivers::VgaText::new();
                if self.column >= display.width {
                    self.newline();
                }
                display.write_cell(
                    self.row,
                    self.column,
                    (self.color as u16) << 8 | byte as u16,
                );
                self.column += 1;
            }
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

static WRITER: interrupts::IrqLock<Writer> = interrupts::IrqLock::new(Writer {
    row: 0,
    column: 0,
    color: VGA_COLOR,
    scrollback: [[BLANK_CELL; VGA_WIDTH]; SCROLLBACK_LINES],
    scrollback_count: 0,
    scrollback_head: 0,
    scroll_offset: 0,
    live_screen: [[BLANK_CELL; VGA_WIDTH]; VGA_HEIGHT],
    viewing_scrollback: false,
});

pub(crate) fn set_vga_color(color: u8) {
    WRITER.lock().set_color(color);
}

pub(crate) fn scrollback_page_up() {
    WRITER.lock().scroll_page_up();
}

pub(crate) fn scrollback_page_down() {
    WRITER.lock().scroll_page_down();
}

fn print(args: fmt::Arguments) {
    use fmt::Write;
    let output_start = crate::performance::timestamp();
    let mut writer = WRITER.lock();
    let _ = writer.write_fmt(args);
    crate::performance::record_vga_output(1, output_start);
}

// Formatting-compatible wrappers let kernel code use familiar print syntax
// without bringing in std or allocating a temporary String.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::print(format_args!("{}\n", format_args!($($arg)*))));
}

/// First entry point called by the assembly bootstrap.
#[no_mangle]
pub extern "C" fn kernel_main(multiboot_magic: u64, multiboot_info: u64) -> ! {
    let serial_ready = serial::init();
    recovery::set_boot_stage(recovery::BootStage::Serial);
    serial::println(format_args!("PDC-OS kernel entry"));
    recovery::set_boot_stage(recovery::BootStage::Cpu);
    let cpu = arch::x86_64::detect();
    serial::println(format_args!(
        "Hardware: CPU vendor={} PAE={} long-mode={} COM1={}",
        cpu.vendor(),
        if cpu.pae { "yes" } else { "no" },
        if cpu.long_mode { "yes" } else { "no" },
        if serial_ready { "ready" } else { "unavailable" }
    ));
    serial::println(format_args!(
        "Hardware: interrupt-controller=legacy-PIC ATA=auto-detect PS/2=auto-detect"
    ));
    let mut cpu_memory = crate::arch::x86_64::X86_64;
    let nxe = cpu_memory.enable_no_execute();
    let write_protection = cpu_memory.enable_write_protection();
    serial::println(format_args!(
        "CPU memory protection: NXE={} CR0.WP={}",
        if nxe { "enabled" } else { "unavailable" },
        if write_protection { "enabled" } else { "unavailable" }
    ));
    if let Err(error) = drivers::init_display(multiboot_magic as u32, multiboot_info as usize) {
        serial::println(format_args!(
            "Display initialization: {} unavailable; falling back to VGA text",
            error.name()
        ));
    }
    WRITER.lock().clear_screen();
    let display = drivers::display_info();
    serial::println(format_args!(
        "Display mode: {} ({}x{}), framebuffer {}",
        display.mode.name(),
        display.width,
        display.height,
        if display.graphics_enabled {
            "enabled"
        } else {
            "disabled"
        }
    ));
    println!("PDC-OS monolithic kernel");
    println!(
        "Display: {} {}x{}; framebuffer {}",
        display.mode.name(),
        display.width,
        display.height,
        if display.graphics_enabled { "enabled" } else { "disabled" }
    );
    println!("Booted in 64-bit long mode");
    println!("Rust core is running");
    serial::println(format_args!("Initializing memory and scheduler"));
    recovery::set_boot_stage(recovery::BootStage::Memory);

    // Initialize subsystems in dependency order: allocator and task table first,
    // then interrupts, and finally the interactive terminal.
    // Pass the bootloader information to the memory subsystem so usable regions
    // come from the firmware map rather than a fixed 16 MiB assumption.
    if let Err(error) = memory::init_result(multiboot_magic as u32, multiboot_info as usize) {
        recovery::report_init_failure("memory", error);
    }
    let invariant_failures = kernel::run_invariants(recovery::BootStage::Memory);
    serial::println(format_args!(
        "Kernel invariants: checks={} failures={}",
        kernel::invariant_checks(),
        invariant_failures
    ));
    recovery::set_boot_stage(recovery::BootStage::Scheduler);
    scheduler::SCHEDULER.with(|scheduler| {
        scheduler.spawn(1, "kernel/init");
        scheduler.spawn(2, "kernel/idle");
        scheduler.spawn(3, "kernel/monitor");
    });
    userspace::services::init();
    userspace::syscalls::init();
    recovery::set_boot_stage(recovery::BootStage::Filesystem);
    fs::init();
    recovery::set_boot_stage(recovery::BootStage::Userspace);
    userspace::userspace::init();
    let security_report = security::run_adversarial_tests();
    serial::println(format_args!(
        "Security review: 10.7 cases={} passed={} failures={}",
        security_report.cases,
        security_report.passed,
        security_report.failed()
    ));

    println!(
        "Memory: {} KiB usable, {} KiB allocated",
        memory::usable_bytes() / 1024,
        memory::allocated_bytes() / 1024
    );
    serial::println(format_args!(
        "Memory: usable={} KiB allocated={} KiB map-regions={}",
        memory::usable_bytes() / 1024,
        memory::allocated_bytes() / 1024,
        memory::memory_map_region_count()
    ));
    println!("Scheduler: {} tasks ready", scheduler::SCHEDULER.with(|s| s.task_count()));
    serial::println(format_args!("Memory and scheduler initialized"));
    println!("Interrupt table and PIT initialization...");
    recovery::set_boot_stage(recovery::BootStage::Interrupts);
    interrupts::init();
    println!("PIT timer running");
    serial::println(format_args!("Interrupts and PIT initialized"));
    userspace::terminal::init();
    recovery::set_boot_stage(recovery::BootStage::Terminal);
    println!();
    let boot_perf = performance::snapshot();
    serial::println(format_args!(
        "Performance: context-switches={} irq-events={} syscalls={} disk-hits={} disk-misses={} serial-bytes={} vga-bytes={} frame-allocs={} frame-reclaims={} page-switches={}",
        boot_perf.context_switches,
        boot_perf.irq_events,
        boot_perf.syscalls,
        boot_perf.disk_cache_hits,
        boot_perf.disk_cache_misses,
        boot_perf.serial_bytes,
        boot_perf.vga_bytes,
        memory::frame_allocations(),
        memory::frame_reclaims(),
        memory::page_table_switches()
    ));
    serial::println(format_args!("PDC-OS boot complete"));
    recovery::set_boot_stage(recovery::BootStage::Complete);

    loop {
        // Drain hardware events outside IRQ context; keyboard input is delivered
        // through a queue rather than processed by the interrupt handler.
        userspace::terminal::poll_keyboard();
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    println!("KERNEL PANIC: {}", info);
    serial::println(format_args!("KERNEL PANIC: {}", info));
    recovery::report_system_context("KERNEL PANIC:");
    println!(
        "PANIC CONTEXT: stage={} memory={}KiB allocated",
        recovery::boot_stage().name(),
        memory::allocated_bytes() / 1024
    );
    serial::println(format_args!(
        "PANIC CONTEXT: stage={} usable={}KiB allocated={}KiB",
        recovery::boot_stage().name(),
        memory::usable_bytes() / 1024,
        memory::allocated_bytes() / 1024
    ));
    recovery::halt("unrecoverable kernel panic");
}
