//! PCI configuration space access (legacy 0xCF8/0xCFC mechanism) and bus scan.
use alloc::vec::Vec;
use x86_64::instructions::port::Port;

#[derive(Clone, Copy)]
pub struct Dev {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
}

pub fn read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr: u32 = 0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (func as u32) << 8 | (off as u32 & 0xFC);
    unsafe {
        Port::<u32>::new(0xCF8).write(addr);
        Port::<u32>::new(0xCFC).read()
    }
}

pub fn write32(bus: u8, dev: u8, func: u8, off: u8, v: u32) {
    let addr: u32 = 0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (func as u32) << 8 | (off as u32 & 0xFC);
    unsafe {
        Port::<u32>::new(0xCF8).write(addr);
        Port::<u32>::new(0xCFC).write(v);
    }
}

/// Enable I/O, memory decoding and bus mastering (needed for DMA).
pub fn enable_bus_master(d: &Dev) {
    let cmd = read32(d.bus, d.dev, d.func, 0x04) & 0xFFFF; // keep the status half zero (write-1-to-clear)
    write32(d.bus, d.dev, d.func, 0x04, cmd | 0x07);
}

pub fn scan() -> Vec<Dev> {
    let mut out = Vec::new();
    for bus in 0..8u8 {
        for dev in 0..32u8 {
            if read32(bus, dev, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let header = (read32(bus, dev, 0, 0x0C) >> 16) as u8;
            let funcs = if header & 0x80 != 0 { 8 } else { 1 };
            for func in 0..funcs {
                let id = read32(bus, dev, func, 0);
                if id & 0xFFFF == 0xFFFF {
                    continue;
                }
                let cls = read32(bus, dev, func, 0x08);
                out.push(Dev { bus, dev, func, vendor: id as u16, device: (id >> 16) as u16, class: (cls >> 24) as u8, subclass: (cls >> 16) as u8 });
            }
        }
    }
    out
}

pub fn class_name(class: u8, sub: u8) -> &'static str {
    match (class, sub) {
        (0x01, 0x01) => "IDE controller",
        (0x01, 0x06) => "SATA controller",
        (0x01, _) => "storage",
        (0x02, 0x00) => "ethernet",
        (0x02, _) => "network",
        (0x03, _) => "display",
        (0x04, _) => "multimedia",
        (0x06, 0x00) => "host bridge",
        (0x06, 0x01) => "ISA bridge",
        (0x06, _) => "bridge",
        (0x0C, 0x03) => "USB controller",
        (0x08, _) => "system",
        _ => "other",
    }
}
