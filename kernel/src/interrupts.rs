//! IDT, CPU exceptions, PIC/PIT setup, hardware IRQs and the int 0x80 gate.
use crate::{gdt, hlt_loop, keyboard, process, syscall};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::{PrivilegeLevel, VirtAddr};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;
pub const SYSCALL_VECTOR: u8 = 0x80;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.segment_not_present.set_handler_fn(segment_not_present_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
            idt[SYSCALL_VECTOR]
                .set_handler_addr(VirtAddr::new(syscall::entry_addr()))
                .set_privilege_level(PrivilegeLevel::Ring3); // ring 3 apps may use int 0x80
        }
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[PIC_2_OFFSET + 6].set_handler_fn(ata_irq_handler); // IRQ14 (primary ATA)
        idt[PIC_2_OFFSET + 7].set_handler_fn(ata_irq_handler); // IRQ15 (secondary ATA)
        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

/// Program PIT channel 0 to fire `hz` times per second.
pub fn init_pit(hz: u32) {
    let divisor = (1_193_182 / hz) as u16;
    unsafe {
        Port::<u8>::new(0x43).write(0x36);
        Port::<u8>::new(0x40).write((divisor & 0xFF) as u8);
        Port::<u8>::new(0x40).write((divisor >> 8) as u8);
    }
}

/// Did this exception/interrupt come from ring 3 code?
fn from_user(sf: &InterruptStackFrame) -> bool {
    sf.code_segment.rpl() == PrivilegeLevel::Ring3
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    if from_user(&stack_frame) {
        crate::elf::user_fault("divide error", 0, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: DIVIDE ERROR\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    if from_user(&stack_frame) {
        crate::elf::user_fault("invalid instruction", 0, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn general_protection_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    if from_user(&stack_frame) {
        crate::elf::user_fault("general protection fault (forbidden instruction)", error_code, stack_frame.instruction_pointer.as_u64());
    }
    panic!("EXCEPTION: GENERAL PROTECTION FAULT (code {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn segment_not_present_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    panic!("EXCEPTION: SEGMENT NOT PRESENT (code {:#x})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn ata_irq_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(PIC_2_OFFSET + 6);
    }
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    use x86_64::registers::control::Cr2;
    if from_user(&stack_frame) {
        crate::elf::user_fault("page fault", Cr2::read_raw(), stack_frame.instruction_pointer.as_u64());
    }
    println!("EXCEPTION: PAGE FAULT");
    println!("Accessed address: {:?}", Cr2::read());
    println!("Error code: {:?}", error_code);
    println!("{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn timer_interrupt_handler(stack_frame: InterruptStackFrame) {
    process::on_tick();
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
    // Ctrl+C / Esc kills a user app that is hogging the CPU
    if from_user(&stack_frame) && crate::elf::user_should_stop() {
        crate::elf::user_abort("stopped by Ctrl+C / Esc");
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    keyboard::handle_scancode(scancode);
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}
