//! Disk tools: `disks`, `part ...` (partition editor) and `install <disk>` (system installer).
use crate::ata::{self, Disk, SECTOR};
use crate::fat32::{self, Fat32};
use crate::mbr::{self, Layout, Table, SYSTEM_AREA, TYPE_FAT32};
use crate::{shell, ui};
use alloc::{format, string::{String, ToString}, vec::Vec};

const MIN_INSTALL_SECTORS: u32 = SYSTEM_AREA + 40 * 2048;

fn disk_arg(args: &[&str], i: usize) -> Result<Disk, String> {
    let n: usize = args.get(i).and_then(|s| s.parse().ok()).ok_or_else(|| String::from("missing disk number (see 'disks')"))?;
    ata::disk(n).ok_or_else(|| format!("no disk {}", n))
}

fn state_of(d: &Disk) -> String {
    match mbr::read_layout(d) {
        Ok(Layout::Blank) => String::from("no partition table"),
        Ok(Layout::Table(t)) => format!("MBR, {} partition(s)", t.iter().flatten().count()),
        Ok(Layout::Foreign) => String::from("boot/other data"),
        Err(_) => String::from("unreadable"),
    }
}

pub fn disks_cmd() {
    let ds = ata::disks();
    if ds.is_empty() {
        println!("No ATA disks found. (VirtualBox: use the IDE controller; SATA/AHCI is not supported yet.)");
        return;
    }
    let rows: Vec<String> = ds
        .iter()
        .map(|d| {
            format!(
                "{}  {:<10} {:>6} MiB  {:<18} {}{}",
                d.index,
                d.position(),
                d.sectors / 2048,
                d.model().chars().take(18).collect::<String>(),
                state_of(d),
                if d.index == 0 { "  (boot medium)" } else { "" }
            )
        })
        .collect();
    ui::table("Disks", Some("#  Position   Size        Model              Layout"), &rows);
}

fn load_table(d: &Disk) -> Result<Table, String> {
    match mbr::read_layout(d)? {
        Layout::Blank => Ok([None; 4]),
        Layout::Table(t) => Ok(t),
        Layout::Foreign => Err(format!(
            "disk {} has other data in the partition-table area of its boot sector{}",
            d.index,
            if d.index == 0 { "; refusing to touch the boot medium" } else { "; run 'part wipe' first (this erases the table area)" }
        )),
    }
}

fn parse_size(s: &str) -> Result<u32, String> {
    if s == "max" {
        return Ok(0);
    }
    let mib: u32 = s.parse().map_err(|_| String::from("size must be a number of MiB or 'max'"))?;
    mib.checked_mul(2048).ok_or_else(|| String::from("size too large"))
}

fn print_table(d: &Disk, t: &Table) {
    let rows: Vec<String> = t
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            p.map(|p| {
                format!(
                    "{}  {}  {:<11} {:>10} {:>8} MiB",
                    i + 1,
                    if p.boot { "*" } else { " " },
                    mbr::type_name(p.ptype),
                    p.start,
                    p.sectors / 2048
                )
            })
        })
        .collect();
    if rows.is_empty() {
        println!("disk {}: no partitions (first {} MiB are reserved for the LightOS system area)", d.index, SYSTEM_AREA / 2048);
    } else {
        ui::table(&format!("Partitions of disk {}", d.index), Some("#  Bt Type        Start LBA     Size"), &rows);
    }
}

pub fn part_cmd(args: &[&str]) {
    if let Err(e) = part(args) {
        println!("error: {}", e);
    }
}

fn part_usage() {
    println!("part list <disk>");
    println!("part new <disk> <MiB|max>      create a FAT32 partition after the 8 MiB system area");
    println!("part delete <disk> <n>");
    println!("part wipe <disk>               remove the partition table");
    println!("part mkfs <disk> <n>           format partition n as FAT32 (erases it)");
}

