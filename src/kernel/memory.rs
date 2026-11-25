//! Physical memory, page tables, address spaces, and kernel heap.
//!
//! The frame allocator is bitmap based and can recycle freed frames.  The
//! bootloader's Multiboot memory map selects usable RAM, with a conservative
//! fallback for bootloaders that omit the map.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::interrupts::{IrqGuard, IrqLock};
use crate::recovery::InitError;

const FRAME_SIZE: usize = 4096;
const FRAME_COUNT: usize = 1024 * 1024; // 4 GiB addressable at 4 KiB granularity.
const BITMAP_WORDS: usize = FRAME_COUNT / 64;
const MULTIBOOT_MAGIC: u32 = 0x2BAD_B002;

pub const USER_VIRTUAL_START: usize = 0x1000;
pub const USER_VIRTUAL_END: usize = 0x0008_0000_0000_0000;
pub const KERNEL_VIRTUAL_START: usize = 0xffff_8000_0000_0000;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn new(address: usize) -> Option<Self> {
        if address % FRAME_SIZE == 0 {
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
static HEAP_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HEAP_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HEAP_FRAGMENT_COUNT: AtomicUsize = AtomicUsize::new(0);

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
        let new = if free { old | (1usize << bit) } else { old & !(1usize << bit) };
        if FRAME_BITMAP[word].compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire).is_ok() {
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

    if multiboot_magic == MULTIBOOT_MAGIC && multiboot_info != 0 {
        // The Multiboot v1 info fields used here are: flags at 0, mmap_length
        // at 44, and mmap_addr at 48.
        let flags = unsafe { read_u32(multiboot_info, 0) };
        if (flags & (1 << 6)) != 0 {
            let mmap_length = unsafe { read_u32(multiboot_info, 44) } as usize;
            let mmap_addr = unsafe { read_u32(multiboot_info, 48) } as usize;
            let mut offset = 0usize;
            while offset + 4 <= mmap_length {
                let entry = mmap_addr + offset;
                let size = unsafe { read_u32(entry, 0) } as usize;
                if size < 20 || offset + 4 + size > mmap_length {
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
                        if frame >= 256 { // Reserve the first MiB used by firmware/BIOS and boot tables.
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
        for frame in 4096..8192 { // 16 MiB through 32 MiB.
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
    let mut result = None;
    'outer: for word in 0..BITMAP_WORDS {
        let mut word_value = FRAME_BITMAP[word].load(Ordering::Acquire);
        while word_value != 0 {
            let bit = word_value.trailing_zeros() as usize;
            let frame = word * 64 + bit;
            let mask = 1usize << bit;
            let old = FRAME_BITMAP[word].load(Ordering::Acquire);
            if FRAME_BITMAP[word].compare_exchange(old, old & !mask, Ordering::AcqRel, Ordering::Acquire).is_ok() {
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

/// Frame-allocation counters exposed to diagnostics and performance reports.
pub fn frame_allocations() -> usize { FRAME_ALLOCATIONS.load(Ordering::Relaxed) }
pub fn frame_deallocations() -> usize { FRAME_DEALLOCATIONS.load(Ordering::Relaxed) }
pub fn frame_allocation_failures() -> usize { FRAME_ALLOCATION_FAILURES.load(Ordering::Relaxed) }
pub fn frame_reclaims() -> usize { FRAME_RECLAIMS.load(Ordering::Relaxed) }
pub fn page_table_switches() -> usize { PAGE_TABLE_SWITCHES.load(Ordering::Relaxed) }
pub fn heap_allocations() -> usize { HEAP_ALLOCATIONS.load(Ordering::Relaxed) }
pub fn heap_deallocations() -> usize { HEAP_DEALLOCATIONS.load(Ordering::Relaxed) }
pub fn heap_fragment_count() -> usize { HEAP_FRAGMENT_COUNT.load(Ordering::Relaxed) }

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
    pub const fn empty() -> Self { Self { value: 0 } }
    pub fn is_present(&self) -> bool { self.value & PAGE_PRESENT != 0 }
    pub fn address(&self) -> usize { (self.value & 0x000F_FFFF_FFFF_F000) as usize }
}

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        Self { entries: [const { PageTableEntry::empty() }; 512] }
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
        Self { slots: IrqLock::new([EMPTY; 16]) }
    }

    fn with<R>(&self, action: impl FnOnce(&mut [AddressSpace; 16]) -> R) -> R {
        let mut slots = self.slots.lock();
        action(&mut slots)
    }
}

unsafe impl Sync for AddressSpaceTable {}

static ADDRESS_SPACES: AddressSpaceTable = AddressSpaceTable::new();
static NEXT_ADDRESS_SPACE: AtomicUsize = AtomicUsize::new(1);

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
                *slot = AddressSpace { id, cr3, active: false, mapping_count: 0 };
                return Some(id);
            }
        }
        deallocate_frame(cr3);
        None
    })
}

/// Map one 4 KiB virtual page in an address space.
pub fn map_page(address_space_id: usize, virtual_address: usize, physical_address: usize, flags: PageFlags) -> bool {
    if virtual_address % FRAME_SIZE != 0 || physical_address % FRAME_SIZE != 0 {
        return false;
    }
    if virtual_address.checked_add(FRAME_SIZE).is_none()
        || physical_address.checked_add(FRAME_SIZE).is_none()
    {
        return false;
    }
    let canonical_userspace = virtual_address >= USER_VIRTUAL_START
        && virtual_address < USER_VIRTUAL_END;
    let canonical_kernel = virtual_address >= KERNEL_VIRTUAL_START;
    if !canonical_userspace && !canonical_kernel {
        return false;
    }
    let cr3 = ADDRESS_SPACES.with(|slots| {
        slots.iter().find(|slot| slot.id == address_space_id).map(|slot| slot.cr3)
    });
    let cr3 = match cr3 { Some(cr3) => cr3, None => return false };
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
        let pdpt = match next_table(pml4_entry, user) { Some(table) => table, None => return false };
        let pdpt_entry = &mut (*pdpt).entries[pdpt_index];
        let pd = match next_table(pdpt_entry, user) { Some(table) => table, None => return false };
        let pd_entry = &mut (*pd).entries[pd_index];
        if pd_entry.value & (1 << 7) != 0 {
            return false; // Do not overwrite a 2 MiB bootstrap mapping implicitly.
        }
        let pt = match next_table(pd_entry, user) { Some(table) => table, None => return false };
        let value = if matches!(flags, PageFlags::UserExecutable) {
            (physical_address as u64) | flags_value(flags)
        } else {
            (physical_address as u64) | flags_value(flags) | PAGE_NX
        };
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
    if virtual_address % FRAME_SIZE != 0 {
        return false;
    }
    let cr3 = ADDRESS_SPACES.with(|slots| {
        slots.iter().find(|slot| slot.id == address_space_id).map(|slot| slot.cr3)
    });
    let cr3 = match cr3 { Some(cr3) => cr3, None => return false };
    unsafe {
        let pml4 = cr3 as *mut PageTable;
        let pml4_entry = &(*pml4).entries[(virtual_address >> 39) & 0x1FF];
        let pdpt = match table_address(pml4_entry) { Some(table) => table, None => return false };
        let pdpt_entry = &(*pdpt).entries[(virtual_address >> 30) & 0x1FF];
        let pd = match table_address(pdpt_entry) { Some(table) => table, None => return false };
        let pd_entry = &(*pd).entries[(virtual_address >> 21) & 0x1FF];
        if pd_entry.value & (1 << 7) != 0 {
            return false;
        }
        let pt = match table_address(pd_entry) { Some(table) => table, None => return false };
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

pub fn unmap_typed_page(address_space_id: usize, virtual_address: VirtualAddress) -> bool {
    unmap_page(address_space_id, virtual_address.as_usize())
}

pub fn address_space_mapping_count(address_space_id: usize) -> Option<usize> {
    ADDRESS_SPACES.with(|slots| {
        slots.iter()
            .find(|slot| slot.id == address_space_id)
            .map(|slot| slot.mapping_count)
    })
}

/// Destroy an address space and reclaim its root page table frame.
pub fn destroy_address_space(address_space_id: usize) -> bool {
    ADDRESS_SPACES.with(|slots| {
        for slot in slots.iter_mut() {
            if slot.id == address_space_id {
                let cr3 = slot.cr3;
                *slot = AddressSpace {
                    id: 0,
                    cr3: 0,
                    active: false,
                    mapping_count: 0,
                };
                // Reclaim every private physical leaf and intermediate table
                // owned by this address space.  Walking only the present entries
                // keeps cleanup bounded and avoids deallocating the bootstrap
                // kernel tables or an unrelated address space.
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
                // The PML4 itself is the address-space root frame.
                return deallocate_frame(cr3);
            }
        }
        false
    })
}

/// Activate an address space by writing its physical PML4 address to CR3.
pub fn activate_address_space(address_space_id: usize) -> bool {
    let cr3 = ADDRESS_SPACES.with(|slots| {
        slots.iter_mut().find(|slot| slot.id == address_space_id).map(|slot| {
            slot.active = true;
            slot.cr3
        })
    });
    let cr3 = match cr3 { Some(cr3) => cr3, None => return false };
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
    (value + align - 1) & !(align - 1)
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _heap_guard = HEAP_LOCK.lock();
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
            if address >= block as usize && address + layout.size() <= block as usize + size {
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
            if !next.is_null()
                && (current as usize) + (*current).size == next as usize
            {
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
        let new_layout = Layout::from_size_align(new_size, layout.align()).unwrap_or(Layout::new::<u8>());
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
