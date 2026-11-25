//! Reusable hardware-driver primitives.
//!
//! Drivers keep port and device details below kernel subsystems.  A device
//! may expose one of the small traits below, while interrupt handlers should
//! do the minimum work required to notify or queue data.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::recovery::InitError;

/// Common status interface for device initialization and health reporting.
pub trait Device {
    fn name(&self) -> &'static str;
    fn initialize(&mut self) -> bool;
    fn ready(&self) -> bool;
}

/// Minimal byte-oriented x86 port I/O.
///
/// These helpers are kept in one place so drivers do not need duplicated
/// assembly blocks and can be audited without following terminal or IRQ code.
pub mod io {
    pub unsafe fn outb(port: u16, value: u8) {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nostack, nomem)
        );
    }

    pub unsafe fn inb(port: u16) -> u8 {
        let value: u8;
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nostack, nomem)
        );
        value
    }

    pub unsafe fn outw(port: u16, value: u16) {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nostack, nomem)
        );
    }

    pub unsafe fn inw(port: u16) -> u16 {
        let value: u16;
        core::arch::asm!(
            "in ax, dx",
            out("ax") value,
            in("dx") port,
            options(nostack, nomem)
        );
        value
    }
}

/// The legacy Programmable Interval Timer channel 0.
pub struct Pit {
    divisor: u16,
    initialized: AtomicBool,
}

impl Pit {
    pub const fn new() -> Self {
        Self {
            divisor: 1193,
            initialized: AtomicBool::new(false),
        }
    }

    /// Program channel 0 for approximately 1 kHz using mode 3.
    pub fn initialize(&mut self) {
        unsafe {
            io::outb(0x43, 0x36);
            io::outb(0x40, self.divisor as u8);
            io::outb(0x40, (self.divisor >> 8) as u8);
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// Program and read back the PIT command/divisor through bounded setup.
    /// The command write is validated by keeping setup bounded and publishing
    /// a typed failure instead of looping forever in a device driver.
    pub fn initialize_result(&mut self) -> Result<(), InitError> {
        if self.divisor == 0 {
            return Err(InitError::InvalidResponse);
        }
        for _ in 0..10_000 {
            unsafe {
                io::outb(0x43, 0x36);
                io::outb(0x40, self.divisor as u8);
                io::outb(0x40, (self.divisor >> 8) as u8);
            }
            self.initialized.store(true, Ordering::Release);
            return Ok(());
        }
        Err(InitError::TimedOut)
    }

    pub fn ticks_to_milliseconds(ticks: usize) -> usize {
        ticks
    }
}

impl Device for Pit {
    fn name(&self) -> &'static str {
        "i8253 PIT"
    }

    fn initialize(&mut self) -> bool {
        self.initialize();
        true
    }

    fn ready(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

/// Kernel-private text console rendered through VGA or the linear framebuffer.
pub(crate) struct VgaText {
    pub(crate) width: usize,
    pub(crate) height: usize,
    buffer: *mut u16,
}

/// Output modes that the kernel can currently select.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayMode {
    VgaText,
    Framebuffer,
}

impl DisplayMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::VgaText => "VGA text",
            Self::Framebuffer => "linear framebuffer",
        }
    }
}

const FRAMEBUFFER_ALIAS: usize = 0x4000_0000;
const FRAMEBUFFER_PAGE_MASK: usize = 0x1F_FFFF;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 8;
const FRAMEBUFFER_CELL_COUNT: usize = VgaText::MAX_WIDTH * VgaText::MAX_HEIGHT;

static FRAMEBUFFER_ENABLED: AtomicBool = AtomicBool::new(false);
static FRAMEBUFFER_BASE: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_PITCH: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_WIDTH: AtomicUsize = AtomicUsize::new(512);
static FRAMEBUFFER_HEIGHT: AtomicUsize = AtomicUsize::new(512);
static FRAMEBUFFER_RED_POSITION: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_GREEN_POSITION: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_BLUE_POSITION: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_CELLS: [AtomicUsize; FRAMEBUFFER_CELL_COUNT] =
    [const { AtomicUsize::new(0x0720) }; FRAMEBUFFER_CELL_COUNT];

/// Read-only display capabilities exposed to diagnostics and shell commands.
#[derive(Clone, Copy)]
pub struct DisplayInfo {
    pub mode: DisplayMode,
    pub width: usize,
    pub height: usize,
    pub graphics_enabled: bool,
}

