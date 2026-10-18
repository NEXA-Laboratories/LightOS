//! Interactive kernel shell (los-shell).
use crate::{allocator, e1000, editor, elf, fat32, installer, net, pci, keyboard, los, memory, process, syscall, ui, vga_buffer::{self, Color}, VERSION};
use alloc::{format, string::String, vec::Vec};
use x86_64::instructions::port::Port;
use x86_64::VirtAddr;

const PROMPT: &str = "los> ";

pub fn run() -> ! {
    println!();
    println!("╔════════════════════════════════════════════╗");
    println!("║     LightOS Ready - Welcome to Shell      ║");
    println!("╚════════════════════════════════════════════╝");
    println!("Type 'help' for commands.");
    let mut line = String::new();
    loop {
        vga_buffer::set_color(Color::LightCyan, Color::Black);
        print!("{}", PROMPT);
        vga_buffer::set_color(Color::LightGray, Color::Black);
        line.clear();
        read_line(&mut line);
        let args: Vec<&str> = line.split_whitespace().collect();
        if !args.is_empty() {
            execute(&args);
        }
    }
}

/// Ask for a typed confirmation; only the exact word YES counts.
pub fn confirm(prompt: &str) -> bool {
    print!("{} > ", prompt);
    let mut l = String::new();
    read_line(&mut l);
    l.trim() == "YES"
}

fn read_line(buf: &mut String) {
    loop {
        match keyboard::read_char() {
            '\n' | '\r' => {
                println!();
                return;
            }
            '\u{8}' | '\u{7f}' => {
                if buf.pop().is_some() {
                    print!("\u{8}");
                }
            }
            c if !c.is_control() => {
                if buf.len() < 200 {
                    buf.push(c);
                    print!("{}", c);
                }
            }
            _ => {}
        }
    }
}

fn execute(args: &[&str]) {
    match args[0] {
        "help" | "?" => help(),
        "clear" | "cls" => vga_buffer::clear(),
        "echo" => println!("{}", args[1..].join(" ")),
        "ver" | "uname" | "version" => println!("LightOS {} x86_64 (NEXA Laboratories)", VERSION),
        "uptime" => {
            let t = process::ticks();
            println!("up {}.{:02} s ({} ticks @ {} Hz)", t / process::TICK_HZ, t % process::TICK_HZ, t, process::TICK_HZ);
        }
        "ps" => ps(),
        "spawn" => spawn(args),
        "run" => {
            let n = process::run_all();
            println!("{} process(es) ran", n);
        }
        "kill" => kill(args),
        "reap" => println!("reaped {} terminated process(es)", process::reap()),
        "mem" => mem(),
        "vtop" => vtop(args),
        "syscalls" | "syscall-test" => syscall_test(),
        "los" => los::command(&args[1..]),
        "ls" | "dir" => fs_ls(args),
        "cat" => fs_cat(args),
        "write" => fs_write(args),
        "mkdir" => fs_mkdir(args),
        "rm" | "del" => fs_rm(args),
        "df" => fs_df(),
        "exec" => exec_cmd(args),
        "pci" => pci_cmd(),
        "ifconfig" | "ip" => ifconfig_cmd(args),
        "ping" => ping_cmd(args),
        "arp" => arp_cmd(),
        "edit" | "nano" => {
            if !los::is_installed("los-edit") {
                println!("the editor is part of the los-edit package:  los install los-edit");
            } else if let Some(path) = args.get(1) {
                if let Err(e) = editor::edit(path) {
                    println!("edit: {}", e);
                }
            } else {
                println!("usage: edit <file>");
            }
        }
        "disks" => installer::disks_cmd(),
        "part" => installer::part_cmd(&args[1..]),
        "install" => installer::install_cmd(&args[1..]),
        "mount" => mount_cmd(args),
        "umount" | "unmount" => {
            fat32::unmount();
            println!("unmounted");
        }
        "bp" => x86_64::instructions::interrupts::int3(),
        "panic" => panic!("panic requested from shell"),
        "reboot" => {
            unsafe { Port::<u8>::new(0x64).write(0xFE) };
            crate::hlt_loop()
        }
        "shutdown" | "poweroff" => {
            unsafe {
                Port::<u16>::new(0x604).write(0x2000); // QEMU (ACPI)
                Port::<u16>::new(0xB004).write(0x2000); // older QEMU/Bochs
                Port::<u16>::new(0x4004).write(0x3400); // VirtualBox
            }
            println!("Powering off is only supported under QEMU; it is now safe to halt.");
            crate::hlt_loop()
        }
        other => println!("unknown command: {} (try 'help')", other),
    }
}

