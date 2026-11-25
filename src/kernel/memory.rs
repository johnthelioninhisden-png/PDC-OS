//! Physical memory, page tables, address spaces, and kernel heap.
//!
//! The frame allocator is bitmap based and can recycle freed frames.  The
//! bootloader's Multiboot memory map selects usable RAM, with a conservative
//! fallback for bootloaders that omit the map.

use crate::interrupts::{IrqGuard, IrqLock};
use crate::recovery::InitError;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const FRAME_SIZE: usize = 4096;
const FRAME_COUNT: usize = 1024 * 1024; // 4 GiB addressable at 4 KiB granularity.
const BITMAP_WORDS: usize = FRAME_COUNT / 64;
const MULTIBOOT_MAGIC: u32 = 0x2BAD_B002;

pub const USER_VIRTUAL_START: usize = 0x1000;
pub const USER_VIRTUAL_END: usize = 0x0008_0000_0000_0000;
pub const KERNEL_VIRTUAL_START: usize = 0xffff_8000_0000_0000;
/// Kernel and userspace regions use canonical 4 KiB page boundaries.
pub const PAGE_TABLE_ADDRESS_LIMIT: usize = FRAME_COUNT * FRAME_SIZE;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn new(address: usize) -> Option<Self> {
        // Frame zero is reserved for the firmware/bootstrap null page, and
        // the bounded allocator cannot represent addresses beyond 4 GiB.
        if address != 0 && address % FRAME_SIZE == 0 && address < PAGE_TABLE_ADDRESS_LIMIT {
            Some(Self(address))
        } else {
            None
        }
    }

    pub const fn as_usize(self) -> usize {
        self.0
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub const fn new(address: usize) -> Option<Self> {
        let userspace = address >= USER_VIRTUAL_START && address < USER_VIRTUAL_END;
        let kernel = address >= KERNEL_VIRTUAL_START;
        if address % FRAME_SIZE == 0 && (userspace || kernel) {
            Some(Self(address))
        } else {
            None
        }
    }

    pub const fn as_usize(self) -> usize {
        self.0
    }
}

// 0 is reserved/unavailable and 1 is a free physical frame.
static FRAME_BITMAP: [AtomicUsize; BITMAP_WORDS] = [const { AtomicUsize::new(0) }; BITMAP_WORDS];
static ALLOCATOR_INITIALIZED: AtomicBool = AtomicBool::new(false);
static ALLOCATED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static TOTAL_USABLE_FRAMES: AtomicUsize = AtomicUsize::new(0);
static MEMORY_MAP_REGIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATOR_LOCK: IrqLock<()> = IrqLock::new(());
static FRAME_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FRAME_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FRAME_ALLOCATION_FAILURES: AtomicUsize = AtomicUsize::new(0);
static FRAME_RECLAIMS: AtomicUsize = AtomicUsize::new(0);
static PAGE_TABLE_SWITCHES: AtomicUsize = AtomicUsize::new(0);
// Verification-only allocation ceiling.  A value of usize::MAX leaves normal
// boot behavior unchanged; the suite briefly lowers it to model exhaustion.
static FRAME_ALLOCATION_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static HEAP_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HEAP_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HEAP_FRAGMENT_COUNT: AtomicUsize = AtomicUsize::new(0);
// Fault-injection switch for a bounded kernel-heap exhaustion check.
static HEAP_FORCE_EXHAUSTED: AtomicBool = AtomicBool::new(false);

#[inline]
fn bitmap_test(frame: usize) -> bool {
    let word = frame / 64;
    let bit = frame % 64;
    if word >= BITMAP_WORDS {
        return false;
    }
    (FRAME_BITMAP[word].load(Ordering::Acquire) & (1usize << bit)) != 0
}

