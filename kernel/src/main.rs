//! LightOS 0.4.0 - kernel entry point.
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

#[macro_use]
mod vga_buffer;
#[macro_use]
mod serial;
mod allocator;
mod animation;
mod ata;
mod e1000;
mod editor;
mod elf;
mod fat32;
mod gdt;
mod installer;
mod interrupts;
mod keyboard;
mod logo;
mod mbr;
mod net;
mod pci;
mod los;
mod memory;
mod process;
mod shell;
mod syscall;
mod ui;

use bootloader::{entry_point, BootInfo};
use alloc::format;
use core::panic::PanicInfo;
use vga_buffer::Color;

pub const VERSION: &str = "0.4.0";

entry_point!(kernel_main);

fn boot_ok(msg: &str) {
    vga_buffer::set_color(Color::LightGreen, Color::Black);
    print!("[OK] ");
    vga_buffer::set_color(Color::LightGray, Color::Black);
    println!("{}", msg);
    serial_println!("[OK] {}", msg);
}

fn boot_note(msg: &str) {
    vga_buffer::set_color(Color::Yellow, Color::Black);
    print!("[--] ");
    vga_buffer::set_color(Color::LightGray, Color::Black);
    println!("{}", msg);
    serial_println!("[--] {}", msg);
}

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // The animation needs the PIT timer, so bring up the minimum first.
    gdt::init();
    interrupts::init_idt();
    unsafe { interrupts::PICS.lock().initialize() };
    interrupts::init_pit(process::TICK_HZ as u32);
    x86_64::instructions::interrupts::enable();
    animation::play();

    vga_buffer::clear();
    println!("╔════════════════════════════════════════════╗");
    println!("║        LightOS v{} - Booting...         ║", VERSION);
    println!("╚════════════════════════════════════════════╝");

    boot_ok("GDT initialized");
    boot_ok("IDT initialized");
    boot_ok("PICs initialized");
    boot_ok("Interrupts enabled");
    memory::init(boot_info);
    boot_ok("Memory manager initialized");
    boot_ok("Heap initialized");
    let ndisks = ata::scan();
    boot_ok(&format!("ATA: {} disk(s) detected", ndisks));
    match fat32::mount_auto() {
        Ok(msg) => boot_ok(&msg),
        Err(e) => boot_note(&format!("{}; packages will not persist (see 'disks', 'install')", e)),
    }
    match e1000::probe() {
        Ok(msg) => boot_ok(&msg),
        Err(e) => boot_note(&format!("network: {}", e)),
    }
    process::init();
    boot_ok("Process manager initialized");
    syscall::init();
    boot_ok("System calls initialized");
    los::init();
    boot_ok("LOS package manager initialized");

    shell::run()
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    // The panic may have happened while a writer lock was held.
    unsafe {
        vga_buffer::WRITER.force_unlock();
        serial::SERIAL1.force_unlock();
    }
    vga_buffer::set_color(Color::White, Color::Red);
    println!("\n*** KERNEL PANIC ***\n{}", info);
    serial_println!("KERNEL PANIC: {}", info);
    hlt_loop()
}
