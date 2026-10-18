//! Minimal FAT32 driver over the ATA data disk.
//!
//! Supports: mount (raw volume or first MBR partition), list, read, write (create/overwrite),
//! mkdir, remove. Names are 8.3 (case-insensitive); long file names are skipped when listing.
use crate::ata::{self, Disk, SECTOR};
use crate::mbr::{self, Layout};
use alloc::{format, string::{String, ToString}, vec, vec::Vec};
use spin::Mutex;

const EOC: u32 = 0x0FFF_FFFF;
const MASK: u32 = 0x0FFF_FFFF;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_LFN: u8 = 0x0F;

pub struct Fat32 {
    disk: Disk,
    part_lba: u32,
    spc: u32,
    reserved: u32,
    nfats: u32,
    fat_size: u32,
    root: u32,
    data_start: u32,
    clusters: u32,
    next_free: u32,
}

pub struct DirInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

#[derive(Clone)]
struct Raw {
    idx: usize, // entry index within the directory data
    name: [u8; 11],
    attr: u8,
    cluster: u32,
    size: u32,
}

fn le16(b: &[u8], o: usize) -> u32 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u32
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn e(s: &'static str) -> String {
    s.to_string()
}

fn parse_entries(data: &[u8]) -> Vec<Raw> {
    let mut out = Vec::new();
    for (i, ch) in data.chunks(32).enumerate() {
        if ch.len() < 32 || ch[0] == 0x00 {
            break;
        }
        if ch[0] == 0xE5 || ch[11] == ATTR_LFN || ch[11] & ATTR_VOLUME != 0 {
            continue;
        }
        let mut name = [0u8; 11];
        name.copy_from_slice(&ch[0..11]);
        out.push(Raw {
            idx: i,
            name,
            attr: ch[11],
            cluster: (le16(ch, 20) << 16) | le16(ch, 26),
            size: le32(ch, 28),
        });
    }
    out
}

fn valid_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-_~!#$%&()@^{}".contains(&c)
}

fn to_83(name: &str) -> Result<[u8; 11], String> {
    let up = name.to_ascii_uppercase();
    let (base, ext) = match up.rsplit_once('.') {
        Some((b, x)) => (b, x),
        None => (up.as_str(), ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(format!("'{}' is not a valid 8.3 name (max 8 chars + 3 extension)", name));
    }
    if !base.bytes().chain(ext.bytes()).all(valid_char) {
        return Err(format!("'{}' contains characters not allowed in 8.3 names", name));
    }
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(out)
}

fn from_83(n: &[u8; 11]) -> String {
    let base: String = n[..8].iter().take_while(|&&c| c != b' ').map(|&c| c as char).collect();
    let ext: String = n[8..].iter().take_while(|&&c| c != b' ').map(|&c| c as char).collect();
    if ext.is_empty() { base } else { format!("{}.{}", base, ext) }
}

fn split_path(path: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_string()),
        }
    }
    Ok(out)
}

impl Fat32 {
    /// Mount the FAT32 volume whose boot sector is at `part_lba` (0 for a raw, unpartitioned disk).
    pub fn mount(disk: Disk, part_lba: u32) -> Result<Fat32, String> {
        let mut b = [0u8; SECTOR];
        disk.read_sector(part_lba, &mut b).map_err(|x| x.to_string())?;
        let bps = le16(&b, 11);
        let spc = b[13] as u32;
        let reserved = le16(&b, 14);
        let nfats = b[16] as u32;
        let root_ent = le16(&b, 17);
        let tot16 = le16(&b, 19);
        let tot32 = le32(&b, 32);
        let fat_size = le32(&b, 36);
        let root = le32(&b, 44);
        if bps != 512 || spc == 0 || !spc.is_power_of_two() || nfats == 0 || fat_size == 0 || root_ent != 0 {
            return Err(e("not a FAT32 volume (format it with: part mkfs <disk> <part>)"));
        }
        let total = if tot16 != 0 { tot16 } else { tot32 };
        let data_sectors = total.saturating_sub(reserved + nfats * fat_size);
        let clusters = data_sectors / spc;
        if clusters < 65525 || root < 2 {
            return Err(e("volume is not FAT32 (too few clusters)"));
        }
        Ok(Fat32 {
            disk,
            part_lba,
            spc,
            reserved,
            nfats,
            fat_size,
            root,
            data_start: part_lba + reserved + nfats * fat_size,
            clusters,
            next_free: 2,
        })
    }

