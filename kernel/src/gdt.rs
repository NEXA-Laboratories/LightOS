//! GDT + TSS: kernel code, user (ring 3) code/data, a dedicated double-fault stack and the
//! kernel stack the CPU switches to when an interrupt or system call arrives from ring 3.
use lazy_static::lazy_static;
use x86_64::instructions::segmentation::{Segment, CS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const KSTACK_SIZE: usize = 256 * 1024;
static mut KSTACK: [u8; KSTACK_SIZE] = [0; KSTACK_SIZE];

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
            stack_start + STACK_SIZE as u64
        };
        // rsp0: where the CPU lands when something interrupts ring 3 code
        tss.privilege_stack_table[0] = VirtAddr::from_ptr(core::ptr::addr_of!(KSTACK)) + KSTACK_SIZE as u64;
        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, user_data, user_code, tss_selector })
    };
}

struct Selectors {
    code_selector: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}

/// (code, stack) selectors for ring 3, with RPL = 3.
pub fn user_selectors() -> (u16, u16) {
    (GDT.1.user_code.0 | 3, GDT.1.user_data.0 | 3)
}