fn help() {
    println!("System:   help clear echo ver uptime mem vtop <hex> reboot shutdown");
    println!("Files:    ls [dir] cat <f> write <f> <text> mkdir <d> rm <f> df exec <elf>");
    println!("Editor:   edit <file>   (needs: los install los-edit; F2 save, Esc quit)");
    println!("Network:  pci  ifconfig [ip mask gw]  ping <ip> [count]  arp");
    println!("Disks:    disks  part list|new|delete|wipe|mkfs  mount [disk [part]]  unmount");
    println!("Install:  install <disk>   (copies LightOS to another disk, see docs/VIRTUALBOX.md)");
    println!("Process:  ps spawn <prog> run kill <pid> reap   (progs: hello counter sysdemo)");
    println!("Kernel:   syscalls (int 0x80 self-test)  bp (breakpoint)  panic");
    println!("Packages: los install|remove|list|search|update|available|info|verify|autoremove");
    println!("          run  (los run <pkg>)   los help   for details");
}

fn ps() {
    let rows: Vec<String> = process::snapshot()
        .iter()
        .map(|p| format!("{:>4}  {:<18} {:<11} {:>4}  {:>9}", p.pid, p.name, p.state.as_str(), p.priority, p.ticks))
        .collect();
    ui::table("Process List", Some(" PID  Name               State       Prio      Ticks"), &rows);
    println!("context switches: {}", process::switches());
}

fn spawn(args: &[&str]) {
    let Some(name) = args.get(1) else {
        println!("usage: spawn <program>   (hello, counter, sysdemo)");
        return;
    };
    match process::PROGRAMS.iter().find(|(n, _)| n == name) {
        Some((n, f)) => match process::spawn(n, 5, *f) {
            Ok(pid) => println!("spawned {} as PID {} (use 'run' to schedule)", n, pid),
            Err(e) => println!("spawn failed: {}", e),
        },
        None => println!("no such program: {}", name),
    }
}

fn kill(args: &[&str]) {
    match args.get(1).and_then(|s| s.parse::<u32>().ok()) {
        Some(pid) => match process::kill(pid) {
            Ok(()) => println!("killed {}", pid),
            Err(e) => println!("kill: {}", e),
        },
        None => println!("usage: kill <pid>"),
    }
}

fn mem() {
    let mib = |b: u64| b / (1024 * 1024);
    println!("Physical memory (usable): {} MiB", mib(memory::usable_bytes()));
    println!("Frames allocated:         {} ({} KiB)", memory::frames_used(), memory::frames_used() * 4);
    println!("Physical map offset:      {:#x}", memory::phys_offset());
    println!(
        "Heap: {:#x} .. {:#x}  used {} KiB / free {} KiB",
        allocator::HEAP_START,
        allocator::HEAP_START + allocator::HEAP_SIZE,
        allocator::used() / 1024,
        allocator::free() / 1024
    );
}

fn vtop(args: &[&str]) {
    let Some(a) = args.get(1) else {
        println!("usage: vtop <hex virtual address>");
        return;
    };
    match u64::from_str_radix(a.trim_start_matches("0x"), 16) {
        Ok(v) => match VirtAddr::try_new(v) {
            Ok(va) => match memory::translate(va) {
                Some(pa) => println!("{:#x} -> {:#x}", v, pa.as_u64()),
                None => println!("{:#x} is not mapped", v),
            },
            Err(_) => println!("not a canonical address"),
        },
        Err(_) => println!("bad hex address"),
    }
}

fn syscall_test() {
    use syscall::*;
    println!("Issuing system calls through int 0x80:");
    let n = write(1, "  SYS_WRITE ok\n");
    println!("  SYS_WRITE returned {}", n);
    println!("  SYS_GETPID = {}", getpid());
    println!("  SYS_UPTIME = {} ticks", uptime());
    println!("  SYS_LOS_LIST -> {} package(s) installed", invoke(SYS_LOS_LIST, 0, 0, 0));
    println!("  bad syscall -> {:#x} (expect u64::MAX)", invoke(999, 0, 0, 0));
}

// ------------------------------------------------------------------ files (FAT32)

fn report<T>(r: Result<T, String>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            vga_buffer::set_color(Color::LightRed, Color::Black);
            print!("error: ");
            vga_buffer::set_color(Color::LightGray, Color::Black);
            println!("{}", e);
            None
        }
    }
}

fn fs_ls(args: &[&str]) {
    let path = args.get(1).copied().unwrap_or("/");
    if let Some(list) = report(fat32::with(|fs| fs.list(path))) {
        let rows: Vec<String> = list
            .iter()
            .map(|e| if e.is_dir { format!("{:<14} <DIR>", e.name) } else { format!("{:<14} {:>9} B", e.name, e.size) })
            .collect();
        if rows.is_empty() {
            println!("(empty)");
        } else {
            ui::table(path, None, &rows);
        }
    }
}

