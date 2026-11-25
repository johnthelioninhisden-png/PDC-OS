//! PS/2 Set-1 keyboard decoder for the kernel console.
//!
//! IRQ 1 only reads a byte, decodes it through fixed tables, and appends a
//! bounded key event.  Formatting and console scheduling happen outside IRQ
//! context.  The state here is intentionally allocation-free.

use crate::interrupts::IrqLock;
use crate::recovery::InitError;
use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
pub enum Key {
    Character(u8),
    Backspace,
    Enter,
    Escape,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    ControlC,
    ControlL,
    Release,
    Unknown,
}

const EVENT_CAPACITY: usize = 64;

/// Available fixed keyboard layouts.
#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    Us,
    Dvorak,
}

static QUEUE_OVERFLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
static QUEUE_WAKE_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static DEVICE_LOST: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Fixed ring buffer used at IRQ time; it never allocates or formats.
struct KeyQueue {
    events: [Key; EVENT_CAPACITY],
    read: usize,
    write: usize,
    count: AtomicUsize,
}

impl KeyQueue {
    const fn new() -> Self {
        Self {
            events: [Key::Unknown; EVENT_CAPACITY],
            read: 0,
            write: 0,
            count: AtomicUsize::new(0),
        }
    }

    fn push(&mut self, key: Key) -> bool {
        let count = self.count.load(Ordering::Relaxed);
        if count >= EVENT_CAPACITY {
            // A dropped event is counted separately instead of hiding console
            // pressure behind a successful but incomplete queue operation.
            QUEUE_OVERFLOW_COUNT.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.events[self.write] = key;
        self.write = (self.write + 1) % EVENT_CAPACITY;
        self.count.fetch_add(1, Ordering::Release);
        QUEUE_WAKE_PENDING.store(true, Ordering::Release);
        true
    }

    fn pop(&mut self) -> Option<Key> {
        let count = self.count.load(Ordering::Acquire);
        if count == 0 {
            return None;
        }
        let key = self.events[self.read];
        self.read = (self.read + 1) % EVENT_CAPACITY;
        self.count.fetch_sub(1, Ordering::AcqRel);
        Some(key)
    }
}

static KEY_QUEUE: IrqLock<KeyQueue> = IrqLock::new(KeyQueue::new());

/// Read and decode one PS/2 scancode and publish a queue event.
pub fn handle_irq() {
    let scancode = read_scancode();
    // Controller and keyboard status bytes are not printable key events.
    match scancode {
        0x00 => {
            DEVICE_LOST.store(true, Ordering::Release);
            return;
        }
        0xAA => {
            READY.store(true, Ordering::Release);
            DEVICE_LOST.store(false, Ordering::Release);
        }
        0xFC | 0xFD | 0xFF => {
            DEVICE_LOST.store(true, Ordering::Release);
        }
        _ => {}
    }
    let key = decode(scancode);
    if !matches!(key, Key::Release | Key::Unknown) {
        KEY_QUEUE.lock().push(key);
    }
}

/// Remove the next decoded key event for normal task/terminal processing.
pub fn try_pop() -> Option<Key> {
    KEY_QUEUE.lock().pop()
}

/// Mark that a consumer has observed queued input.  This is deliberately
/// separate from the ring count so wake-up reporting remains useful.
pub fn take_wake_pending() -> bool {
    QUEUE_WAKE_PENDING.swap(false, Ordering::AcqRel)
}

/// Return the number of events omitted because the fixed IRQ queue was full.
pub fn overflow_count() -> usize {
    QUEUE_OVERFLOW_COUNT.load(Ordering::Relaxed)
}

/// Report whether the keyboard device appears to have been lost or reset.
pub fn device_lost() -> bool {
    DEVICE_LOST.load(Ordering::Acquire)
}

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub fn queue_len() -> usize {
    KEY_QUEUE.lock().count.load(Ordering::Acquire)
}

struct ModifierState {
    shift: bool,
    extended: bool,
    control: bool,
    alt: bool,
    caps_lock: bool,
    num_lock: bool,
    layout: Layout,
}

// IRQ-safe state because a hardware stream can otherwise be decoded concurrently.
static MODIFIERS: IrqLock<ModifierState> = IrqLock::new(ModifierState {
    shift: false,
    extended: false,
    control: false,
    alt: false,
    caps_lock: false,
    num_lock: false,
    layout: Layout::Us,
});

/// Select a supported set-1 key layout at runtime.
pub fn set_layout(layout: Layout) {
    MODIFIERS.lock().layout = layout;
}

pub fn layout() -> Layout {
    MODIFIERS.lock().layout
}

pub fn caps_lock() -> bool {
    MODIFIERS.lock().caps_lock
}

pub fn num_lock() -> bool {
    MODIFIERS.lock().num_lock
}

/// Initialize the legacy PS/2 controller and keyboard with bounded waits.
/// Controller commands and reset replies are consumed only here; IRQ 1 remains
/// responsible only for fast scancode publication.
pub fn init() -> bool {
    init_result().is_ok()
}

/// Initialize PS/2 with bounded polls and a typed failure result.
pub fn init_result() -> Result<(), InitError> {
    unsafe {
        // Discard bytes left by firmware before changing controller state.
        for _ in 0..64 {
            if crate::drivers::io::inb(0x64) & 0x01 == 0 {
                break;
            }
            let _ = crate::drivers::io::inb(0x60);
        }

        crate::drivers::io::outb(0x64, 0xAD); // Disable PS/2 port 1.
        crate::drivers::io::outb(0x64, 0xAA); // Controller self-test.
        if !wait_for_data() || crate::drivers::io::inb(0x60) != 0x55 {
            DEVICE_LOST.store(true, Ordering::Release);
            return Err(InitError::TestFailed);
        }

        crate::drivers::io::outb(0x64, 0xAE); // Enable PS/2 port 1.
        crate::drivers::io::outb(0x60, 0xFF); // Keyboard reset.
        if !wait_for_data() {
            DEVICE_LOST.store(true, Ordering::Release);
            return Err(InitError::TimedOut);
        }
        let reply = crate::drivers::io::inb(0x60);
        if reply != 0xAA && reply != 0xFA {
            DEVICE_LOST.store(true, Ordering::Release);
            return Err(InitError::InvalidResponse);
        }

        crate::drivers::io::outb(0x60, 0xF4); // Enable scanning.
        if wait_for_data() {
            let _ = crate::drivers::io::inb(0x60);
        }
    }
    READY.store(true, Ordering::Release);
    DEVICE_LOST.store(false, Ordering::Release);
    Ok(())
}

fn wait_for_data() -> bool {
    for _ in 0..100_000 {
        unsafe {
            if crate::drivers::io::inb(0x64) & 0x01 != 0 {
                return true;
            }
        }
        core::hint::spin_loop();
    }
    false
}

/// Read one byte from the PS/2 controller's data port.
pub fn read_scancode() -> u8 {
    unsafe {
        let value: u8;
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") 0x60u16,
            options(nostack, nomem)
        );
        value
    }
}

