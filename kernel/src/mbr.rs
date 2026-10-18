//! MBR partition tables (4 primary partitions, LBA addressing).
//!
//! The first SYSTEM_AREA sectors (8 MiB) of an installed disk hold the bootloader and kernel,
//! so partitions never start below it. The table lives in sector 0 bytes 446..510 and is only
//! written when that area is free (checked by `classify`).
use crate::ata::{Disk, SECTOR};
use alloc::{format, string::{String, ToString}, vec::Vec};

pub const SYSTEM_AREA: u32 = 16384;
pub const ALIGN: u32 = 2048; // 1 MiB
pub const TYPE_FAT32: u8 = 0x0C;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Part {
    pub boot: bool,
    pub ptype: u8,
    pub start: u32,
    pub sectors: u32,
}

pub type Table = [Option<Part>; 4];

pub enum Layout {
    /// Nothing in the partition-table area (blank disk, or a bootloader that leaves it free).
    Blank,
    Table(Table),
    /// Something else lives there (boot code of a raw filesystem, garbage...).
    Foreign,
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn classify(sec0: &[u8; SECTOR]) -> Layout {
    let area = &sec0[446..510];
    if area.iter().all(|&b| b == 0) {
        return Layout::Blank;
    }
    if sec0[510] != 0x55 || sec0[511] != 0xAA {
        return Layout::Foreign;
    }
    let mut t: Table = [None; 4];
    for i in 0..4 {
        let e = &area[i * 16..(i + 1) * 16];
        if e[0] != 0 && e[0] != 0x80 {
            return Layout::Foreign;
        }
        if e[4] == 0 {
            if e.iter().any(|&b| b != 0) {
                return Layout::Foreign;
            }
            continue;
        }
        let (start, n) = (le32(e, 8), le32(e, 12));
        if start == 0 || n == 0 {
            return Layout::Foreign;
        }
        t[i] = Some(Part { boot: e[0] == 0x80, ptype: e[4], start, sectors: n });
    }
    Layout::Table(t)
}

pub fn read_layout(disk: &Disk) -> Result<Layout, String> {
    let mut b = [0u8; SECTOR];
    disk.read_sector(0, &mut b).map_err(|e| e.to_string())?;
    Ok(classify(&b))
}

/// Write the table into a sector-0 image (and make sure the 0x55AA signature is present).
pub fn encode_into(sec0: &mut [u8; SECTOR], t: &Table) {
    for i in 0..4 {
        let o = 446 + i * 16;
        sec0[o..o + 16].fill(0);
        if let Some(p) = t[i] {
            sec0[o] = if p.boot { 0x80 } else { 0 };
            sec0[o + 1..o + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS: use LBA
            sec0[o + 4] = p.ptype;
            sec0[o + 5..o + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
            sec0[o + 8..o + 12].copy_from_slice(&p.start.to_le_bytes());
            sec0[o + 12..o + 16].copy_from_slice(&p.sectors.to_le_bytes());
        }
    }
    sec0[510] = 0x55;
    sec0[511] = 0xAA;
}

pub fn write_table(disk: &Disk, t: &Table) -> Result<(), String> {
    let mut b = [0u8; SECTOR];
    disk.read_sector(0, &mut b).map_err(|e| e.to_string())?;
    encode_into(&mut b, t);
    disk.write_sector(0, &b).map_err(|e| e.to_string())
}

fn align_up(v: u32) -> u32 {
    (v + ALIGN - 1) / ALIGN * ALIGN
}

/// Add a partition in the first gap that fits. `size_sectors == 0` means "all of the gap".
pub fn add_part(t: &mut Table, disk_sectors: u32, size_sectors: u32, ptype: u8) -> Result<usize, String> {
    let slot = t.iter().position(|p| p.is_none()).ok_or_else(|| String::from("partition table is full (4 primary partitions)"))?;
    let mut used: Vec<(u32, u32)> = t.iter().flatten().map(|p| (p.start, p.start + p.sectors)).collect();
    used.sort();
    let mut gaps: Vec<(u32, u32)> = Vec::new();
    let mut cursor = SYSTEM_AREA;
    for (s, e) in used {
        if s > cursor {
            gaps.push((cursor, s));
        }
        cursor = cursor.max(e);
    }
    if disk_sectors > cursor {
        gaps.push((cursor, disk_sectors));
    }
    for (gs, ge) in gaps {
        let s = align_up(gs);
        let e = ge / ALIGN * ALIGN;
        if e <= s {
            continue;
        }
        let avail = e - s;
        let want = if size_sectors == 0 { avail } else { align_up(size_sectors) };
        if want >= ALIGN && want <= avail {
            t[slot] = Some(Part { boot: slot == 0, ptype, start: s, sectors: want });
            return Ok(slot);
        }
    }
    Err(format!("no free space for that size after the {} MiB system area", SYSTEM_AREA / 2048))
}

pub fn type_name(t: u8) -> &'static str {
    match t {
        0x0B | 0x0C => "FAT32",
        0x83 => "Linux",
        0x07 => "NTFS/exFAT",
        0x82 => "Linux swap",
        _ => "other",
    }
}