fn fs_cat(args: &[&str]) {
    let Some(path) = args.get(1) else {
        println!("usage: cat <file>");
        return;
    };
    if let Some(data) = report(fat32::with(|fs| fs.read_file(path))) {
        println!("{}", String::from_utf8_lossy(&data));
    }
}

fn fs_write(args: &[&str]) {
    if args.len() < 3 {
        println!("usage: write <file> <text...>");
        return;
    }
    let mut text = args[2..].join(" ");
    text.push('\n');
    if report(fat32::with(|fs| fs.write_file(args[1], text.as_bytes()))).is_some() {
        println!("wrote {} bytes to {}", text.len(), args[1]);
    }
}

fn fs_mkdir(args: &[&str]) {
    match args.get(1) {
        Some(p) => {
            report(fat32::with(|fs| fs.mkdir(p)));
        }
        None => println!("usage: mkdir <dir>"),
    }
}

fn fs_rm(args: &[&str]) {
    match args.get(1) {
        Some(p) => {
            report(fat32::with(|fs| fs.remove(p)));
        }
        None => println!("usage: rm <file-or-empty-dir>"),
    }
}

fn fs_df() {
    if let Some((total, free, cb)) = report(fat32::with(|fs| fs.usage())) {
        let kib = |c: u32| c as u64 * cb as u64 / 1024;
        println!("Total {} KiB, used {} KiB, free {} KiB ({} B clusters)", kib(total), kib(total - free), kib(free), cb);
    }
}

fn exec_cmd(args: &[&str]) {
    let Some(path) = args.get(1) else {
        println!("usage: exec <path-to-elf>   e.g. exec /BIN/HELLO.ELF");
        return;
    };
    let Some(data) = report(fat32::with(|fs| fs.read_file(path))) else { return };
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(code) = report(elf::exec(name, &data)) {
        println!("{} exited with code {}", name, code);
    }
}

fn mount_cmd(args: &[&str]) {
    let r = match (args.get(1), args.get(2)) {
        (None, _) => fat32::mount_auto(),
        (Some(d), p) => match (d.parse::<usize>(), p.map(|x| x.parse::<usize>()).unwrap_or(Ok(0))) {
            (Ok(d), Ok(p)) => fat32::mount_part(d, p),
            _ => Err(String::from("usage: mount [disk [partition]]   (partition 0 = raw disk)")),
        },
    };
    if let Some(m) = report(r) {
        println!("{}", m);
    }
}

// ------------------------------------------------------------------ network

fn pci_cmd() {
    let rows: Vec<String> = pci::scan()
        .iter()
        .map(|d| {
            format!(
                "{:02x}:{:02x}.{}  {:04x}:{:04x}  {}",
                d.bus, d.dev, d.func, d.vendor, d.device, pci::class_name(d.class, d.subclass)
            )
        })
        .collect();
    ui::table("PCI devices", Some("Bus:Dev.F  Vendor:Dev  Class"), &rows);
}

fn ifconfig_cmd(args: &[&str]) {
    if args.len() >= 2 {
        match (net::parse_ip(args[1]), args.get(2).and_then(|s| net::parse_ip(s)), args.get(3).and_then(|s| net::parse_ip(s))) {
            (Some(ip), mask, gw) => {
                let ((_, m0, g0), _) = net::config();
                net::set_config(ip, mask.unwrap_or(m0), gw.unwrap_or(g0));
            }
            _ => {
                println!("usage: ifconfig <ip> [mask] [gateway]   e.g. ifconfig 10.0.2.15 255.255.255.0 10.0.2.2");
                return;
            }
        }
    }
    let ((ip, mask, gw), mac) = net::config();
    match (mac, e1000::with(|n| (n.link_up(), n.pci_id))) {
        (Some(m), Some((link, id))) => {
            println!("eth0: Intel e1000 {:04x}:{:04x}  link {}", id.0, id.1, if link { "up" } else { "DOWN" });
            println!("      MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]);
            println!("      inet {}  mask {}  gateway {}", net::fmt_ip(ip), net::fmt_ip(mask), net::fmt_ip(gw));
        }
        _ => println!("no network card (VirtualBox: Adapter type 'Intel PRO/1000 MT Desktop', attached to NAT)"),
    }
}

fn ping_cmd(args: &[&str]) {
    let Some(ip) = args.get(1).and_then(|s| net::parse_ip(s)) else {
        println!("usage: ping <a.b.c.d> [count]   e.g. ping 10.0.2.2");
        return;
    };
    let count = args.get(2).and_then(|s| s.parse::<u32>().ok()).unwrap_or(4).clamp(1, 20);
    net::ping(ip, count);
}

fn arp_cmd() {
    let t = net::arp_table();
    if t.is_empty() {
        println!("ARP cache is empty");
    }
    for (ip, m) in t {
        println!("{:<15} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", net::fmt_ip(ip), m[0], m[1], m[2], m[3], m[4], m[5]);
    }
}
