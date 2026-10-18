//! ATA PIO driver for up to four IDE disks (primary/secondary channel, master/slave).
//! Disk numbers follow the IDE position: 0 = primary master (the boot medium),
//! 1 = primary slave, 2 = secondary master, 3 = secondary slave.
use alloc::{string::String, vec::Vec};
use spin::Mutex;
use x86_64::instructions::port::Port;

const ST_ERR: u8 = 0x01;
const ST_DRQ: u8 = 0x08;
const ST_DF: u8 = 0x20;
const ST_BSY: u8 = 0x80;

pub const SECTOR: usize = 512;
const CHANNELS: [(u16, u16); 2] = [(0x1F0, 0x3F6), (0x170, 0x376)];
const POSITIONS: [&str; 4] = ["pri-master", "pri-slave", "sec-master", "sec-slave"];

#[derive(Clone, Copy)]
pub struct Disk {
    pub index: usize,
    base: u16,
    ctrl: u16,
    slave: bool,
    pub sectors: u32,
    model: [u8; 40],
}

fn inb(p: u16) -> u8 {
    unsafe { Port::<u8>::new(p).read() }
}
fn outb(p: u16, v: u8) {
    unsafe { Port::<u8>::new(p).write(v) }
}
fn inw(p: u16) -> u16 {
    unsafe { Port::<u16>::new(p).read() }
}
fn outw(p: u16, v: u16) {
    unsafe { Port::<u16>::new(p).write(v) }
}

impl Disk {
    // register offsets from the channel base: 0 data, 1 error, 2 count, 3..5 lba, 6 drive, 7 status/command
    fn r(&self, off: u16) -> u16 {
        self.base + off
    }

    fn delay(&self) {
        for _ in 0..4 {
            inb(self.ctrl);
        }
    }

    fn wait_ready(&self, spins: u32) -> Result<u8, &'static str> {
        for _ in 0..spins {
            let s = inb(self.r(7));
            if s & ST_BSY == 0 {
                return Ok(s);
            }
        }
        Err("ATA timeout (busy)")
    }

    fn wait_drq(&self) -> Result<(), &'static str> {
        for _ in 0..4_000_000 {
            let s = inb(self.r(7));
            if s & ST_BSY == 0 {
                if s & (ST_ERR | ST_DF) != 0 {
                    return Err("ATA device error");
                }
                if s & ST_DRQ != 0 {
                    return Ok(());
                }
            }
        }
        Err("ATA timeout (drq)")
    }

    fn select(&self, lba_top: u8) {
        outb(self.r(6), 0xE0 | ((self.slave as u8) << 4) | (lba_top & 0x0F));
        self.delay();
    }

    pub fn position(&self) -> &'static str {
        POSITIONS[self.index]
    }

    pub fn model(&self) -> String {
        let s: String = self.model.iter().map(|&b| if (32..127).contains(&b) { b as char } else { ' ' }).collect();
        String::from(s.trim())
    }

    fn probe(index: usize) -> Option<Disk> {
        let (base, ctrl) = CHANNELS[index / 2];
        let mut d = Disk { index, base, ctrl, slave: index % 2 == 1, sectors: 0, model: [b' '; 40] };
        outb(ctrl, 0x02); // nIEN: never raise IRQ14/15, we poll
        d.select(0);
        if inb(d.r(7)) == 0xFF {
            return None; // floating bus: no controller/device
        }
        outb(d.r(2), 0);
        outb(d.r(3), 0);
        outb(d.r(4), 0);
        outb(d.r(5), 0);
        outb(d.r(7), 0xEC); // IDENTIFY
        if inb(d.r(7)) == 0 {
            return None;
        }
        d.wait_ready(400_000).ok()?;
        if inb(d.r(4)) != 0 || inb(d.r(5)) != 0 {
            return None; // ATAPI / SATA signature
        }
        d.wait_drq().ok()?;
        let mut words = [0u16; 256];
        for w in words.iter_mut() {
            *w = inw(d.r(0));
        }
        d.sectors = (words[60] as u32) | ((words[61] as u32) << 16);
        if d.sectors == 0 {
            return None;
        }
        for i in 0..20 {
            let w = words[27 + i];
            d.model[i * 2] = (w >> 8) as u8;
            d.model[i * 2 + 1] = w as u8;
        }
        Some(d)
    }

    fn setup(&self, lba: u32, cmd: u8) -> Result<(), &'static str> {
        if lba >= self.sectors {
            return Err("LBA out of range");
        }
        self.wait_ready(4_000_000)?;
        self.select((lba >> 24) as u8);
        outb(self.r(1), 0);
        outb(self.r(2), 1);
        outb(self.r(3), lba as u8);
        outb(self.r(4), (lba >> 8) as u8);
        outb(self.r(5), (lba >> 16) as u8);
        outb(self.r(7), cmd);
        Ok(())
    }

    pub fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR]) -> Result<(), &'static str> {
        self.setup(lba, 0x20)?;
        self.wait_drq()?;
        for i in 0..256 {
            let w = inw(self.r(0));
            buf[i * 2] = w as u8;
            buf[i * 2 + 1] = (w >> 8) as u8;
        }
        Ok(())
    }

    pub fn write_sector(&self, lba: u32, buf: &[u8; SECTOR]) -> Result<(), &'static str> {
        self.setup(lba, 0x30)?;
        self.wait_drq()?;
        for i in 0..256 {
            outw(self.r(0), (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8));
        }
        outb(self.r(7), 0xE7); // flush cache
        self.wait_ready(4_000_000)?;
        Ok(())
    }
}

static DISKS: Mutex<Vec<Disk>> = Mutex::new(Vec::new());

/// Probe all four IDE positions; returns how many disks were found.
pub fn scan() -> usize {
    let mut found: Vec<Disk> = Vec::new();
    for idx in 0..4 {
        if let Some(d) = Disk::probe(idx) {
            // some controllers echo the master when the slave is absent
            if idx % 2 == 1 {
                if let Some(m) = found.iter().find(|x| x.index == idx - 1) {
                    if m.model == d.model && m.sectors == d.sectors {
                        continue;
                    }
                }
            }
            found.push(d);
        }
    }
    let n = found.len();
    *DISKS.lock() = found;
    n
}

pub fn disks() -> Vec<Disk> {
    DISKS.lock().clone()
}

pub fn disk(index: usize) -> Option<Disk> {
    DISKS.lock().iter().find(|d| d.index == index).copied()
}