#[inline]
fn bitmap_set(frame: usize, free: bool) {
    let word = frame / 64;
    let bit = frame % 64;
    if word >= BITMAP_WORDS {
        return;
    }
    loop {
        let old = FRAME_BITMAP[word].load(Ordering::Acquire);
        let new = if free {
            old | (1usize << bit)
        } else {
            old & !(1usize << bit)
        };
        if FRAME_BITMAP[word]
            .compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

fn lock_allocator() -> IrqGuard<'static, ()> {
    ALLOCATOR_LOCK.lock()
}

unsafe fn read_u32(address: usize, offset: usize) -> u32 {
    ((address + offset) as *const u32).read_unaligned()
}

unsafe fn read_u64(address: usize, offset: usize) -> u64 {
    ((address + offset) as *const u64).read_unaligned()
}

/// Initialize physical memory from the Multiboot memory map.
pub fn init(multiboot_magic: u32, multiboot_info: usize) {
    let _ = init_result(multiboot_magic, multiboot_info);
}

/// Initialize memory and return whether the bootloader or bounded fallback
/// supplied at least one usable physical frame.
pub fn init_result(multiboot_magic: u32, multiboot_info: usize) -> Result<(), InitError> {
    if ALLOCATOR_INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }

    let _allocator_guard = lock_allocator();
    let mut usable = 0usize;
    let mut map_regions = 0usize;

    if multiboot_magic == MULTIBOOT_MAGIC
        && multiboot_info != 0
        && multiboot_info <= PAGE_TABLE_ADDRESS_LIMIT
        && multiboot_info
            .checked_add(512)
            .is_some_and(|end| end <= PAGE_TABLE_ADDRESS_LIMIT)
    {
        // The Multiboot v1 info fields used here are: flags at 0, mmap_length
        // at 44, and mmap_addr at 48.
        let flags = unsafe { read_u32(multiboot_info, 0) };
        if (flags & (1 << 6)) != 0 {
            let mmap_length = unsafe { read_u32(multiboot_info, 44) } as usize;
            let mmap_addr = unsafe { read_u32(multiboot_info, 48) } as usize;
            let Some(mmap_end) = mmap_addr.checked_add(mmap_length) else {
                TOTAL_USABLE_FRAMES.store(0, Ordering::Release);
                MEMORY_MAP_REGIONS.store(0, Ordering::Release);
                ALLOCATOR_INITIALIZED.store(true, Ordering::Release);
                return Err(InitError::InvalidResponse);
            };
            if mmap_addr == 0
                || mmap_end > PAGE_TABLE_ADDRESS_LIMIT
                || mmap_length > 1024 * 1024
            {
                TOTAL_USABLE_FRAMES.store(0, Ordering::Release);
                MEMORY_MAP_REGIONS.store(0, Ordering::Release);
                ALLOCATOR_INITIALIZED.store(true, Ordering::Release);
                return Err(InitError::InvalidResponse);
            }
            let mut offset = 0usize;
            while offset + 4 <= mmap_length {
                let entry = mmap_addr + offset;
                let size = unsafe { read_u32(entry, 0) } as usize;
                if size < 20
                    || offset.checked_add(4).and_then(|value| value.checked_add(size)).is_none()
                    || offset + 4 + size > mmap_length
                    || entry.checked_add(size + 4).is_none_or(|end| end > mmap_end)
                {
                    break;
                }
                let base = unsafe { read_u64(entry, 4) };
                let length = unsafe { read_u64(entry, 12) };
                let type_value = unsafe { read_u32(entry, 20) };
                map_regions += 1;
                if type_value == 1 {
                    let start = ((base + (FRAME_SIZE as u64 - 1)) / FRAME_SIZE as u64) as usize;
                    let end = ((base.saturating_add(length)) / FRAME_SIZE as u64) as usize;
                    for frame in start..end.min(FRAME_COUNT) {
                        if frame >= 256 {
                            // Reserve the first MiB used by firmware/BIOS and boot tables.
                            bitmap_set(frame, true);
                            usable += 1;
                        }
                    }
                }
                offset += 4 + size;
            }
        }
    }

    // Conservative fallback if no valid map was supplied.  Never assume memory
    // above 16 MiB until it has been reported by the bootloader.
    if usable == 0 {
        for frame in 4096..8192 {
            // 16 MiB through 32 MiB.
            bitmap_set(frame, true);
            usable += 1;
        }
    }

    // The first 16 MiB are reserved for the kernel, bootstrap tables, and
    // firmware.  Mark any map entries inside that range unavailable.
    for frame in 0..4096 {
        bitmap_set(frame, false);
    }

    TOTAL_USABLE_FRAMES.store(usable, Ordering::Release);
    MEMORY_MAP_REGIONS.store(map_regions, Ordering::Release);
    ALLOCATOR_INITIALIZED.store(true, Ordering::Release);
    if usable == 0 {
        Err(InitError::NotPresent)
    } else {
        Ok(())
    }
}

/// Report how many Multiboot memory-map entries were parsed.
pub fn memory_map_region_count() -> usize {
    MEMORY_MAP_REGIONS.load(Ordering::Relaxed)
}