    pub fn disk_index(&self) -> usize {
        self.disk.index
    }
    pub fn part_lba(&self) -> u32 {
        self.part_lba
    }

    // ------------------------------------------------------------------ low level
    fn rd(&self, lba: u32) -> Result<[u8; SECTOR], String> {
        let mut b = [0u8; SECTOR];
        self.disk.read_sector(lba, &mut b).map_err(|x| x.to_string())?;
        Ok(b)
    }
    fn wr(&self, lba: u32, b: &[u8; SECTOR]) -> Result<(), String> {
        self.disk.write_sector(lba, b).map_err(|x| x.to_string())
    }
    fn cluster_lba(&self, c: u32) -> u32 {
        self.data_start + (c - 2) * self.spc
    }
    fn cluster_bytes(&self) -> usize {
        self.spc as usize * SECTOR
    }

    fn fat_get(&self, c: u32) -> Result<u32, String> {
        let off = c * 4;
        let b = self.rd(self.part_lba + self.reserved + off / 512)?;
        Ok(le32(&b, (off % 512) as usize) & MASK)
    }

    fn fat_set(&self, c: u32, v: u32) -> Result<(), String> {
        let off = c * 4;
        let i = (off % 512) as usize;
        for k in 0..self.nfats {
            let lba = self.part_lba + self.reserved + k * self.fat_size + off / 512;
            let mut b = self.rd(lba)?;
            let old = le32(&b, i);
            let new = (old & !MASK) | (v & MASK);
            b[i..i + 4].copy_from_slice(&new.to_le_bytes());
            self.wr(lba, &b)?;
        }
        Ok(())
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>, String> {
        let mut out = Vec::new();
        let mut c = first;
        if c < 2 {
            return Ok(out);
        }
        loop {
            out.push(c);
            if out.len() as u32 > self.clusters {
                return Err(e("corrupt cluster chain (loop)"));
            }
            let next = self.fat_get(c)?;
            if next >= 0x0FFF_FFF8 {
                return Ok(out);
            }
            if next < 2 || next >= self.clusters + 2 {
                return Err(e("corrupt cluster chain"));
            }
            c = next;
        }
    }

    fn alloc_cluster(&mut self) -> Result<u32, String> {
        let end = self.clusters + 2;
        for step in 0..self.clusters {
            let c = 2 + (self.next_free - 2 + step) % self.clusters;
            if self.fat_get(c)? == 0 {
                self.fat_set(c, EOC)?;
                self.next_free = if c + 1 >= end { 2 } else { c + 1 };
                return Ok(c);
            }
        }
        Err(e("disk full"))
    }

    fn free_chain(&self, first: u32) -> Result<(), String> {
        for c in self.chain(first)? {
            self.fat_set(c, 0)?;
        }
        Ok(())
    }

    fn read_clusters(&self, chain: &[u32]) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(chain.len() * self.cluster_bytes());
        for &c in chain {
            for s in 0..self.spc {
                out.extend_from_slice(&self.rd(self.cluster_lba(c) + s)?);
            }
        }
        Ok(out)
    }