/// Convert one set-1 make/break scancode into the shell's smaller key model.
pub fn decode(scancode: u8) -> Key {
    let mut state = MODIFIERS.lock();
        // E0 marks the next code as an extended-key prefix; unsupported
        // extended keys are consumed without leaking the prefix to later keys.
        if scancode == 0xE0 {
            state.extended = true;
            return Key::Unknown;
        }

        if scancode & 0x80 != 0 {
            // High-bit codes indicate release.  Release scancodes clear held
            // modifiers and never generate a printable queue event.
            let code = scancode & 0x7F;
            if code == 0x2A || code == 0x36 {
                state.shift = false;
            }
            if code == 0x1D {
                state.control = false;
            }
            if code == 0x38 {
                state.alt = false;
            }
            state.extended = false;
            return Key::Release;
        }

        match scancode {
            0x38 => {
                state.alt = true;
                Key::Unknown
            }
            0x1D => {
                // Control is a prefix/modifier for the next key, so it must remain
                // set across ordinary key makes until the Control key is released.
                state.control = true;
                Key::Unknown
            }
            0x0E => Key::Backspace,
            0x1C => Key::Enter,
            0x01 => Key::Escape,
            0x2A | 0x36 => {
                state.shift = true;
                Key::Unknown
            }
            0x3A => {
                state.caps_lock = !state.caps_lock;
                Key::Unknown
            }
            0x45 => {
                state.num_lock = !state.num_lock;
                Key::Unknown
            }
            _ => {
                if state.extended {
                    state.extended = false;
                    return match scancode {
                        0x4B => Key::ArrowLeft,
                        0x4D => Key::ArrowRight,
                        0x48 => Key::ArrowUp,
                        0x50 => Key::ArrowDown,
                        0x47 => Key::Home,
                        0x4F => Key::End,
                        0x49 => Key::PageUp,
                        0x51 => Key::PageDown,
                        0x53 => Key::Delete,
                        _ => Key::Unknown,
                    };
                }
                if state.control {
                    return match scancode {
                        // These fixed controls cover the shell cancellation and
                        // redraw operations without allocating or stringifying.
                        0x2E => Key::ControlC,
                        0x26 => Key::ControlL,
                        _ => Key::Unknown,
                    };
                }
                // Set-1 uses physical keypad scancodes outside E0 for Num Lock.
                // When Num Lock is off these keys act as navigation; with it on
                // they return digits and punctuation.
                if scancode >= 0x47 && scancode <= 0x53 {
                    return if state.num_lock {
                        match scancode {
                            0x47 => Key::Character(b'7'),
                            0x48 => Key::Character(b'8'),
                            0x49 => Key::Character(b'9'),
                            0x4B => Key::Character(b'4'),
                            0x4C => Key::Character(b'5'),
                            0x4D => Key::Character(b'6'),
                            0x4F => Key::Character(b'1'),
                            0x50 => Key::Character(b'2'),
                            0x51 => Key::Character(b'3'),
                            0x52 => Key::Character(b'0'),
                            0x53 => Key::Character(b'.'),
                            _ => Key::Unknown,
                        }
                    } else {
                        match scancode {
                            0x47 => Key::Home,
                            0x48 => Key::ArrowUp,
                            0x49 => Key::Unknown,
                            0x4B => Key::ArrowLeft,
                            0x4C => Key::Unknown,
                            0x4D => Key::ArrowRight,
                            0x4F => Key::End,
                            0x50 => Key::ArrowDown,
                            0x51 => Key::Unknown,
                            0x52 => Key::Unknown,
                            0x53 => Key::Delete,
                            _ => Key::Unknown,
                        }
                    };
                }
                // Zero entries represent keys that are not printable in this
                // early driver; unknown keys are ignored by the terminal.
                let base = match state.layout {
                    Layout::Us => KEY_TABLE,
                    Layout::Dvorak => DVORAK_KEY_TABLE,
                }.get(scancode as usize).copied().unwrap_or(0);
                if base == 0 {
                    Key::Unknown
                } else if base.is_ascii_alphabetic() && (state.shift != state.caps_lock) {
                    let shifted = match state.layout {
                        Layout::Us => SHIFT_TABLE,
                        Layout::Dvorak => DVORAK_SHIFT_TABLE,
                    }.get(scancode as usize).copied().unwrap_or(base);
                    Key::Character(shifted)
                } else {
                    Key::Character(base)
                }
            }
        }
}