/// Reserve and return a 4 KiB physical frame, or None when physical memory is exhausted.
pub fn allocate_frame() -> Option<usize> {
    if !ALLOCATOR_INITIALIZED.load(Ordering::Acquire) {
        return None;
    }
    let _allocator_guard = lock_allocator();
    if ALLOCATED_FRAMES.load(Ordering::Acquire)
        >= FRAME_ALLOCATION_LIMIT.load(Ordering::Acquire)
    {
        FRAME_ALLOCATION_FAILURES.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let mut result = None;
    'outer: for word in 0..BITMAP_WORDS {
        let mut word_value = FRAME_BITMAP[word].load(Ordering::Acquire);
        while word_value != 0 {
            let bit = word_value.trailing_zeros() as usize;
            let frame = word * 64 + bit;
            let mask = 1usize << bit;
            let old = FRAME_BITMAP[word].load(Ordering::Acquire);
            if FRAME_BITMAP[word]
                .compare_exchange(old, old & !mask, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                ALLOCATED_FRAMES.fetch_add(1, Ordering::Relaxed);
                FRAME_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                result = Some(frame * FRAME_SIZE);
                break 'outer;
            }
            word_value = FRAME_BITMAP[word].load(Ordering::Acquire);
        }
    }
    if result.is_none() {
        FRAME_ALLOCATION_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Typed frame allocation prevents callers from losing physical-address type
/// safety while crossing architecture-neutral HAL boundaries.
pub fn allocate_typed_frame() -> Option<PhysicalAddress> {
    PhysicalAddress::new(allocate_frame()?)
}

/// Return a frame to the reclaimable physical allocator.
pub fn deallocate_frame(address: usize) -> bool {
    if address == 0 || address % FRAME_SIZE != 0 || address / FRAME_SIZE >= FRAME_COUNT {
        return false;
    }
    let _allocator_guard = lock_allocator();
    let frame = address / FRAME_SIZE;
    if bitmap_test(frame) {
        return false;
    }
    bitmap_set(frame, true);
    ALLOCATED_FRAMES.fetch_sub(1, Ordering::Relaxed);
    FRAME_DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    FRAME_RECLAIMS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Typed counterpart to [`deallocate_frame`].
pub fn deallocate_typed_frame(address: PhysicalAddress) -> bool {
    deallocate_frame(address.as_usize())
}

/// Frame-allocation counters exposed to diagnostics and performance reports.
pub fn frame_allocations() -> usize {
    FRAME_ALLOCATIONS.load(Ordering::Relaxed)
}
pub fn frame_deallocations() -> usize {
    FRAME_DEALLOCATIONS.load(Ordering::Relaxed)
}
pub fn frame_allocation_failures() -> usize {
    FRAME_ALLOCATION_FAILURES.load(Ordering::Relaxed)
}
pub fn frame_reclaims() -> usize {
    FRAME_RECLAIMS.load(Ordering::Relaxed)
}
pub fn page_table_switches() -> usize {
    PAGE_TABLE_SWITCHES.load(Ordering::Relaxed)
}
pub fn heap_allocations() -> usize {
    HEAP_ALLOCATIONS.load(Ordering::Relaxed)
}
pub fn heap_deallocations() -> usize {
    HEAP_DEALLOCATIONS.load(Ordering::Relaxed)
}
pub fn heap_fragment_count() -> usize {
    HEAP_FRAGMENT_COUNT.load(Ordering::Relaxed)
}

/// Repeatedly allocate and return a fixed heap block to detect state leaks.
pub fn heap_repeated_probe() -> bool {
    let layout = match Layout::from_size_align(64, 8) {
        Ok(layout) => layout,
        Err(_) => return false,
    };
    for _ in 0..32 {
        let pointer = unsafe { GLOBAL_ALLOCATOR.alloc(layout) };
        if pointer.is_null() {
            return false;
        }
        unsafe {
            GLOBAL_ALLOCATOR.dealloc(pointer, layout);
        }
    }
    true
}

/// Fault-inject heap exhaustion without consuming or permanently damaging the heap.
pub fn heap_exhaustion_probe() -> bool {
    let layout = match Layout::from_size_align(1, 1) {
        Ok(layout) => layout,
        Err(_) => return false,
    };
    HEAP_FORCE_EXHAUSTED.store(true, Ordering::Release);
    let pointer = unsafe { GLOBAL_ALLOCATOR.alloc(layout) };
    HEAP_FORCE_EXHAUSTED.store(false, Ordering::Release);
    pointer.is_null()
}

/// Fault-inject frame exhaustion at the current active allocation count.
pub fn frame_exhaustion_probe() -> bool {
    let active = ALLOCATED_FRAMES.load(Ordering::Acquire);
    FRAME_ALLOCATION_LIMIT.store(active, Ordering::Release);
    let exhausted = allocate_frame().is_none();
    FRAME_ALLOCATION_LIMIT.store(usize::MAX, Ordering::Release);
    exhausted
}

/// Validate Multiboot address and length contracts without trusting caller data.
pub fn validate_boot_info(multiboot_magic: u32, multiboot_info: usize) -> bool {
    if multiboot_magic != MULTIBOOT_MAGIC || multiboot_info == 0 {
        return false;
    }
    multiboot_info <= PAGE_TABLE_ADDRESS_LIMIT
        && multiboot_info
            .checked_add(512)
            .is_some_and(|end| end <= PAGE_TABLE_ADDRESS_LIMIT)
}

/// Report total bytes handed out by the frame allocator.
pub fn allocated_bytes() -> usize {
    ALLOCATED_FRAMES.load(Ordering::Relaxed) * FRAME_SIZE
}

/// Report usable physical memory discovered by the bootloader or fallback.
pub fn usable_bytes() -> usize {
    TOTAL_USABLE_FRAMES.load(Ordering::Relaxed) * FRAME_SIZE
}

/// Report available physical memory after active allocations.
pub fn available_bytes() -> usize {
    usable_bytes().saturating_sub(allocated_bytes())
}

/// Return the allocator's fixed physical frame size.
pub fn frame_size() -> usize {
    FRAME_SIZE
}

const PAGE_PRESENT: u64 = 1;
const PAGE_WRITABLE: u64 = 1 << 1;
const PAGE_USER: u64 = 1 << 2;
const PAGE_NX: u64 = 1 << 63;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry {
    pub value: u64,
}

impl PageTableEntry {
    pub const fn empty() -> Self {
        Self { value: 0 }
    }
    pub fn is_present(&self) -> bool {
        self.value & PAGE_PRESENT != 0
    }
    pub fn address(&self) -> usize {
        (self.value & 0x000F_FFFF_FFFF_F000) as usize
    }
}

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { PageTableEntry::empty() }; 512],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageFlags {
    ReadOnly,
    ReadWrite,
    UserReadOnly,
    UserReadWrite,
    UserExecutable,
}

impl PageFlags {
    pub const fn is_user(self) -> bool {
        matches!(self, Self::UserReadOnly | Self::UserReadWrite | Self::UserExecutable)
    }

    pub const fn is_writable(self) -> bool {
        matches!(self, Self::ReadWrite | Self::UserReadWrite)
    }

    pub const fn is_executable(self) -> bool {
        matches!(self, Self::UserExecutable)
    }
}

fn flags_value(flags: PageFlags) -> u64 {
    match flags {
        PageFlags::ReadOnly => PAGE_PRESENT,
        PageFlags::ReadWrite => PAGE_PRESENT | PAGE_WRITABLE,
        PageFlags::UserReadOnly => PAGE_PRESENT | PAGE_USER,
        PageFlags::UserReadWrite => PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER,
        PageFlags::UserExecutable => PAGE_PRESENT | PAGE_USER,
    }
}

#[derive(Clone, Copy)]
pub struct AddressSpace {
    pub id: usize,
    pub cr3: usize,
    pub active: bool,
    pub mapping_count: usize,
}

pub struct AddressSpaceTable {
    slots: IrqLock<[AddressSpace; 16]>,
}

impl AddressSpaceTable {
    pub const fn new() -> Self {
        const EMPTY: AddressSpace = AddressSpace {
            id: 0,
            cr3: 0,
            active: false,
            mapping_count: 0,
        };
        Self {
            slots: IrqLock::new([EMPTY; 16]),
        }
    }

    fn with<R>(&self, action: impl FnOnce(&mut [AddressSpace; 16]) -> R) -> R {
        let mut slots = self.slots.lock();
        action(&mut slots)
    }
}

unsafe impl Sync for AddressSpaceTable {}

static ADDRESS_SPACES: AddressSpaceTable = AddressSpaceTable::new();
static NEXT_ADDRESS_SPACE: AtomicUsize = AtomicUsize::new(1);
// Capture the bootstrap CR3 once; destroying the active private PML4 must
// never leave the CPU executing through a frame that is about to be reclaimed.
static BOOTSTRAP_CR3: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn table_address(entry: &PageTableEntry) -> Option<*mut PageTable> {
    if entry.is_present() {
        Some(entry.address() as *mut PageTable)
    } else {
        None
    }
}

unsafe fn next_table(entry: &mut PageTableEntry, user: bool) -> Option<*mut PageTable> {
    if let Some(table) = table_address(entry) {
        // Preserve existing page protections while descending into an existing table.
        return Some(table);
    }
    let frame = allocate_frame()?;
    let table = frame as *mut PageTable;
    core::ptr::write_bytes(table as *mut u8, 0, FRAME_SIZE);
    entry.value = frame as u64 | PAGE_PRESENT | PAGE_WRITABLE | if user { PAGE_USER } else { 0 };
    Some(table)
}

/// Validate a 4 KiB mapping before any page-table traversal or dereference.
/// This is also the central policy check used by the security diagnostics.
pub fn validate_mapping(
    address_space_id: usize,
    virtual_address: usize,
    physical_address: usize,
    flags: PageFlags,
) -> bool {
    if virtual_address % FRAME_SIZE != 0 || physical_address % FRAME_SIZE != 0 {
        return false;
    }
    if physical_address == 0
        || physical_address >= PAGE_TABLE_ADDRESS_LIMIT
        || virtual_address.checked_add(FRAME_SIZE).is_none()
        || physical_address.checked_add(FRAME_SIZE).is_none()
    {
        return false;
    }

    let canonical_userspace =
        virtual_address >= USER_VIRTUAL_START && virtual_address < USER_VIRTUAL_END;
    let canonical_kernel = virtual_address >= KERNEL_VIRTUAL_START;
    if !canonical_userspace && !canonical_kernel {
        return false;
    }
    if canonical_userspace != flags.is_user() {
        return false;
    }

    // Address spaces must exist, and no mapping may create an executable
    // writable page through the public mapping API.
    if flags.is_writable() && flags.is_executable() {
        return false;
    }
    ADDRESS_SPACES.with(|slots| {
        slots.iter().any(|slot| slot.id == address_space_id)
    })
}

/// Create an isolated 4-level x86-64 address space.
pub fn create_address_space() -> Option<usize> {
    let cr3 = allocate_frame()?;
    unsafe {
        core::ptr::write_bytes(cr3 as *mut u8, 0, FRAME_SIZE);
    }
    let id = NEXT_ADDRESS_SPACE.fetch_add(1, Ordering::Relaxed);
    ADDRESS_SPACES.with(|slots| {
        for slot in slots.iter_mut() {
            if slot.id == 0 {
                *slot = AddressSpace {
                    id,
                    cr3,
                    active: false,
                    mapping_count: 0,
                };
                return Some(id);
            }
        }
        deallocate_frame(cr3);
        None
    })
}

/// Map one 4 KiB virtual page in an address space.
pub fn map_page(
    address_space_id: usize,
    virtual_address: usize,
    physical_address: usize,
    flags: PageFlags,
) -> bool {
    if !validate_mapping(address_space_id, virtual_address, physical_address, flags) {
        return false;
    }
    let cr3 = ADDRESS_SPACES.with(|slots| {
        slots
            .iter()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| slot.cr3)
    });
    let cr3 = match cr3 {
        Some(cr3) => cr3,
        None => return false,
    };
    let pml4 = cr3 as *mut PageTable;
    let pml4_index = (virtual_address >> 39) & 0x1FF;
    let pdpt_index = (virtual_address >> 30) & 0x1FF;
    let pd_index = (virtual_address >> 21) & 0x1FF;
    let pt_index = (virtual_address >> 12) & 0x1FF;
    let user = matches!(
        flags,
        PageFlags::UserReadOnly | PageFlags::UserReadWrite | PageFlags::UserExecutable
    );

    unsafe {
        let pml4_entry = &mut (*pml4).entries[pml4_index];
        let pdpt = match next_table(pml4_entry, user) {
            Some(table) => table,
            None => return false,
        };
        let pdpt_entry = &mut (*pdpt).entries[pdpt_index];
        let pd = match next_table(pdpt_entry, user) {
            Some(table) => table,
            None => return false,
        };
        let pd_entry = &mut (*pd).entries[pd_index];
        if pd_entry.value & (1 << 7) != 0 {
            return false; // Do not overwrite a 2 MiB bootstrap mapping implicitly.
        }
        let pt = match next_table(pd_entry, user) {
            Some(table) => table,
            None => return false,
        };
        // A writable or read-only mapping is never executable unless its
        // explicit PageFlags variant says so.  This makes the NX policy visible
        // in every leaf entry instead of relying on CPU defaults.
        let value = (physical_address as u64)
            | flags_value(flags)
            | if flags.is_executable() { 0 } else { PAGE_NX };
        let entry = &mut (*pt).entries[pt_index];
        let was_mapped = entry.is_present();
        entry.value = value;
        if !was_mapped {
            ADDRESS_SPACES.with(|slots| {
                if let Some(slot) = slots.iter_mut().find(|slot| slot.id == address_space_id) {
                    slot.mapping_count = slot.mapping_count.saturating_add(1);
                }
            });
        }
    }
    invalidate_tlb(VirtualAddress::new(virtual_address).unwrap());
    true
}

/// Typed wrapper that prevents raw address arithmetic at HAL call sites.
pub fn map_typed_page(
    address_space_id: usize,
    virtual_address: VirtualAddress,
    physical_address: PhysicalAddress,
    flags: PageFlags,
) -> bool {
    map_page(
        address_space_id,
        virtual_address.as_usize(),
        physical_address.as_usize(),
        flags,
    )
}

/// Remove a 4 KiB virtual mapping from an address space.
pub fn unmap_page(address_space_id: usize, virtual_address: usize) -> bool {
    if virtual_address % FRAME_SIZE != 0
        || (virtual_address < USER_VIRTUAL_START && virtual_address != 0)
        || (virtual_address >= USER_VIRTUAL_END && virtual_address < KERNEL_VIRTUAL_START)
    {
        return false;
    }
    let cr3 = ADDRESS_SPACES.with(|slots| {
        slots
            .iter()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| slot.cr3)
    });
    let cr3 = match cr3 {
        Some(cr3) => cr3,
        None => return false,
    };
    unsafe {
        let pml4 = cr3 as *mut PageTable;
        let pml4_entry = &(*pml4).entries[(virtual_address >> 39) & 0x1FF];
        let pdpt = match table_address(pml4_entry) {
            Some(table) => table,
            None => return false,
        };
        let pdpt_entry = &(*pdpt).entries[(virtual_address >> 30) & 0x1FF];
        let pd = match table_address(pdpt_entry) {
            Some(table) => table,
            None => return false,
        };
        let pd_entry = &(*pd).entries[(virtual_address >> 21) & 0x1FF];
        if pd_entry.value & (1 << 7) != 0 {
            return false;
        }
        let pt = match table_address(pd_entry) {
            Some(table) => table,
            None => return false,
        };
        let entry = &mut (*pt).entries[(virtual_address >> 12) & 0x1FF];
        if !entry.is_present() {
            return false;
        }
        entry.value = 0;
    }
    ADDRESS_SPACES.with(|slots| {
        if let Some(slot) = slots.iter_mut().find(|slot| slot.id == address_space_id) {
            slot.mapping_count = slot.mapping_count.saturating_sub(1);
        }
    });
    invalidate_tlb(VirtualAddress::new(virtual_address).unwrap());
    true
}