fn part(args: &[&str]) -> Result<(), String> {
    let Some(&sub) = args.first() else {
        part_usage();
        return Ok(());
    };
    match sub {
        "list" | "ls" => {
            let d = disk_arg(args, 1)?;
            let t = load_table(&d)?;
            print_table(&d, &t);
        }
        "new" | "add" => {
            let d = disk_arg(args, 1)?;
            let size = parse_size(args.get(2).copied().ok_or_else(|| String::from("usage: part new <disk> <MiB|max>"))?)?;
            let mut t = load_table(&d)?;
            let n = mbr::add_part(&mut t, d.sectors, size, TYPE_FAT32)?;
            mbr::write_table(&d, &t)?;
            println!("created partition {} on disk {} - format it with: part mkfs {} {}", n + 1, d.index, d.index, n + 1);
            print_table(&d, &t);
        }
        "delete" | "del" | "rm" => {
            let d = disk_arg(args, 1)?;
            let n: usize = args.get(2).and_then(|s| s.parse().ok()).ok_or_else(|| String::from("usage: part delete <disk> <n>"))?;
            let mut t = load_table(&d)?;
            if n == 0 || n > 4 || t[n - 1].is_none() {
                return Err(format!("disk {} has no partition {}", d.index, n));
            }
            if !shell::confirm(&format!("Delete partition {} of disk {}? Type YES", n, d.index)) {
                return Err(String::from("cancelled"));
            }
            t[n - 1] = None;
            mbr::write_table(&d, &t)?;
            if fat32::current_disk() == Some(d.index) {
                fat32::unmount();
            }
            println!("partition {} deleted", n);
        }
        "wipe" => {
            let d = disk_arg(args, 1)?;
            if d.index == 0 && !matches!(mbr::read_layout(&d)?, Layout::Blank | Layout::Table(_)) {
                return Err(String::from("refusing to wipe the boot medium"));
            }
            if !shell::confirm(&format!("Remove the partition table of disk {}? Type YES", d.index)) {
                return Err(String::from("cancelled"));
            }
            mbr::write_table(&d, &[None; 4])?;
            if fat32::current_disk() == Some(d.index) {
                fat32::unmount();
            }
            println!("partition table of disk {} cleared", d.index);
        }
        "mkfs" | "format" => {
            let d = disk_arg(args, 1)?;
            let n: usize = args.get(2).and_then(|s| s.parse().ok()).ok_or_else(|| String::from("usage: part mkfs <disk> <n>"))?;
            let t = load_table(&d)?;
            let p = t.get(n.wrapping_sub(1)).copied().flatten().ok_or_else(|| format!("disk {} has no partition {}", d.index, n))?;
            if !shell::confirm(&format!("Format partition {} of disk {} ({} MiB)? ALL DATA WILL BE LOST. Type YES", n, d.index, p.sectors / 2048)) {
                return Err(String::from("cancelled"));
            }
            if fat32::current_disk() == Some(d.index) {
                fat32::unmount();
            }
            fat32::format(&d, p.start, p.sectors)?;
            println!("done. Mount it with: mount {} {}", d.index, n);
        }
        _ => part_usage(),
    }
    Ok(())
}

// ------------------------------------------------------------------ installer

fn copy_tree(src: &Fat32, dst: &mut Fat32, dir: &str) -> Result<usize, String> {
    let mut n = 0;
    for e in src.list(dir)? {
        let path = if dir == "/" { format!("/{}", e.name) } else { format!("{}/{}", dir, e.name) };
        if e.is_dir {
            dst.mkdir(&path)?;
            n += copy_tree(src, dst, &path)?;
        } else {
            let data = src.read_file(&path)?;
            dst.write_file(&path, &data)?;
            n += 1;
        }
    }
    Ok(n)
}

pub fn install_cmd(args: &[&str]) {
    let Some(idx) = args.first().and_then(|s| s.parse::<usize>().ok()) else {
        println!("usage: install <disk>    (see 'disks'; disk 0 is the boot medium and cannot be the target)");
        return;
    };
    if let Err(e) = install(idx) {
        println!("error: {}", e);
    }
}

