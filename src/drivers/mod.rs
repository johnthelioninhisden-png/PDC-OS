//! Reusable hardware-driver primitives.
//!
//! Drivers keep port and device details below kernel subsystems.  A device
//! may expose one of the small traits below, while interrupt handlers should
//! do the minimum work required to notify or queue data.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::recovery::InitError;
use crate::interrupts::IrqLock;

const DEVICE_SLOTS: usize = 16;
const RESOURCE_SLOTS: usize = 4;
const INTERRUPT_SLOTS: usize = 16;

#[derive(Clone, Copy)]
pub struct DeviceRegistration {
    pub name: &'static str,
    pub state: DeviceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Empty,
    Discovered,
    Registered,
    Initialized,
    Active,
    Removed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Platform,
    Legacy,
    Storage,
    Display,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareResource {
    PortRange { base: u16, length: u16 },
    Irq { line: u8 },
    MemoryRange { base: usize, length: usize },
}

#[derive(Clone, Copy)]
pub struct DeviceDescriptor {
    pub name: &'static str,
    pub class: DeviceClass,
    pub resources: [HardwareResource; RESOURCE_SLOTS],
    pub resource_count: usize,
}

impl DeviceDescriptor {
    pub const fn new(name: &'static str, class: DeviceClass) -> Self {
        Self {
            name,
            class,
            resources: [HardwareResource::PortRange { base: 0, length: 0 }; RESOURCE_SLOTS],
            resource_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptMode {
    Exclusive,
    Shared,
}

pub type InterruptHandler = fn() -> bool;

#[derive(Clone, Copy)]
pub struct InterruptRegistration {
    pub owner: &'static str,
    pub vector: u8,
    pub irq: u8,
    pub mode: InterruptMode,
    pub handler: Option<InterruptHandler>,
}

impl InterruptRegistration {
    pub const fn empty() -> Self {
        Self {
            owner: "",
            vector: 0,
            irq: 0,
            mode: InterruptMode::Exclusive,
            handler: None,
        }
    }
}

pub struct DeviceRegistry {
    slots: IrqLock<[DeviceRegistration; DEVICE_SLOTS]>,
    descriptors: IrqLock<[Option<DeviceDescriptor>; DEVICE_SLOTS]>,
    interrupts: IrqLock<[Option<InterruptRegistration>; INTERRUPT_SLOTS]>,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        const EMPTY: DeviceRegistration = DeviceRegistration {
            name: "",
            state: DeviceState::Empty,
        };
        Self {
            slots: IrqLock::new([EMPTY; DEVICE_SLOTS]),
            descriptors: IrqLock::new([None; DEVICE_SLOTS]),
            interrupts: IrqLock::new([None; INTERRUPT_SLOTS]),
        }
    }

    /// Discover a device by publishing a typed descriptor only.
    /// Resource claims are checked here, before a driver acquires hardware.
    pub fn register_descriptor(&self, descriptor: DeviceDescriptor) -> bool {
        let mut slots = self.slots.lock();
        let mut descriptors = self.descriptors.lock();

        if slots.iter().any(|slot| slot.name == descriptor.name
            && slot.state != DeviceState::Empty
            && slot.state != DeviceState::Removed)
        {
            return false;
        }

        for resource in descriptor.resources.iter().take(descriptor.resource_count) {
            for existing in descriptors.iter().flatten() {
                if Self::resources_overlap(*resource, existing.resources, existing.resource_count) {
                    return false;
                }
            }
        }

        for index in 0..DEVICE_SLOTS {
            if slots[index].state == DeviceState::Empty
                || slots[index].state == DeviceState::Removed
            {
                slots[index] = DeviceRegistration {
                    name: descriptor.name,
                    state: DeviceState::Discovered,
                };
                descriptors[index] = Some(descriptor);
                return true;
            }
        }
        false
    }

    /// Compatibility registration for callers that have not yet published resources.
    pub fn register(&self, name: &'static str) -> bool {
        let descriptor = DeviceDescriptor::new(name, DeviceClass::Platform);
        self.register_descriptor(descriptor)
    }

    fn resources_overlap(
        candidate: HardwareResource,
        existing: [HardwareResource; RESOURCE_SLOTS],
        existing_count: usize,
    ) -> bool {
        for resource in existing.iter().take(existing_count) {
            let overlaps = match (candidate, *resource) {
                (HardwareResource::PortRange { base: a, length: la },
                 HardwareResource::PortRange { base: b, length: lb }) => {
                    let a_end = a as usize + la as usize;
                    let b_end = b as usize + lb as usize;
                    (a as usize) < b_end && (b as usize) < a_end
                }
                (HardwareResource::Irq { line: a }, HardwareResource::Irq { line: b }) => a == b,
                (HardwareResource::MemoryRange { base: a, length: la },
                 HardwareResource::MemoryRange { base: b, length: lb }) => {
                    let a_end = a.saturating_add(la);
                    let b_end = b.saturating_add(lb);
                    a < b_end && b < a_end
                }
                _ => false,
            };
            if overlaps {
                return true;
            }
        }
        false
    }

    /// Mark discovery complete and publish a driver as ready to initialize.
    pub fn initialize(&self, name: &str) -> bool {
        let mut slots = self.slots.lock();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.name == name) {
            slot.state = DeviceState::Initialized;
            true
        } else {
            false
        }
    }

    pub fn activate(&self, name: &str) -> bool {
        let mut slots = self.slots.lock();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.name == name) {
            if slot.state == DeviceState::Initialized {
                slot.state = DeviceState::Active;
                return true;
            }
        }
        false
    }