/// Report the compiled-in display policy without exposing device operations.
pub fn display_info() -> DisplayInfo {
    if FRAMEBUFFER_ENABLED.load(Ordering::Acquire) {
        DisplayInfo {
            mode: DisplayMode::Framebuffer,
            width: FRAMEBUFFER_WIDTH.load(Ordering::Acquire),
            height: FRAMEBUFFER_HEIGHT.load(Ordering::Acquire),
            graphics_enabled: true,
        }
    } else {
        DisplayInfo {
            mode: DisplayMode::VgaText,
            width: VgaText::WIDTH,
            height: VgaText::HEIGHT,
            graphics_enabled: false,
        }
    }
}

/// Select the Multiboot v1 RGB framebuffer when it is available.
/// The legacy text console remains the fallback for unsupported modes.
pub fn init_display(multiboot_magic: u32, multiboot_info: usize) -> Result<(), InitError> {
    const MULTIBOOT_MAGIC: u32 = 0x2BAD_B002;
    if multiboot_magic != MULTIBOOT_MAGIC || multiboot_info == 0 {
        return Err(InitError::NotPresent);
    }

    let flags = unsafe { (multiboot_info as *const u32).read_unaligned() };
    if flags & (1 << 12) == 0 {
        return Err(InitError::NotPresent);
    }

    let address = unsafe { ((multiboot_info + 88) as *const u64).read_unaligned() };
    let pitch = unsafe { ((multiboot_info + 96) as *const u32).read_unaligned() as usize };
    let width = unsafe { ((multiboot_info + 100) as *const u32).read_unaligned() as usize };
    let height = unsafe { ((multiboot_info + 104) as *const u32).read_unaligned() as usize };
    let bits_per_pixel = unsafe { ((multiboot_info + 108) as *const u8).read_unaligned() };
    let framebuffer_type = unsafe { ((multiboot_info + 109) as *const u8).read_unaligned() };
    let red_position = unsafe { ((multiboot_info + 110) as *const u8).read_unaligned() as usize };
    let red_size = unsafe { ((multiboot_info + 111) as *const u8).read_unaligned() };
    let green_position = unsafe { ((multiboot_info + 112) as *const u8).read_unaligned() as usize };
    let green_size = unsafe { ((multiboot_info + 113) as *const u8).read_unaligned() };
    let blue_position = unsafe { ((multiboot_info + 114) as *const u8).read_unaligned() as usize };
    let blue_size = unsafe { ((multiboot_info + 115) as *const u8).read_unaligned() };
    let byte_length = pitch.checked_mul(height).ok_or(InitError::InvalidResponse)?;
    let page_offset = address as usize & FRAMEBUFFER_PAGE_MASK;

    crate::serial::println(format_args!(
        "Framebuffer candidate: address={:#x} {}x{} pitch={} bpp={} type={} rgb=R{}/{} G{}/{} B{}/{}",
        address,
        width,
        height,
        pitch,
        bits_per_pixel,
        framebuffer_type,
        red_position,
        red_size,
        green_position,
        green_size,
        blue_position,
        blue_size
    ));

    if address >> 32 != 0
        || width < CELL_WIDTH
        || height < CELL_HEIGHT
        || width / CELL_WIDTH > VgaText::MAX_WIDTH
        || height / CELL_HEIGHT > VgaText::MAX_HEIGHT
        || bits_per_pixel != 32
        || framebuffer_type != 1
        || pitch < width * 4
        || byte_length == 0
        || page_offset.checked_add(byte_length).filter(|length| *length <= 4 * 1024 * 1024).is_none()
        || [red_position, green_position, blue_position].iter().any(|position| *position > 24)
        // GRUB/QEMU can omit the red width for an 8-bit channel at bit zero.
        || (red_size != 8 && !(red_size == 0 && red_position == 0))
        || green_size != 8
        || blue_size != 8
        || red_position == green_position
        || red_position == blue_position
        || green_position == blue_position
    {
        return Err(InitError::Unsupported);
    }

    FRAMEBUFFER_BASE.store(FRAMEBUFFER_ALIAS + page_offset, Ordering::Release);
    FRAMEBUFFER_PITCH.store(pitch, Ordering::Release);
    FRAMEBUFFER_WIDTH.store(width, Ordering::Release);
    FRAMEBUFFER_HEIGHT.store(height, Ordering::Release);
    FRAMEBUFFER_RED_POSITION.store(red_position, Ordering::Release);
    FRAMEBUFFER_GREEN_POSITION.store(green_position, Ordering::Release);
    FRAMEBUFFER_BLUE_POSITION.store(blue_position, Ordering::Release);
    FRAMEBUFFER_ENABLED.store(true, Ordering::Release);
    Ok(())
}

impl VgaText {
    pub(crate) const WIDTH: usize = 80;
    pub(crate) const HEIGHT: usize = 25;
    pub(crate) const MAX_WIDTH: usize = 170;
    pub(crate) const MAX_HEIGHT: usize = 128;
    pub(crate) const CELL_COLOR: u16 = 0x07;