fn install(idx: usize) -> Result<(), String> {
    let src = ata::disk(0).ok_or_else(|| String::from("no boot medium (disk 0) found"))?;
    if idx == 0 {
        return Err(String::from("disk 0 is the medium you booted from; choose another disk (see 'disks')"));
    }
    let dst = ata::disk(idx).ok_or_else(|| format!("no disk {}", idx))?;
    if dst.sectors < MIN_INSTALL_SECTORS {
        return Err(format!("disk {} is too small ({} MiB); need at least {} MiB", idx, dst.sectors / 2048, MIN_INSTALL_SECTORS / 2048));
    }

    let mut s0 = [0u8; SECTOR];
    src.read_sector(0, &mut s0).map_err(|e| e.to_string())?;
    if s0[510] != 0x55 || s0[511] != 0xAA {
        return Err(String::from("disk 0 has no boot sector"));
    }
    if let Layout::Foreign = mbr::classify(&s0) {
        return Err(String::from(
            "the boot sector of disk 0 uses the partition-table area, so a table cannot be stored next to it. \
             Run tools/mkboot.py on your bootimage and report its output.",
        ));
    }

    // Decide the partition layout of the target.
    let (table, part_no, fresh) = match mbr::read_layout(&dst)? {
        Layout::Table(mut t) => {
            if t.iter().flatten().any(|p| p.start < SYSTEM_AREA) {
                return Err(String::from("an existing partition overlaps the 8 MiB system area; use 'part wipe' on that disk first"));
            }
            let n = match t.iter().position(|p| matches!(p, Some(p) if p.ptype == 0x0B || p.ptype == 0x0C)) {
                Some(n) => n,
                None => mbr::add_part(&mut t, dst.sectors, 0, TYPE_FAT32)?,
            };
            (t, n, false)
        }
        _ => {
            let mut t: Table = [None; 4];
            let n = mbr::add_part(&mut t, dst.sectors, 0, TYPE_FAT32)?;
            (t, n, true)
        }
    };
    let part = table[part_no].ok_or_else(|| String::from("internal error: no target partition"))?;

    println!("LightOS {} installer", crate::VERSION);
    println!("  target:    disk {} ({} MiB, {})", idx, dst.sectors / 2048, dst.model());
    println!("  system:    bootloader + kernel -> first {} MiB", SYSTEM_AREA / 2048);
    println!(
        "  partition: {} {} (FAT32, {} MiB, LBA {}) will be {}",
        part_no + 1,
        if fresh { "of a NEW table" } else { "of the existing table" },
        part.sectors / 2048,
        part.start,
        "FORMATTED"
    );
    if fat32::current_disk().is_some() && fat32::current_disk() != Some(idx) {
        println!("  data:      the mounted volume's files (packages, apps) will be copied");
    }
    if !shell::confirm("ALL DATA on that partition is lost. Type YES to install") {
        return Err(String::from("cancelled"));
    }

    // 1) system area
    let n = src.sectors.min(SYSTEM_AREA);
    print!("Writing system area");
    let mut buf = [0u8; SECTOR];
    for lba in 0..n {
        src.read_sector(lba, &mut buf).map_err(|e| e.to_string())?;
        if lba == 0 {
            mbr::encode_into(&mut buf, &table);
        }
        dst.write_sector(lba, &buf).map_err(|e| e.to_string())?;
        if lba % 2048 == 0 {
            print!(".");
        }
    }
    println!();
    print!("Verifying");
    let mut chk = [0u8; SECTOR];
    for lba in 0..n {
        src.read_sector(lba, &mut buf).map_err(|e| e.to_string())?;
        if lba == 0 {
            mbr::encode_into(&mut buf, &table);
        }
        dst.read_sector(lba, &mut chk).map_err(|e| e.to_string())?;
        if buf != chk {
            return Err(format!("verification failed at sector {}", lba));
        }
        if lba % 2048 == 0 {
            print!(".");
        }
    }
    println!(" ok");

    // 2) filesystem
    let live_disk = fat32::current_disk();
    if live_disk == Some(idx) {
        fat32::unmount();
    }
    print!("Formatting partition {}", part_no + 1);
    fat32::format(&dst, part.start, part.sectors)?;
    let mut dfs = Fat32::mount(dst, part.start)?;

    // 3) copy the live volume (apps, package database) and mark the install
    let mut copied = 0;
    if let Some(ld) = live_disk {
        if ld != idx {
            copied = fat32::with(|live| copy_tree(live, &mut dfs, "/"))?;
        }
    }
    if !dfs.is_dir("/LOS") {
        dfs.mkdir("/LOS")?;
    }
    dfs.write_file("/LOS/INSTALL.TXT", format!("LightOS {} installed\n", crate::VERSION).as_bytes())?;
    println!("Copied {} file(s) from the live volume.", copied);

    println!();
    println!("Installation complete.");
    println!("Shut down ('shutdown'), then boot from disk {} instead of the live medium", idx);
    println!("(VirtualBox: run scripts\\vbox-finish.ps1).");
    if fat32::current_disk().is_none() {
        let _ = fat32::mount_part(idx, part_no + 1);
    }
    Ok(())
}