fn invalidate_tlb(address: VirtualAddress) {
    unsafe {
        core::arch::asm!(
            "invlpg [{address}]",
            address = in(reg) address.as_usize(),
            options(nostack, preserves_flags),
        );
    }
}

/// Public architecture-neutral page invalidation entry point for kernel callers.
pub fn invalidate_page_tlb(address: VirtualAddress) {
    invalidate_tlb(address);
}

/// Reloading CR3 invalidates the address-space-wide TLB on this architecture.
pub fn invalidate_address_space_tlb() {
    unsafe {
        let cr3: u64;
        core::arch::asm!(
            "mov {cr3}, cr3",
            cr3 = out(reg) cr3,
            options(nomem, nostack),
        );
        core::arch::asm!(
            "mov cr3, {cr3}",
            cr3 = in(reg) cr3,
            options(nostack, preserves_flags),
        );
    }
}

pub fn unmap_typed_page(address_space_id: usize, virtual_address: VirtualAddress) -> bool {
    unmap_page(address_space_id, virtual_address.as_usize())
}

pub fn address_space_mapping_count(address_space_id: usize) -> Option<usize> {
    ADDRESS_SPACES.with(|slots| {
        slots
            .iter()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| slot.mapping_count)
    })
}

/// Verify an existing 4 KiB leaf entry and its permission contract.
/// This is a bounded read-only audit: missing tables or leaves return false.
pub fn validate_existing_mapping(
    address_space_id: usize,
    virtual_address: usize,
    expected_flags: PageFlags,
) -> bool {
    if !VirtualAddress::new(virtual_address).is_some() {
        return false;
    }
    let Some(cr3) = ADDRESS_SPACES.with(|slots| {
        slots
            .iter()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| slot.cr3)
    }) else {
        return false;
    };
    unsafe {
        let pml4 = cr3 as *mut PageTable;
        let pml4_entry = &(*pml4).entries[(virtual_address >> 39) & 0x1FF];
        let Some(pdpt) = table_address(pml4_entry) else {
            return false;
        };
        let pdpt_entry = &(*pdpt).entries[(virtual_address >> 30) & 0x1FF];
        let Some(pd) = table_address(pdpt_entry) else {
            return false;
        };
        let pd_entry = &(*pd).entries[(virtual_address >> 21) & 0x1FF];
        if pd_entry.value & (1 << 7) != 0 {
            return false;
        }
        let Some(pt) = table_address(pd_entry) else {
            return false;
        };
        let entry = &(*pt).entries[(virtual_address >> 12) & 0x1FF];
        if !entry.is_present() {
            return false;
        }

        let writable = entry.value & PAGE_WRITABLE != 0;
        let user = entry.value & PAGE_USER != 0;
        let nx = entry.value & PAGE_NX != 0;
        writable == expected_flags.is_writable()
            && user == expected_flags.is_user()
            && nx == !expected_flags.is_executable()
    }
}