    pub(crate) fn new() -> Self {
        let framebuffer_enabled = FRAMEBUFFER_ENABLED.load(Ordering::Acquire);
        Self {
            width: if framebuffer_enabled {
                FRAMEBUFFER_WIDTH.load(Ordering::Acquire) / CELL_WIDTH
            } else {
                Self::WIDTH
            },
            height: if framebuffer_enabled {
                FRAMEBUFFER_HEIGHT.load(Ordering::Acquire) / CELL_HEIGHT
            } else {
                Self::HEIGHT
            },
            buffer: 0xB8000 as *mut u16,
        }
    }

    pub(crate) fn read_cell(&self, row: usize, column: usize) -> u16 {
        debug_assert!(row < self.height && column < self.width);
        if FRAMEBUFFER_ENABLED.load(Ordering::Acquire) {
            return FRAMEBUFFER_CELLS[row * self.width + column].load(Ordering::Acquire) as u16;
        }
        unsafe {
            self.buffer
                .add(row * self.width + column)
                .read_volatile()
        }
    }

    pub(crate) fn write_cell(&mut self, row: usize, column: usize, value: u16) {
        debug_assert!(row < self.height && column < self.width);
        if FRAMEBUFFER_ENABLED.load(Ordering::Acquire) {
            FRAMEBUFFER_CELLS[row * self.width + column].store(value as usize, Ordering::Release);
            draw_framebuffer_cell(row, column, value);
            return;
        }
        unsafe {
            self.buffer
                .add(row * self.width + column)
                .write_volatile(value);
        }
    }

    pub(crate) fn clear(&mut self) {
        if FRAMEBUFFER_ENABLED.load(Ordering::Acquire) {
            for row in 0..self.height {
                for column in 0..self.width {
                    self.write_cell(row, column, Self::CELL_COLOR << 8 | b' ' as u16);
                }
            }
            return;
        }
        for offset in 0..(self.width * self.height) {
            unsafe {
                self.buffer
                    .add(offset)
                    .write_volatile(Self::CELL_COLOR << 8 | b' ' as u16);
            }
        }
    }
}

fn vga_rgb(color: u8) -> (u8, u8, u8) {
    const PALETTE: [(u8, u8, u8); 16] = [
        (0, 0, 0), (0, 0, 170), (0, 170, 0), (0, 170, 170),
        (170, 0, 0), (170, 0, 170), (170, 85, 0), (170, 170, 170),
        (85, 85, 85), (85, 85, 255), (85, 255, 85), (85, 255, 255),
        (255, 85, 85), (255, 85, 255), (255, 255, 85), (255, 255, 255),
    ];
    PALETTE[(color & 0x0F) as usize]
}

fn framebuffer_pixel(x: usize, y: usize, rgb: (u8, u8, u8)) {
    let red_position = FRAMEBUFFER_RED_POSITION.load(Ordering::Relaxed);
    let green_position = FRAMEBUFFER_GREEN_POSITION.load(Ordering::Relaxed);
    let blue_position = FRAMEBUFFER_BLUE_POSITION.load(Ordering::Relaxed);
    let pixel = ((rgb.0 as u32) << red_position)
        | ((rgb.1 as u32) << green_position)
        | ((rgb.2 as u32) << blue_position);
    let address = FRAMEBUFFER_BASE.load(Ordering::Relaxed)
        + y * FRAMEBUFFER_PITCH.load(Ordering::Relaxed)
        + x * 4;
    unsafe {
        (address as *mut u32).write_volatile(pixel);
    }
}

fn draw_framebuffer_cell(row: usize, column: usize, cell: u16) {
    let character = (cell as u8).to_ascii_uppercase();
    let foreground = vga_rgb((cell >> 8) as u8);
    let background = vga_rgb((cell >> 12) as u8);
    let origin_x = column * CELL_WIDTH;
    let origin_y = row * CELL_HEIGHT;
    let rows = glyph(character);

    for y in 0..CELL_HEIGHT {
        for x in 0..CELL_WIDTH {
            let pixel = if y < 7 && x > 0 && x < 6 && rows[y] & (1 << (5 - x)) != 0 {
                foreground
            } else {
                background
            };
            framebuffer_pixel(origin_x + x, origin_y + y, pixel);
        }
    }
}

