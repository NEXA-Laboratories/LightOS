//! Physical frame allocator, page-table access (virtual memory) and heap bring-up.
use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use bootloader::BootInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::registers::control::Cr3;
use alloc::string::{String, ToString};
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

static USABLE_BYTES: AtomicU64 = AtomicU64::new(0);
static FRAMES_USED: AtomicU64 = AtomicU64::new(0);
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);
static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static FRAMES: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// # Safety
    /// The memory map must be valid: all `Usable` frames really are unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator { memory_map, next: 0 }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        self.memory_map
            .iter()
            .filter(|r| r.region_type == MemoryRegionType::Usable)
            .map(|r| r.range.start_addr()..r.range.end_addr())
            .flat_map(|r| r.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        if frame.is_some() {
            self.next += 1;
            FRAMES_USED.fetch_add(1, Ordering::Relaxed);
        }
        frame
    }
}

unsafe fn active_level_4_table(offset: VirtAddr) -> &'static mut PageTable {
    let (frame, _) = Cr3::read();
    let virt = offset + frame.start_address().as_u64();
    &mut *virt.as_mut_ptr::<PageTable>()
}

pub fn init(boot_info: &'static BootInfo) {
    let offset = VirtAddr::new(boot_info.physical_memory_offset);
    PHYS_OFFSET.store(offset.as_u64(), Ordering::Relaxed);
    let mut mapper = unsafe { OffsetPageTable::new(active_level_4_table(offset), offset) };
    let mut frames = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };

    let usable: u64 = boot_info
        .memory_map
        .iter()
        .filter(|r| r.region_type == MemoryRegionType::Usable)
        .map(|r| r.range.end_addr() - r.range.start_addr())
        .sum();
    USABLE_BYTES.store(usable, Ordering::Relaxed);

    crate::allocator::init_heap(&mut mapper, &mut frames).expect("heap initialization failed");
    *MAPPER.lock() = Some(mapper);
    *FRAMES.lock() = Some(frames);
}

/// Virtual -> physical translation through the live page tables.
pub fn translate(addr: VirtAddr) -> Option<PhysAddr> {
    MAPPER.lock().as_ref().and_then(|m| m.translate_addr(addr))
}

pub fn usable_bytes() -> u64 {
    USABLE_BYTES.load(Ordering::Relaxed)
}
pub fn frames_used() -> u64 {
    FRAMES_USED.load(Ordering::Relaxed)
}
pub fn phys_offset() -> u64 {
    PHYS_OFFSET.load(Ordering::Relaxed)
}

/// Make sure [start, start+len) is mapped (kernel-only, read/write/execute). Pages that are
/// already mapped are reused, so re-running an app does not leak frames.
pub fn map_region(start: u64, len: usize) -> Result<(), String> {
    if len == 0 {
        return Ok(());
    }
    let mut mg = MAPPER.lock();
    let mut fg = FRAMES.lock();
    let (mapper, frames) = match (mg.as_mut(), fg.as_mut()) {
        (Some(m), Some(f)) => (m, f),
        _ => return Err("memory manager not ready".to_string()),
    };
    let first = Page::<Size4KiB>::containing_address(VirtAddr::new(start));
    let last = Page::<Size4KiB>::containing_address(VirtAddr::new(start + len as u64 - 1));
    for page in Page::range_inclusive(first, last) {
        if mapper.translate_page(page).is_ok() {
            continue;
        }
        let frame = frames.allocate_frame().ok_or_else(|| "out of physical memory".to_string())?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper
                .map_to(page, frame, flags, frames)
                .map_err(|_| "page mapping failed".to_string())?
                .flush();
        }
    }
    Ok(())
}

static MMIO_NEXT: AtomicU64 = AtomicU64::new(0x0000_0200_0000_0000);

/// Map device registers (physical `phys`, `size` bytes) as uncached memory; returns the virtual address.
pub fn map_mmio(phys: u64, size: usize) -> Result<u64, String> {
    let pages = (size + 4095) / 4096;
    let base = MMIO_NEXT.fetch_add((pages * 4096) as u64, Ordering::SeqCst);
    let mut mg = MAPPER.lock();
    let mut fg = FRAMES.lock();
    let (mapper, frames) = match (mg.as_mut(), fg.as_mut()) {
        (Some(m), Some(f)) => (m, f),
        _ => return Err("memory manager not ready".to_string()),
    };
    for i in 0..pages {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(base + (i * 4096) as u64));
        let frame = PhysFrame::containing_address(PhysAddr::new((phys & !0xFFF) + (i * 4096) as u64));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE | PageTableFlags::WRITE_THROUGH;
        unsafe {
            mapper
                .map_to(page, frame, flags, frames)
                .map_err(|_| "MMIO mapping failed".to_string())?
                .flush();
        }
    }
    Ok(base + (phys & 0xFFF))
}