/// Destroy an address space and reclaim its root page table frame.
pub fn destroy_address_space(address_space_id: usize) -> bool {
    ADDRESS_SPACES.with(|slots| {
        for slot in slots.iter_mut() {
            if slot.id == address_space_id {
                let cr3 = slot.cr3;
                let was_active = slot.active;
                *slot = AddressSpace {
                    id: 0,
                    cr3: 0,
                    active: false,
                    mapping_count: 0,
                };
                // Reclaim private user mappings and every page-table frame they
                // reach.  We walk only present entries, so a malformed or partial
                // address space cannot make cleanup scan beyond 512 slots per level.
                unsafe {
                    let pml4 = cr3 as *mut PageTable;
                    for pml4_entry in &mut (*pml4).entries {
                        if !pml4_entry.is_present() {
                            continue;
                        }
                        let pdpt = pml4_entry.address() as *mut PageTable;
                        for pdpt_entry in &mut (*pdpt).entries {
                            if !pdpt_entry.is_present() {
                                continue;
                            }
                            let pd = pdpt_entry.address() as *mut PageTable;
                            for pd_entry in &mut (*pd).entries {
                                if !pd_entry.is_present() {
                                    continue;
                                }
                                if pd_entry.value & (1 << 7) == 0 {
                                    let pt = pd_entry.address() as *mut PageTable;
                                    for pt_entry in &mut (*pt).entries {
                                        if pt_entry.is_present() {
                                            // In this kernel's process model the
                                            // address space owns mapped leaf frames.
                                            let _ = deallocate_frame(pt_entry.address());
                                        }
                                    }
                                    let _ = deallocate_frame(pd_entry.address());
                                }
                            }
                            let _ = deallocate_frame(pdpt_entry.address());
                        }
                        let _ = deallocate_frame(pml4_entry.address());
                    }
                }
                // If a caller destroys the currently active root, restore the
                // bootstrap PML4 before the private root frame becomes reusable.
                if was_active {
                    let bootstrap_cr3 = BOOTSTRAP_CR3.load(Ordering::Acquire);
                    if bootstrap_cr3 != 0 {
                        unsafe {
                            core::arch::asm!(
                                "mov cr3, {cr3}",
                                cr3 = in(reg) bootstrap_cr3 as u64,
                                options(nostack, preserves_flags),
                            );
                        }
                    }
                }
                // The PML4 itself is the address-space root frame; reclaim it
                // only after all child tables and leaves have been released.
                return deallocate_frame(cr3);
            }
        }
        false
    })
}