// Construct both lookup tables at compile time so IRQ-time decoding is a
// bounded indexed lookup and does not allocate or format key data.
const KEY_TABLE: [u8; 128] = make_key_table(false);
const SHIFT_TABLE: [u8; 128] = make_key_table(true);
const DVORAK_KEY_TABLE: [u8; 128] = make_dvorak_table(false);
const DVORAK_SHIFT_TABLE: [u8; 128] = make_dvorak_table(true);

/// Build a small fixed Dvorak layout for the set-1 US physical scancode
/// positions.  It keeps layout translation in a compile-time lookup table,
/// just like the default US layout.
const fn make_dvorak_table(shift: bool) -> [u8; 128] {
    let mut table = make_key_table(shift);
    if shift {
        table[0x10] = b'"';
        table[0x11] = b'<';
        table[0x12] = b'>';
        table[0x13] = b'P';
        table[0x14] = b'Y';
        table[0x15] = b'F';
        table[0x16] = b'G';
        table[0x17] = b'C';
        table[0x18] = b'R';
        table[0x19] = b'L';
        table[0x1E] = b'A';
        table[0x1F] = b'O';
        table[0x20] = b'E';
        table[0x21] = b'U';
        table[0x22] = b'I';
        table[0x23] = b'D';
        table[0x24] = b'H';
        table[0x25] = b'T';
        table[0x26] = b'N';
        table[0x27] = b'S';
        table[0x2C] = b':';
        table[0x2D] = b'Q';
        table[0x2E] = b'J';
        table[0x2F] = b'K';
        table[0x30] = b'X';
        table[0x31] = b'B';
        table[0x32] = b'M';
        table[0x33] = b'W';
        table[0x34] = b'V';
        table[0x35] = b'Z';
    } else {
        table[0x10] = b'\'';
        table[0x11] = b',';
        table[0x12] = b'.';
        table[0x13] = b'p';
        table[0x14] = b'y';
        table[0x15] = b'f';
        table[0x16] = b'g';
        table[0x17] = b'c';
        table[0x18] = b'r';
        table[0x19] = b'l';
        table[0x1E] = b'a';
        table[0x1F] = b'o';
        table[0x20] = b'e';
        table[0x21] = b'u';
        table[0x22] = b'i';
        table[0x23] = b'd';
        table[0x24] = b'h';
        table[0x25] = b't';
        table[0x26] = b'n';
        table[0x27] = b's';
        table[0x2C] = b';';
        table[0x2D] = b'q';
        table[0x2E] = b'j';
        table[0x2F] = b'k';
        table[0x30] = b'x';
        table[0x31] = b'b';
        table[0x32] = b'm';
        table[0x33] = b'w';
        table[0x34] = b'v';
        table[0x35] = b'z';
    }
    table
}

