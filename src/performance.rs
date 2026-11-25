//! Allocation-free kernel performance counters.
//!
//! Counters use monotonic atomics and CPU timestamp counters where available.
//! They are diagnostic-only: a missed or unavailable TSC measurement must
//! never alter scheduling, memory, or device behavior.

use core::sync::atomic::{AtomicUsize, Ordering};

static CONTEXT_SWITCHES: AtomicUsize = AtomicUsize::new(0);
static CONTEXT_SWITCH_TSC: AtomicUsize = AtomicUsize::new(0);
static CONTEXT_SWITCH_MAX_TSC: AtomicUsize = AtomicUsize::new(0);
static IRQ_EVENTS: AtomicUsize = AtomicUsize::new(0);
static IRQ_WAIT_TSC: AtomicUsize = AtomicUsize::new(0);
static IRQ_WAIT_MAX_TSC: AtomicUsize = AtomicUsize::new(0);
static SYSCALLS: AtomicUsize = AtomicUsize::new(0);
static SYSCALL_TSC: AtomicUsize = AtomicUsize::new(0);
static SYSCALL_MAX_TSC: AtomicUsize = AtomicUsize::new(0);
static DISK_CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static DISK_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);
static SERIAL_BYTES: AtomicUsize = AtomicUsize::new(0);
static SERIAL_WRITES: AtomicUsize = AtomicUsize::new(0);
static SERIAL_MAX_TSC: AtomicUsize = AtomicUsize::new(0);
static VGA_BYTES: AtomicUsize = AtomicUsize::new(0);
static VGA_WRITES: AtomicUsize = AtomicUsize::new(0);
static VGA_MAX_TSC: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub context_switches: usize,
    pub context_switch_total_tsc: usize,
    pub context_switch_max_tsc: usize,
    pub irq_events: usize,
    pub irq_wait_total_tsc: usize,
    pub irq_wait_max_tsc: usize,
    pub syscalls: usize,
    pub syscall_total_tsc: usize,
    pub syscall_max_tsc: usize,
    pub disk_cache_hits: usize,
    pub disk_cache_misses: usize,
    pub serial_bytes: usize,
    pub serial_writes: usize,
    pub serial_max_tsc: usize,
    pub vga_bytes: usize,
    pub vga_writes: usize,
    pub vga_max_tsc: usize,
}

#[inline]
pub fn timestamp() -> usize {
    let low: u32;
    let high: u32;
    // RDTSC is read-only and gives a monotonic platform-local cycle sample.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack)
        );
    }
    ((high as usize) << 32) | low as usize
}

fn elapsed(start: usize) -> usize {
    timestamp().wrapping_sub(start)
}

fn update_max(counter: &AtomicUsize, value: usize) {
    let mut old = counter.load(Ordering::Relaxed);
    while value > old {
        match counter.compare_exchange(old, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => old = actual,
        }
    }
}

pub fn record_context_switch(start: usize) {
    let duration = elapsed(start);
    CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
    CONTEXT_SWITCH_TSC.fetch_add(duration, Ordering::Relaxed);
    update_max(&CONTEXT_SWITCH_MAX_TSC, duration);
}

pub fn record_irq(wait_start: usize) {
    let duration = elapsed(wait_start);
    IRQ_EVENTS.fetch_add(1, Ordering::Relaxed);
    IRQ_WAIT_TSC.fetch_add(duration, Ordering::Relaxed);
    update_max(&IRQ_WAIT_MAX_TSC, duration);
}

pub fn record_syscall(start: usize) {
    let duration = elapsed(start);
    SYSCALLS.fetch_add(1, Ordering::Relaxed);
    SYSCALL_TSC.fetch_add(duration, Ordering::Relaxed);
    update_max(&SYSCALL_MAX_TSC, duration);
}

pub fn record_disk_cache_hit() {
    DISK_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_disk_cache_miss() {
    DISK_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_serial_output(bytes: usize, start: usize) {
    SERIAL_BYTES.fetch_add(bytes, Ordering::Relaxed);
    SERIAL_WRITES.fetch_add(1, Ordering::Relaxed);
    update_max(&SERIAL_MAX_TSC, elapsed(start));
}

pub fn record_vga_output(bytes: usize, start: usize) {
    VGA_BYTES.fetch_add(bytes, Ordering::Relaxed);
    VGA_WRITES.fetch_add(1, Ordering::Relaxed);
    update_max(&VGA_MAX_TSC, elapsed(start));
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        context_switches: CONTEXT_SWITCHES.load(Ordering::Relaxed),
        context_switch_total_tsc: CONTEXT_SWITCH_TSC.load(Ordering::Relaxed),
        context_switch_max_tsc: CONTEXT_SWITCH_MAX_TSC.load(Ordering::Relaxed),
        irq_events: IRQ_EVENTS.load(Ordering::Relaxed),
        irq_wait_total_tsc: IRQ_WAIT_TSC.load(Ordering::Relaxed),
        irq_wait_max_tsc: IRQ_WAIT_MAX_TSC.load(Ordering::Relaxed),
        syscalls: SYSCALLS.load(Ordering::Relaxed),
        syscall_total_tsc: SYSCALL_TSC.load(Ordering::Relaxed),
        syscall_max_tsc: SYSCALL_MAX_TSC.load(Ordering::Relaxed),
        disk_cache_hits: DISK_CACHE_HITS.load(Ordering::Relaxed),
        disk_cache_misses: DISK_CACHE_MISSES.load(Ordering::Relaxed),
        serial_bytes: SERIAL_BYTES.load(Ordering::Relaxed),
        serial_writes: SERIAL_WRITES.load(Ordering::Relaxed),
        serial_max_tsc: SERIAL_MAX_TSC.load(Ordering::Relaxed),
        vga_bytes: VGA_BYTES.load(Ordering::Relaxed),
        vga_writes: VGA_WRITES.load(Ordering::Relaxed),
        vga_max_tsc: VGA_MAX_TSC.load(Ordering::Relaxed),
    }
}