    fn write_clusters(&self, chain: &[u32], data: &[u8]) -> Result<(), String> {
        let mut pos = 0;
        for &c in chain {
            for s in 0..self.spc {
                let mut b = [0u8; SECTOR];
                let end = (pos + SECTOR).min(data.len());
                if pos < data.len() {
                    b[..end - pos].copy_from_slice(&data[pos..end]);
                }
                self.wr(self.cluster_lba(c) + s, &b)?;
                pos += SECTOR;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ directories
    fn lookup(&self, dir: u32, name: &[u8; 11]) -> Result<Option<Raw>, String> {
        let data = self.read_clusters(&self.chain(dir)?)?;
        Ok(parse_entries(&data).into_iter().find(|r| &r.name == name))
    }

    fn resolve_dir(&self, comps: &[String]) -> Result<u32, String> {
        let mut cur = self.root;
        for c in comps {
            let r = self.lookup(cur, &to_83(c)?)?.ok_or_else(|| format!("'{}' not found", c))?;
            if r.attr & ATTR_DIR == 0 {
                return Err(format!("'{}' is not a directory", c));
            }
            cur = if r.cluster < 2 { self.root } else { r.cluster };
        }
        Ok(cur)
    }

    /// Split into (parent directory cluster, final name).
    fn parent_of(&self, path: &str) -> Result<(u32, String), String> {
        let mut comps = split_path(path)?;
        let name = comps.pop().ok_or_else(|| e("path has no file name"))?;
        Ok((self.resolve_dir(&comps)?, name))
    }

    fn put_entry(&mut self, dir: u32, name: [u8; 11], attr: u8, cluster: u32, size: u32) -> Result<(), String> {
        let mut chain = self.chain(dir)?;
        let mut data = self.read_clusters(&chain)?;
        let mut slot = None;
        for (i, ch) in data.chunks(32).enumerate() {
            if ch[0] == 0x00 || ch[0] == 0xE5 {
                if slot.is_none() {
                    slot = Some(i);
                }
                if ch[0] == 0x00 {
                    break;
                }
            } else if ch[11] != ATTR_LFN && ch[11] & ATTR_VOLUME == 0 && ch[0..11] == name {
                slot = Some(i);
                break;
            }
        }
        let i = match slot {
            Some(i) => i,
            None => {
                let nc = self.alloc_cluster()?;
                let last = *chain.last().ok_or_else(|| e("directory has no clusters"))?;
                self.fat_set(last, nc)?;
                chain.push(nc);
                let at = data.len() / 32;
                data.extend(vec![0u8; self.cluster_bytes()]);
                at
            }
        };
        let o = i * 32;
        let ent = &mut data[o..o + 32];
        ent.fill(0);
        ent[0..11].copy_from_slice(&name);
        ent[11] = attr;
        let date: u16 = ((2026 - 1980) << 9) | (10 << 5) | 17;
        for off in [16usize, 18, 24] {
            ent[off..off + 2].copy_from_slice(&date.to_le_bytes());
        }
        ent[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        ent[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        ent[28..32].copy_from_slice(&size.to_le_bytes());
        self.write_clusters(&chain, &data)
    }

    // ------------------------------------------------------------------ public API
    pub fn list(&self, path: &str) -> Result<Vec<DirInfo>, String> {
        let dir = self.resolve_dir(&split_path(path)?)?;
        let data = self.read_clusters(&self.chain(dir)?)?;
        Ok(parse_entries(&data)
            .into_iter()
            .filter(|r| r.name[0] != b'.')
            .map(|r| DirInfo { name: from_83(&r.name), is_dir: r.attr & ATTR_DIR != 0, size: r.size })
            .collect())
    }

    pub fn is_dir(&self, path: &str) -> bool {
        split_path(path).and_then(|c| self.resolve_dir(&c)).is_ok()
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let (dir, name) = self.parent_of(path)?;
        let r = self.lookup(dir, &to_83(&name)?)?.ok_or_else(|| format!("'{}' not found", path))?;
        if r.attr & ATTR_DIR != 0 {
            return Err(format!("'{}' is a directory", path));
        }
        let mut data = self.read_clusters(&self.chain(r.cluster)?)?;
        data.truncate(r.size as usize);
        Ok(data)
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        let (dir, name) = self.parent_of(path)?;
        let n83 = to_83(&name)?;
        let old = self.lookup(dir, &n83)?;
        if let Some(o) = &old {
            if o.attr & ATTR_DIR != 0 {
                return Err(format!("'{}' is a directory", path));
            }
        }
        let cb = self.cluster_bytes();
        let need = (data.len() + cb - 1) / cb;
        let mut chain: Vec<u32> = Vec::new();
        for _ in 0..need {
            match self.alloc_cluster() {
                Ok(c) => {
                    if let Some(&prev) = chain.last() {
                        self.fat_set(prev, c)?;
                    }
                    chain.push(c);
                }
                Err(err) => {
                    for c in &chain {
                        let _ = self.fat_set(*c, 0);
                    }
                    return Err(err);
                }
            }
        }
        self.write_clusters(&chain, data)?;
        let first = chain.first().copied().unwrap_or(0);
        self.put_entry(dir, n83, ATTR_ARCHIVE, first, data.len() as u32)?;
        if let Some(o) = old {
            self.free_chain(o.cluster)?;
        }
        Ok(())
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let (dir, name) = self.parent_of(path)?;
        let n83 = to_83(&name)?;
        if self.lookup(dir, &n83)?.is_some() {
            return Err(format!("'{}' already exists", path));
        }
        let c = self.alloc_cluster()?;
        let mut buf = vec![0u8; self.cluster_bytes()];
        for (i, nm) in [b".          ", b"..         "].iter().enumerate() {
            let o = i * 32;
            buf[o..o + 11].copy_from_slice(&nm[..]);
            buf[o + 11] = ATTR_DIR;
            let target = if i == 0 { c } else if dir == self.root { 0 } else { dir };
            buf[o + 20..o + 22].copy_from_slice(&((target >> 16) as u16).to_le_bytes());
            buf[o + 26..o + 28].copy_from_slice(&(target as u16).to_le_bytes());
        }
        self.write_clusters(&[c], &buf)?;
        self.put_entry(dir, n83, ATTR_DIR, c, 0)
    }

    pub fn remove(&mut self, path: &str) -> Result<(), String> {
        let (dir, name) = self.parent_of(path)?;
        let r = self.lookup(dir, &to_83(&name)?)?.ok_or_else(|| format!("'{}' not found", path))?;
        if r.attr & ATTR_DIR != 0 {
            let inner = self.read_clusters(&self.chain(r.cluster)?)?;
            if parse_entries(&inner).iter().any(|x| x.name[0] != b'.') {
                return Err(format!("directory '{}' is not empty", path));
            }
        }
        if r.cluster >= 2 {
            self.free_chain(r.cluster)?;
        }
        let chain = self.chain(dir)?;
        let mut data = self.read_clusters(&chain)?;
        data[r.idx * 32] = 0xE5;
        self.write_clusters(&chain, &data)
    }

    /// (total clusters, free clusters, bytes per cluster)
    pub fn usage(&self) -> Result<(u32, u32, usize), String> {
        let mut free = 0u32;
        let limit = self.clusters + 2;
        for s in 0..self.fat_size {
            let b = self.rd(self.part_lba + self.reserved + s)?;
            for i in 0..128u32 {
                let idx = s * 128 + i;
                if idx >= 2 && idx < limit && le32(&b, (i * 4) as usize) & MASK == 0 {
                    free += 1;
                }
            }
        }
        Ok((self.clusters, free, self.cluster_bytes()))
    }
}

// ---------------------------------------------------------------------- global mount
static FS: Mutex<Option<Fat32>> = Mutex::new(None);

fn find_fs(d: &Disk) -> Option<(Fat32, String)> {
    let mut b = [0u8; SECTOR];
    d.read_sector(0, &mut b).ok()?;
    if let Layout::Table(t) = mbr::classify(&b) {
        for (i, p) in t.iter().enumerate() {
            if let Some(p) = p {
                if p.ptype == 0x0B || p.ptype == 0x0C {
                    if let Ok(fs) = Fat32::mount(*d, p.start) {
                        return Some((fs, format!("disk{} partition {} FAT32 ({} MiB)", d.index, i + 1, p.sectors / 2048)));
                    }
                }
            }
        }
        return None;
    }
    if &b[82..90] == b"FAT32   " {
        if let Ok(fs) = Fat32::mount(*d, 0) {
            return Some((fs, format!("disk{} FAT32 ({} MiB, raw)", d.index, d.sectors / 2048)));
        }
    }
    None
}

/// Mount the first FAT32 volume found on any disk (partitioned or raw).
pub fn mount_auto() -> Result<String, String> {
    let disks = ata::disks();
    if disks.is_empty() {
        return Err(e("no ATA disks found"));
    }
    for d in &disks {
        if let Some((fs, desc)) = find_fs(d) {
            *FS.lock() = Some(fs);
            return Ok(format!("mounted {}", desc));
        }
    }
    Err(e("no FAT32 volume on any disk (try 'disks', 'part', 'install')"))
}

/// Mount a specific volume: `part` 1-4, or 0 for a raw disk.
pub fn mount_part(disk_index: usize, part: usize) -> Result<String, String> {
    let d = ata::disk(disk_index).ok_or_else(|| format!("no disk {}", disk_index))?;
    let lba = if part == 0 {
        0
    } else {
        match mbr::read_layout(&d)? {
            Layout::Table(t) => t.get(part - 1).copied().flatten().ok_or_else(|| format!("disk {} has no partition {}", disk_index, part))?.start,
            _ => return Err(format!("disk {} has no partition table", disk_index)),
        }
    };
    let fs = Fat32::mount(d, lba)?;
    *FS.lock() = Some(fs);
    Ok(format!("mounted disk{} {}", disk_index, if part == 0 { String::from("(raw)") } else { format!("partition {}", part) }))
}

pub fn unmount() {
    *FS.lock() = None;
}

pub fn current_disk() -> Option<usize> {
    FS.lock().as_ref().map(|f| f.disk_index())
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// Create a fresh FAT32 filesystem in `sectors` sectors starting at `part_lba` (min ~40 MiB).
pub fn format(disk: &Disk, part_lba: u32, sectors: u32) -> Result<(), String> {
    if sectors < 80_000 {
        return Err(e("partition too small for FAT32 (need at least 40 MiB)"));
    }
    let wr = |lba: u32, b: &[u8; SECTOR]| disk.write_sector(part_lba + lba, b).map_err(|x| x.to_string());
    let mut spc: u32 = 1;
    while spc < 8 && sectors / (spc * 2) >= 70_000 {
        spc *= 2;
    }
    let reserved: u32 = 32;
    let nfats: u32 = 2;
    let mut fat: u32 = 1;
    loop {
        let clusters = (sectors - reserved - nfats * fat) / spc;
        let need = ((clusters + 2) * 4 + 511) / 512;
        if need <= fat {
            break;
        }
        fat = need;
    }
    let mut bs = [0u8; SECTOR];
    bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"LIGHTOS ");
    put16(&mut bs, 11, 512);
    bs[13] = spc as u8;
    put16(&mut bs, 14, reserved as u16);
    bs[16] = nfats as u8;
    bs[21] = 0xF8;
    put16(&mut bs, 24, 63);
    put16(&mut bs, 26, 255);
    put32(&mut bs, 28, part_lba);
    put32(&mut bs, 32, sectors);
    put32(&mut bs, 36, fat);
    put32(&mut bs, 44, 2);
    put16(&mut bs, 48, 1);
    put16(&mut bs, 50, 6);
    bs[64] = 0x80;
    bs[66] = 0x29;
    put32(&mut bs, 67, 0x4C4F_5330);
    bs[71..82].copy_from_slice(b"LIGHTOS    ");
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xAA;
    let mut fsinfo = [0u8; SECTOR];
    put32(&mut fsinfo, 0, 0x4161_5252);
    put32(&mut fsinfo, 484, 0x6141_7272);
    put32(&mut fsinfo, 488, 0xFFFF_FFFF);
    put32(&mut fsinfo, 492, 0xFFFF_FFFF);
    put32(&mut fsinfo, 508, 0xAA55_0000);
    for base in [0u32, 6] {
        wr(base, &bs)?;
        wr(base + 1, &fsinfo)?;
    }
    // the unused reserved sectors may hold stale data from a previous install
    let zero = [0u8; SECTOR];
    for s in [2u32, 3, 4, 5, 8, 9, 10, 11] {
        wr(s, &zero)?;
    }
    for k in 0..nfats {
        for s in 0..fat {
            let mut b = [0u8; SECTOR];
            if s == 0 {
                put32(&mut b, 0, 0x0FFF_FFF8);
                put32(&mut b, 4, 0x0FFF_FFFF);
                put32(&mut b, 8, 0x0FFF_FFFF); // cluster 2 = root directory, end of chain
            }
            wr(reserved + k * fat + s, &b)?;
            if s % 1024 == 0 {
                print!(".");
            }
        }
    }
    let data_start = reserved + nfats * fat;
    for s in 0..spc {
        wr(data_start + s, &zero)?; // empty root directory
    }
    println!();
    Ok(())
}

pub fn is_mounted() -> bool {
    FS.lock().is_some()
}

pub fn with<R>(f: impl FnOnce(&mut Fat32) -> Result<R, String>) -> Result<R, String> {
    let mut g = FS.lock();
    match g.as_mut() {
        Some(fs) => f(fs),
        None => Err(e("no filesystem mounted")),
    }
}