/// Activate an address space by writing its physical PML4 address to CR3.
pub fn activate_address_space(address_space_id: usize) -> bool {
    if BOOTSTRAP_CR3.load(Ordering::Acquire) == 0 {
        let mut cr3: u64;
        unsafe {
            core::arch::asm!(
                "mov {cr3}, cr3",
                cr3 = out(reg) cr3,
                options(nomem, nostack),
            );
        }
        let _ =
            BOOTSTRAP_CR3.compare_exchange(0, cr3 as usize, Ordering::AcqRel, Ordering::Acquire);
    }
    let cr3 = ADDRESS_SPACES.with(|slots| {
        // Only one address space may be active in the single-CPU kernel.  Keep
        // the slot state consistent even if activation is called twice.
        for slot in slots.iter_mut() {
            slot.active = false;
        }
        slots
            .iter_mut()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| {
                slot.active = true;
                slot.cr3
            })
    });
    let cr3 = match cr3 {
        Some(cr3) => cr3,
        None => return false,
    };
    unsafe {
        core::arch::asm!("mov cr3, {0}", in(reg) cr3 as u64, options(nostack, preserves_flags));
    }
    PAGE_TABLE_SWITCHES.fetch_add(1, Ordering::Relaxed);
    true
}

/// Kernel heap allocator metadata: a block is followed by an optional free
/// block.  Repeated allocation/deallocation coalesces neighboring blocks.
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

