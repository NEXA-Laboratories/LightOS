//! Intel 82540EM-family (e1000) network driver: polled RX/TX with DMA descriptor rings.
//! This is the NIC VirtualBox ("Intel PRO/1000 MT Desktop") and QEMU (`-nic user,model=e1000`) emulate.
use crate::{memory, pci, process::sleep_ms};
use alloc::{alloc::{alloc_zeroed, Layout}, format, string::{String, ToString}};
use core::ptr::{null_mut, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;
use x86_64::VirtAddr;

const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_EERD: u32 = 0x0014;
const REG_ICR: u32 = 0x00C0;
const REG_IMC: u32 = 0x00D8;
const REG_RCTL: u32 = 0x0100;
const REG_TCTL: u32 = 0x0400;
const REG_TIPG: u32 = 0x0410;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;
const REG_MTA: u32 = 0x5200;
const REG_RAL0: u32 = 0x5400;
const REG_RAH0: u32 = 0x5404;

const CTRL_LRST: u32 = 1 << 3;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_ILOS: u32 = 1 << 7;
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;

const RX_N: usize = 32;
const TX_N: usize = 16;
const BUF: usize = 2048;

const SUPPORTED: [u16; 4] = [0x100E, 0x100F, 0x1004, 0x1008];

#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    csum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

pub struct E1000 {
    mmio: u64,
    rx_ring: *mut RxDesc,
    tx_ring: *mut TxDesc,
    rx_buf: [u64; RX_N],
    tx_buf: [u64; TX_N],
    rx_cur: usize,
    tx_cur: usize,
    pub mac: [u8; 6],
    pub pci_id: (u16, u16),
}

unsafe impl Send for E1000 {}

fn pages(n: usize) -> Result<*mut u8, String> {
    let layout = Layout::from_size_align(n * 4096, 4096).map_err(|_| "bad layout".to_string())?;
    let p = unsafe { alloc_zeroed(layout) };
    if p.is_null() {
        Err("out of memory for NIC buffers".to_string())
    } else {
        Ok(p)
    }
}

fn phys(p: u64) -> Result<u64, String> {
    memory::translate(VirtAddr::new(p)).map(|a| a.as_u64()).ok_or_else(|| "cannot translate DMA address".to_string())
}

impl E1000 {
    fn rd(&self, reg: u32) -> u32 {
        unsafe { read_volatile((self.mmio + reg as u64) as *const u32) }
    }
    fn wr(&self, reg: u32, v: u32) {
        unsafe { write_volatile((self.mmio + reg as u64) as *mut u32, v) }
    }

    fn eeprom(&self, word: u32) -> Option<u16> {
        self.wr(REG_EERD, 1 | (word << 8));
        for _ in 0..200_000 {
            let v = self.rd(REG_EERD);
            if v & 0x10 != 0 {
                return Some((v >> 16) as u16);
            }
        }
        None
    }

    pub fn link_up(&self) -> bool {
        self.rd(REG_STATUS) & 2 != 0
    }

    pub fn init(d: pci::Dev) -> Result<E1000, String> {
        pci::enable_bus_master(&d);
        let bar0 = pci::read32(d.bus, d.dev, d.func, 0x10);
        if bar0 & 1 != 0 {
            return Err("BAR0 is I/O space (unsupported)".to_string());
        }
        let mmio = memory::map_mmio((bar0 & !0xF) as u64, 0x20000)?;
        let mut n = E1000 {
            mmio,
            rx_ring: null_mut(),
            tx_ring: null_mut(),
            rx_buf: [0; RX_N],
            tx_buf: [0; TX_N],
            rx_cur: 0,
            tx_cur: 0,
            mac: [0; 6],
            pci_id: (d.vendor, d.device),
        };

        n.wr(REG_IMC, 0xFFFF_FFFF);
        n.wr(REG_CTRL, n.rd(REG_CTRL) | CTRL_RST);
        sleep_ms(30);
        n.wr(REG_IMC, 0xFFFF_FFFF);
        n.rd(REG_ICR);
        let ctrl = (n.rd(REG_CTRL) | CTRL_SLU | CTRL_ASDE) & !(CTRL_LRST | CTRL_ILOS | CTRL_PHY_RST);
        n.wr(REG_CTRL, ctrl);

        // MAC address: receive-address registers (preloaded from the EEPROM), else read the EEPROM.
        let (ral, rah) = (n.rd(REG_RAL0), n.rd(REG_RAH0));
        if ral != 0 || (rah & 0xFFFF) != 0 {
            n.mac[0..4].copy_from_slice(&ral.to_le_bytes());
            n.mac[4..6].copy_from_slice(&(rah as u16).to_le_bytes());
        } else {
            for i in 0..3 {
                let w = n.eeprom(i).ok_or_else(|| "cannot read the MAC address".to_string())?;
                n.mac[i as usize * 2] = w as u8;
                n.mac[i as usize * 2 + 1] = (w >> 8) as u8;
            }
            n.wr(REG_RAL0, u32::from_le_bytes([n.mac[0], n.mac[1], n.mac[2], n.mac[3]]));
            n.wr(REG_RAH0, u32::from_le_bytes([n.mac[4], n.mac[5], 0, 0]) | (1 << 31));
        }
        for i in 0..128u32 {
            n.wr(REG_MTA + i * 4, 0);
        }

        // RX ring
        n.rx_ring = pages(1)? as *mut RxDesc;
        let rx_mem = pages(RX_N * BUF / 4096)? as u64;
        for i in 0..RX_N {
            let virt = rx_mem + (i * BUF) as u64;
            n.rx_buf[i] = virt;
            let desc = RxDesc { addr: phys(virt)?, length: 0, csum: 0, status: 0, errors: 0, special: 0 };
            unsafe { write_volatile(n.rx_ring.add(i), desc) };
        }
        let rp = phys(n.rx_ring as u64)?;
        n.wr(REG_RDBAL, rp as u32);
        n.wr(REG_RDBAH, (rp >> 32) as u32);
        n.wr(REG_RDLEN, (RX_N * 16) as u32);
        n.wr(REG_RDH, 0);
        n.wr(REG_RDT, (RX_N - 1) as u32);
        n.wr(REG_RCTL, (1 << 1) | (1 << 15) | (1 << 26)); // EN | BAM (broadcast) | SECRC, 2048-byte buffers

        // TX ring
        n.tx_ring = pages(1)? as *mut TxDesc;
        let tx_mem = pages(TX_N * BUF / 4096)? as u64;
        for i in 0..TX_N {
            let virt = tx_mem + (i * BUF) as u64;
            n.tx_buf[i] = virt;
            let desc = TxDesc { addr: phys(virt)?, length: 0, cso: 0, cmd: 0, status: 1, css: 0, special: 0 }; // DD set: free
            unsafe { write_volatile(n.tx_ring.add(i), desc) };
        }
        let tp = phys(n.tx_ring as u64)?;
        n.wr(REG_TDBAL, tp as u32);
        n.wr(REG_TDBAH, (tp >> 32) as u32);
        n.wr(REG_TDLEN, (TX_N * 16) as u32);
        n.wr(REG_TDH, 0);
        n.wr(REG_TDT, 0);
        n.wr(REG_TCTL, (1 << 1) | (1 << 3) | (0x0F << 4) | (0x40 << 12)); // EN | PSP | CT | COLD
        n.wr(REG_TIPG, 0x0060_200A);
        Ok(n)
    }

    /// Fetch one received frame into `out`. Returns its length (0 for a damaged frame).
    pub fn recv(&mut self, out: &mut [u8]) -> Option<usize> {
        let cur = self.rx_cur;
        let d = unsafe { read_volatile(self.rx_ring.add(cur)) };
        if d.status & 1 == 0 {
            return None;
        }
        fence(Ordering::SeqCst);
        let mut len = 0;
        if d.errors == 0 {
            len = (d.length as usize).min(out.len());
            unsafe { core::ptr::copy_nonoverlapping(self.rx_buf[cur] as *const u8, out.as_mut_ptr(), len) };
        }
        let fresh = RxDesc { addr: d.addr, length: 0, csum: 0, status: 0, errors: 0, special: 0 };
        unsafe { write_volatile(self.rx_ring.add(cur), fresh) };
        fence(Ordering::SeqCst);
        self.wr(REG_RDT, cur as u32);
        self.rx_cur = (cur + 1) % RX_N;
        Some(len)
    }

    pub fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        if frame.is_empty() || frame.len() > 1514 {
            return Err("bad frame size".to_string());
        }
        let i = self.tx_cur;
        let mut free = false;
        for _ in 0..2_000_000 {
            let d = unsafe { read_volatile(self.tx_ring.add(i)) };
            if d.status & 1 != 0 {
                free = true;
                break;
            }
        }
        if !free {
            return Err("transmit ring stuck".to_string());
        }
        unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), self.tx_buf[i] as *mut u8, frame.len()) };
        let desc = TxDesc { addr: phys(self.tx_buf[i])?, length: frame.len() as u16, cso: 0, cmd: 0x0B, status: 0, css: 0, special: 0 };
        unsafe { write_volatile(self.tx_ring.add(i), desc) };
        fence(Ordering::SeqCst);
        self.tx_cur = (i + 1) % TX_N;
        self.wr(REG_TDT, self.tx_cur as u32);
        Ok(())
    }
}