fn glyph(character: u8) -> [u8; 7] {
    match character {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 18, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'.' => [0, 0, 0, 0, 0, 12, 12],
        b',' => [0, 0, 0, 0, 0, 12, 8],
        b':' => [0, 12, 12, 0, 12, 12, 0],
        b';' => [0, 12, 12, 0, 12, 8, 0],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'_' => [0, 0, 0, 0, 0, 0, 31],
        b'=' => [0, 31, 0, 31, 0, 0, 0],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'/' => [1, 2, 2, 4, 8, 8, 16],
        b'\\' => [16, 8, 8, 4, 2, 2, 1],
        b'\'' => [12, 12, 4, 0, 0, 0, 0],
        b'"' => [10, 10, 10, 0, 0, 0, 0],
        b'!' => [4, 4, 4, 4, 4, 0, 4],
        b'?' => [14, 17, 1, 2, 4, 0, 4],
        b'(' => [2, 4, 8, 8, 8, 4, 2],
        b')' => [8, 4, 2, 2, 2, 4, 8],
        b'[' => [14, 8, 8, 8, 8, 8, 14],
        b']' => [14, 2, 2, 2, 2, 2, 14],
        b'<' => [2, 4, 8, 16, 8, 4, 2],
        b'>' => [8, 4, 2, 1, 2, 4, 8],
        b'*' => [0, 17, 10, 31, 10, 17, 0],
        b'%' => [17, 2, 4, 8, 16, 17, 0],
        b'#' => [10, 10, 31, 10, 31, 10, 10],
        b'$' => [4, 15, 20, 14, 5, 30, 4],
        b'@' => [14, 17, 23, 21, 23, 16, 14],
        b'|' => [4, 4, 4, 4, 4, 4, 4],
        b' ' => [0; 7],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}

impl Device for VgaText {
    fn name(&self) -> &'static str {
        "VGA text display"
    }

    fn initialize(&mut self) -> bool {
        self.clear();
        true
    }

    fn ready(&self) -> bool {
        true
    }
}

/// Polling ATA PIO driver for primary or secondary slave/master devices.
pub struct AtaPio {
    base: u16,
    slave: bool,
    ready: AtomicBool,
}

impl AtaPio {
    pub const fn primary_master() -> Self {
        Self {
            base: 0x1F0,
            slave: false,
            ready: AtomicBool::new(false),
        }
    }

    fn wait_status(&self, busy_clear: bool) -> bool {
        for _ in 0..20_000 {
            let status = unsafe { io::inb(self.base + 7) };
            if busy_clear && status & 0x80 == 0 {
                return status & 0x01 == 0;
            }
            if !busy_clear && status & 0x08 != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Return a typed initialization result for ATA PIO polling.
    pub fn initialize_result(&mut self) -> Result<(), InitError> {
        match self.identify() {
            Some(_) => Ok(()),
            None => {
                if unsafe { io::inb(self.base + 7) } == 0 {
                    Err(InitError::NotPresent)
                } else {
                    Err(InitError::TimedOut)
                }
            }
        }
    }

    pub fn identify(&mut self) -> Option<[u16; 256]> {
        unsafe {
            io::outb(self.base + 6, if self.slave { 0xB0 } else { 0xA0 });
            io::outb(self.base + 2, 0);
            io::outb(self.base + 3, 0);
            io::outb(self.base + 4, 0);
            io::outb(self.base + 5, 0);
            io::outb(self.base + 7, 0xEC);
        }
        let status = unsafe { io::inb(self.base + 7) };
        if status == 0 {
            return None;
        }
        if !self.wait_status(false) {
            return None;
        }
        let mut identity = [0u16; 256];
        for word in &mut identity {
            *word = unsafe { io::inw(self.base) };
        }
        self.ready.store(true, Ordering::Release);
        Some(identity)
    }

    pub fn read_sector(&mut self, lba: u32) -> Option<[u8; 512]> {
        if !self.ready.load(Ordering::Acquire) {
            return None;
        }
        unsafe {
            io::outb(self.base + 6, 0xE0 | (if self.slave { 0x10 } else { 0 } )
                | ((lba >> 24) & 0x0F) as u8);
            io::outb(self.base + 2, 1);
            io::outb(self.base + 3, lba as u8);
            io::outb(self.base + 4, (lba >> 8) as u8);
            io::outb(self.base + 5, (lba >> 16) as u8);
            io::outb(self.base + 7, 0x20);
        }
        if !self.wait_status(false) {
            return None;
        }
        let mut sector = [0u8; 512];
        for chunk in sector.chunks_exact_mut(2) {
            let word = unsafe { io::inw(self.base) };
            chunk[0] = word as u8;
            chunk[1] = (word >> 8) as u8;
        }
        Some(sector)
    }
}

impl Device for AtaPio {
    fn name(&self) -> &'static str {
        "ATA PIO storage"
    }

    fn initialize(&mut self) -> bool {
        self.identify().is_some()
    }

    fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}