static mut HEAP: [u64; 262144] = [0; 262144]; // 2 MiB kernel heap.
static HEAP_HEAD: AtomicUsize = AtomicUsize::new(0);
static HEAP_LOCK: IrqLock<()> = IrqLock::new(());

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    let Some(aligned) = value.checked_add(align.saturating_sub(1)) else {
        return usize::MAX;
    };
    aligned & !(align.saturating_sub(1))
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _heap_guard = HEAP_LOCK.lock();
        if HEAP_FORCE_EXHAUSTED.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }
        let header_size = core::mem::size_of::<FreeBlock>();
        let heap_ptr = core::ptr::addr_of_mut!(HEAP);
        let base = heap_ptr as usize;
        if HEAP_HEAD.load(Ordering::Acquire) == 0 {
            let head = base as *mut FreeBlock;
            (*head).size = core::mem::size_of::<[u64; 262144]>();
            (*head).next = core::ptr::null_mut();
            HEAP_HEAD.store(base, Ordering::Release);
        }

        let mut previous: *mut FreeBlock = core::ptr::null_mut();
        let mut block = HEAP_HEAD.load(Ordering::Acquire) as *mut FreeBlock;
        while !block.is_null() {
            let address = align_up(block as usize, layout.align());
            let size = (*block).size;
            let block_end = match (block as usize).checked_add(size) {
                Some(end) => end,
                None => break,
            };
            let allocation_end = match address.checked_add(layout.size()) {
                Some(end) => end,
                None => break,
            };
            if address != usize::MAX && address >= block as usize && allocation_end <= block_end {
                let next = (*block).next;
                if address - block as usize >= header_size + layout.size() {
                    let remainder = address as *mut FreeBlock;
                    (*remainder).size = size - (address - block as usize);
                    (*remainder).next = next;
                    if previous.is_null() {
                        HEAP_HEAD.store(remainder as usize, Ordering::Release);
                    } else {
                        (*previous).next = remainder;
                    }
                } else if previous.is_null() {
                    HEAP_HEAD.store(next as usize, Ordering::Release);
                } else {
                    (*previous).next = next;
                }
                HEAP_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                return address as *mut u8;
            }
            previous = block;
            block = (*block).next;
        }
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _heap_guard = HEAP_LOCK.lock();
        if ptr.is_null() {
            return;
        }
        // Stage 8 pointer-boundary audit: never accept pointers outside the
        // fixed kernel heap or layouts whose end arithmetic would overflow.
        let heap_ptr = core::ptr::addr_of_mut!(HEAP) as usize;
        let heap_end = match heap_ptr.checked_add(core::mem::size_of::<[u64; 262144]>()) {
            Some(end) => end,
            None => return,
        };
        let ptr_address = ptr as usize;
        let allocation_end = match ptr_address.checked_add(layout.size()) {
            Some(end) => end,
            None => return,
        };
        if ptr_address < heap_ptr
            || allocation_end > heap_end
            || ptr_address % layout.align() != 0
        {
            return;
        }
        let block = ptr as *mut FreeBlock;
        (*block).size = layout.size().max(core::mem::size_of::<FreeBlock>());
        (*block).next = HEAP_HEAD.load(Ordering::Acquire) as *mut FreeBlock;
        HEAP_HEAD.store(block as usize, Ordering::Release);
        HEAP_DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);

        // Count free blocks as a fragmentation diagnostic and coalesce adjacent
        // blocks here, keeping the hot allocator path allocation-free.
        let mut fragments = 0usize;
        let mut current = HEAP_HEAD.load(Ordering::Acquire) as *mut FreeBlock;
        while !current.is_null() {
            fragments += 1;
            let next = (*current).next;
            if !next.is_null() && (current as usize) + (*current).size == next as usize {
                (*current).size += (*next).size;
                (*current).next = (*next).next;
                fragments = fragments.saturating_sub(1);
            } else {
                current = next;
            }
        }
        HEAP_FRAGMENT_COUNT.store(fragments, Ordering::Release);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = self.alloc(layout);
        if !ptr.is_null() {
            core::ptr::write_bytes(ptr, 0, layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout =
            Layout::from_size_align(new_size, layout.align()).unwrap_or(Layout::new::<u8>());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

pub struct KernelHeap;

#[global_allocator]
static GLOBAL_ALLOCATOR: KernelHeap = KernelHeap;