    pub fn descriptor(&self, name: &str) -> Option<DeviceDescriptor> {
        let slots = self.slots.lock();
        let descriptors = self.descriptors.lock();
        for (index, slot) in slots.iter().enumerate() {
            if slot.name == name
                && matches!(slot.state, DeviceState::Discovered
                    | DeviceState::Registered
                    | DeviceState::Initialized
                    | DeviceState::Active)
            {
                return descriptors[index];
            }
        }
        None
    }

    pub fn register_interrupt(
        &self,
        owner: &'static str,
        vector: u8,
        irq: u8,
        mode: InterruptMode,
        handler: Option<InterruptHandler>,
    ) -> bool {
        let mut interrupts = self.interrupts.lock();
        if interrupts.iter().any(|entry| {
            entry.map_or(false, |registration| {
                registration.vector == vector
                    && registration.mode == InterruptMode::Exclusive
            })
        }) {
            return false;
        }
        for entry in interrupts.iter_mut() {
            if entry.is_none() {
                *entry = Some(InterruptRegistration {
                    owner,
                    vector,
                    irq,
                    mode,
                    handler,
                });
                return true;
            }
        }
        false
    }

    pub fn unregister_interrupt(&self, owner: &str, vector: u8) -> bool {
        let mut interrupts = self.interrupts.lock();
        for entry in interrupts.iter_mut() {
            if entry.map_or(false, |registration| {
                registration.owner == owner && registration.vector == vector
            }) {
                *entry = None;
                return true;
            }
        }
        false
    }

    /// Dispatch a device interrupt only when a registered handler exists.
    pub fn dispatch_interrupt(&self, vector: u8) -> bool {
        let interrupts = self.interrupts.lock();
        if let Some(registration) = interrupts.iter().flatten().find(|registration| {
            registration.vector == vector
        }) {
            if let Some(handler) = registration.handler {
                return handler();
            }
            return true;
        }
        false
    }

    /// Begin removal: release descriptor resources and IRQ registrations,
    /// then publish the slot as removed.
    pub fn remove(&self, name: &str) -> bool {
        let mut slots = self.slots.lock();
        let mut descriptors = self.descriptors.lock();
        let mut interrupts = self.interrupts.lock();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.name == name) {
            for descriptor in descriptors.iter_mut() {
                if descriptor.map_or(false, |value| value.name == name) {
                    *descriptor = None;
                }
            }
            for entry in interrupts.iter_mut() {
                if entry.map_or(false, |registration| registration.owner == name) {
                    *entry = None;
                }
            }
            *slot = DeviceRegistration {
                name: "",
                state: DeviceState::Removed,
            };
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.slots.lock()
            .iter()
            .filter(|slot| matches!(slot.state,
                DeviceState::Discovered
                | DeviceState::Registered
                | DeviceState::Initialized
                | DeviceState::Active))
            .count()
    }
}

pub static DEVICE_REGISTRY: DeviceRegistry = DeviceRegistry::new();

fn descriptor_with_port(
    name: &'static str,
    class: DeviceClass,
    base: u16,
    length: u16,
    irq: Option<u8>,
) -> DeviceDescriptor {
    let mut descriptor = DeviceDescriptor::new(name, class);
    descriptor.resources[0] = HardwareResource::PortRange { base, length };
    descriptor.resource_count = 1;
    if let Some(line) = irq {
        if descriptor.resource_count < RESOURCE_SLOTS {
            descriptor.resources[descriptor.resource_count] =
                HardwareResource::Irq { line };
            descriptor.resource_count += 1;
        }
    }
    descriptor
}

/// Hardware discovery only publishes device descriptions and claims.
/// Driver construction and initialization remain responsibilities of the
/// corresponding implementation module.
pub fn discover_legacy_devices() -> usize {
    let discovered = [
        descriptor_with_port("COM1 serial", DeviceClass::Platform, 0x3F8, 8, Some(4)),
        descriptor_with_port("i8253 PIT", DeviceClass::Platform, 0x40, 4, Some(0)),
        descriptor_with_port("8259 PIC", DeviceClass::Platform, 0x20, 2, None),
        descriptor_with_port("PS/2 keyboard", DeviceClass::Input, 0x60, 5, Some(1)),
        descriptor_with_port("VGA text", DeviceClass::Display, 0x3D4, 12, None),
    ];
    let mut count = 0;
    for descriptor in discovered.iter().copied() {
        if DEVICE_REGISTRY.register_descriptor(descriptor) {
            count += 1;
        }
    }
    count
}

/// Common status interface for device initialization and health reporting.
pub trait Device {
    fn name(&self) -> &'static str;
    fn initialize(&mut self) -> bool;
    fn shutdown(&mut self) -> bool {
        // Drivers without additional teardown can safely use this default.
        true
    }
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
    crate::drivers::DEVICE_REGISTRY.initialize("VGA text");
    crate::drivers::DEVICE_REGISTRY.activate("VGA text");
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

    fn shutdown(&mut self) -> bool {
        // PIO mode has no reset/register teardown; stop accepting reads and
        // let the registry revoke the port and interrupt ownership.
        self.ready.store(false, Ordering::Release);
        true
    }

    fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}