/// Build the supported US set-1 printable-key mapping, optionally shifted.
const fn make_key_table(shift: bool) -> [u8; 128] {
    let mut table = [0u8; 128];
    table[0x02] = if shift { b'!' } else { b'1' };
    table[0x03] = if shift { b'@' } else { b'2' };
    table[0x04] = if shift { b'#' } else { b'3' };
    table[0x05] = if shift { b'$' } else { b'4' };
    table[0x06] = if shift { b'%' } else { b'5' };
    table[0x07] = if shift { b'^' } else { b'6' };
    table[0x08] = if shift { b'&' } else { b'7' };
    table[0x09] = if shift { b'*' } else { b'8' };
    table[0x0A] = if shift { b'(' } else { b'9' };
    table[0x0B] = if shift { b')' } else { b'0' };
    table[0x0C] = if shift { b'_' } else { b'-' };
    table[0x0D] = if shift { b'+' } else { b'=' };
    table[0x10] = if shift { b'Q' } else { b'q' };
    table[0x11] = if shift { b'W' } else { b'w' };
    table[0x12] = if shift { b'E' } else { b'e' };
    table[0x13] = if shift { b'R' } else { b'r' };
    table[0x14] = if shift { b'T' } else { b't' };
    table[0x15] = if shift { b'Y' } else { b'y' };
    table[0x16] = if shift { b'U' } else { b'u' };
    table[0x17] = if shift { b'I' } else { b'i' };
    table[0x18] = if shift { b'O' } else { b'o' };
    table[0x19] = if shift { b'P' } else { b'p' };
    table[0x1A] = if shift { b'{' } else { b'[' };
    table[0x1B] = if shift { b'}' } else { b']' };
    table[0x1E] = if shift { b'A' } else { b'a' };
    table[0x1F] = if shift { b'S' } else { b's' };
    table[0x20] = if shift { b'D' } else { b'd' };
    table[0x21] = if shift { b'F' } else { b'f' };
    table[0x22] = if shift { b'G' } else { b'g' };
    table[0x23] = if shift { b'H' } else { b'h' };
    table[0x24] = if shift { b'J' } else { b'j' };
    table[0x25] = if shift { b'K' } else { b'k' };
    table[0x26] = if shift { b'L' } else { b'l' };
    table[0x27] = if shift { b':' } else { b';' };
    table[0x28] = if shift { b'"' } else { b'\'' };
    table[0x29] = if shift { b'~' } else { b'`' };
    table[0x2C] = if shift { b'Z' } else { b'z' };
    table[0x2D] = if shift { b'X' } else { b'x' };
    table[0x2E] = if shift { b'C' } else { b'c' };
    table[0x2F] = if shift { b'V' } else { b'v' };
    table[0x30] = if shift { b'B' } else { b'b' };
    table[0x31] = if shift { b'N' } else { b'n' };
    table[0x32] = if shift { b'M' } else { b'm' };
    table[0x33] = if shift { b'<' } else { b',' };
    table[0x34] = if shift { b'>' } else { b'.' };
    table[0x35] = if shift { b'?' } else { b'/' };
    table[0x37] = b'*';
    table[0x39] = b' ';
    table[0x56] = if shift { b'|' } else { b'\\' };
    table
}