static NIC: Mutex<Option<E1000>> = Mutex::new(None);

/// Run `f` on the NIC, or return None if there is none.
pub fn with<R>(f: impl FnOnce(&mut E1000) -> R) -> Option<R> {
    NIC.lock().as_mut().map(f)
}

pub fn probe() -> Result<String, String> {
    let devs = pci::scan();
    let d = devs
        .iter()
        .find(|d| d.vendor == 0x8086 && SUPPORTED.contains(&d.device))
        .copied()
        .ok_or_else(|| "no Intel e1000 network card (VirtualBox: Adapter type 'Intel PRO/1000 MT Desktop')".to_string())?;
    let nic = E1000::init(d)?;
    let m = nic.mac;
    let msg = format!(
        "network: Intel e1000 {:04x}:{:04x} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        d.vendor, d.device, m[0], m[1], m[2], m[3], m[4], m[5]
    );
    *NIC.lock() = Some(nic);
    Ok(msg)
}

/// Wait for the link to come up (VirtualBox delays it by ~3 s after a reset).
pub fn wait_link(timeout_ms: u64) -> bool {
    let mut waited = 0;
    loop {
        if with(|n| n.link_up()).unwrap_or(false) {
            return true;
        }
        if waited >= timeout_ms {
            return false;
        }
        sleep_ms(50);
        waited += 50;
    }
}
