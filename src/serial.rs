//! 16550-compatible COM1 serial port for early diagnostics.

use core::fmt;
use crate::interrupts::IrqLock;
use core::sync::atomic::{AtomicBool, Ordering};

const COM1: u16 = 0x3F8;
static SERIAL_LOCK: IrqLock<()> = IrqLock::new(());
static SERIAL_READY: AtomicBool = AtomicBool::new(false);

// Low-level port helpers are kept local to this driver so callers use the
// formatted output interface instead of depending on UART register details.
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nostack, nomem));
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nostack, nomem));
    value
}

pub fn init() -> bool {
    unsafe {
        // Disable UART IRQs, set the divisor latch, choose a 38400-baud divisor,
        // configure 8-N-1, enable/clear FIFOs, and assert modem-control outputs.
        outb(COM1 + 1, 0);
        outb(COM1 + 3, 0x80);
        outb(COM1, 3);
        outb(COM1 + 1, 0);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
        // 16550-compatible parts expose a scratch register.  A mismatched or
        // missing UART cannot report readiness merely because port writes work.
        outb(COM1 + 7, 0xA5);
        let ready = inb(COM1 + 7) == 0xA5;
        SERIAL_READY.store(ready, Ordering::Release);
        return ready;
    }
}

fn write_byte(byte: u8) -> bool {
    if !SERIAL_READY.load(Ordering::Acquire) {
        return false;
    }
    unsafe {
        // Bound a diagnostic write even if COM1 disappears or line status freezes.
        for _ in 0..100_000 {
            if inb(COM1 + 5) & 0x20 != 0 {
                outb(COM1, byte);
                return true;
            }
            core::hint::spin_loop();
        }
        SERIAL_READY.store(false, Ordering::Release);
        return false;
    }
}

pub fn write_str(text: &str) {
    let output_start = crate::performance::timestamp();
    let _guard = SERIAL_LOCK.lock();
    for byte in text.bytes() {
        // Most terminal emulators expect CRLF even when Rust strings use LF.
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
    crate::performance::record_serial_output(text.len(), output_start);
}

struct Serial;

impl fmt::Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_str(text);
        Ok(())
    }
}

/// Write formatting arguments to COM1 without allocating an intermediate string.
pub fn print(args: fmt::Arguments) {
    use fmt::Write;
    let _ = Serial.write_fmt(args);
}

/// Write formatted diagnostic arguments followed by a line ending to COM1.
pub fn println(args: fmt::Arguments) {
    use fmt::Write;
    let mut serial = Serial;
    let _ = serial.write_fmt(args);
    let _ = serial.write_str("\n");
}
