//! Interactive kernel shell (los-shell): line editing with history (Up/Down) and Tab completion,
//! current directory (`cd`, `pwd`), environment variables (`set`, `$NAME`), scripts (`source`,
//! /LOS/STARTUP.LOS) and all built-in commands.
use crate::{
    allocator, desktop, dhcp, dns, e1000, edition, editor, elf, fat32, http, installer, keyboard, los, memory, net, pci,
    process, syscall, ui,
    vga_buffer::{self, Color},
    VERSION,
};
use alloc::{collections::BTreeMap, format, string::{String, ToString}, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::VirtAddr;

static CWD: Mutex<String> = Mutex::new(String::new());
static ENV: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());
static EXIT: AtomicBool = AtomicBool::new(false);
static EMBEDDED: AtomicBool = AtomicBool::new(false);
static DEPTH: AtomicUsize = AtomicUsize::new(0);

const COMMANDS: &[&str] = &[
    "help", "clear", "cls", "echo", "ver", "uptime", "ps", "spawn", "run", "kill", "reap", "mem", "vtop", "syscalls", "los",
    "ls", "dir", "cat", "write", "mkdir", "rm", "del", "df", "exec", "pci", "keytest", "ifconfig", "ping", "arp", "wget",
    "dhcp", "nslookup", "edit", "disks", "part", "install", "mount", "unmount", "cd", "pwd", "set", "unset", "env",
    "history", "source", "exit", "edition", "desktop", "reboot", "shutdown", "bp", "panic",
];

// ------------------------------------------------------------------ paths, variables, history

fn cwd() -> String {
    let c = CWD.lock();
    if c.is_empty() { String::from("/") } else { c.clone() }
}

pub fn current_dir() -> String {
    cwd()
}

pub fn set_current_dir(p: &str) {
    *CWD.lock() = normalize(p);
}

fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            o => parts.push(o),
        }
    }
    if parts.is_empty() { String::from("/") } else { format!("/{}", parts.join("/")) }
}

fn resolve(p: &str) -> String {
    if p.starts_with('/') { normalize(p) } else { normalize(&format!("{}/{}", cwd(), p)) }
}

fn var(name: &str) -> String {
    match name {
        "CWD" => cwd(),
        "VERSION" => String::from(VERSION),
        "EDITION" => String::from(edition::name(edition::current())),
        "IP" => net::fmt_ip(net::my_ip()),
        _ => ENV.lock().get(name).cloned().unwrap_or_default(),
    }
}

/// Replace $NAME and ${NAME} in a command line.
fn expand(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let (name, next): (String, usize) = if chars[i + 1] == '{' {
                match chars[i + 2..].iter().position(|&c| c == '}') {
                    Some(e) => (chars[i + 2..i + 2 + e].iter().collect(), i + 3 + e),
                    None => {
                        out.push('$');
                        i += 1;
                        continue;
                    }
                }
            } else {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j == i + 1 {
                    out.push('$');
                    i += 1;
                    continue;
                }
                (chars[i + 1..j].iter().collect(), j)
            };
            out.push_str(&var(&name));
            i = next;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn hist_push(l: &str) {
    let mut h = HISTORY.lock();
    if h.last().map_or(false, |x| x == l) {
        return;
    }
    if h.len() >= 50 {
        h.remove(0);
    }
    h.push(l.to_string());
}

// ------------------------------------------------------------------ main loops

fn banner() {
    println!();
    println!("╔════════════════════════════════════════════╗");
    println!("║     LightOS Ready - Welcome to Shell      ║");
    println!("╚════════════════════════════════════════════╝");
    println!("Type 'help' for commands. Up/Down = history, Tab = complete, Ctrl+C/Esc = cancel.");
}

fn print_prompt() {
    vga_buffer::set_color(Color::LightCyan, Color::Black);
    let c = cwd();
    if c == "/" {
        print!("los> ");
    } else {
        print!("los:{}> ", c);
    }
    vga_buffer::set_color(Color::LightGray, Color::Black);
}

fn prompt_and_run() {
    print_prompt();
    let mut line = String::new();
    read_line_ed(&mut line);
    run_line(&line);
}

pub fn run() -> ! {
    banner();
    startup();
    if edition::current() == edition::Edition::Desktop && los::is_installed("los-desktop") {
        desktop::run();
        vga_buffer::clear();
        println!("Back in the console. Type 'desktop' to return to the desktop.");
    }
    loop {
        prompt_and_run();
    }
}

/// The terminal app of the desktop: runs until `exit`.
pub fn run_embedded() {
    EXIT.store(false, Ordering::SeqCst);
    EMBEDDED.store(true, Ordering::SeqCst);
    println!("LightOS terminal - type 'exit' to return to the desktop.");
    while !EXIT.load(Ordering::SeqCst) {
        prompt_and_run();
    }
    EMBEDDED.store(false, Ordering::SeqCst);
}

fn run_line(line: &str) {
    let l = line.trim();
    if l.is_empty() {
        return;
    }
    hist_push(l);
    keyboard::clear_cancel();
    keyboard::set_busy(true);
    exec_line(l);
    keyboard::set_busy(false);
}

/// Expand variables, resolve relative paths against the current directory, run the command.
fn exec_line(line: &str) {
    let expanded = expand(line);
    let mut w: Vec<String> = expanded.split_whitespace().map(String::from).collect();
    if w.is_empty() {
        return;
    }
    match w[0].as_str() {
        "ls" | "dir" => {
            if w.len() < 2 {
                w.push(cwd());
            } else {
                w[1] = resolve(&w[1]);
            }
        }
        "cat" | "write" | "mkdir" | "rm" | "del" | "exec" | "edit" | "nano" | "source" | "sh" | "." | "cd" => {
            if w.len() > 1 {
                w[1] = resolve(&w[1]);
            }
        }
        "wget" | "curl" => {
            if w.len() > 2 {
                w[2] = resolve(&w[2]);
            }
        }
        _ => {}
    }
    let args: Vec<&str> = w.iter().map(|s| s.as_str()).collect();
    execute(&args);
}

fn run_script(path: &str) -> Result<(), String> {
    if DEPTH.load(Ordering::SeqCst) >= 4 {
        return Err(String::from("scripts nested too deeply"));
    }
    let data = fat32::with(|fs| fs.read_file(path))?;
    let text = String::from_utf8_lossy(&data).to_string();
    DEPTH.fetch_add(1, Ordering::SeqCst);
    for line in text.lines() {
        if keyboard::cancel_requested() {
            println!("^C");
            break;
        }
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l == "exit" {
            break;
        }
        exec_line(l);
    }
    DEPTH.fetch_sub(1, Ordering::SeqCst);
    Ok(())
}

/// /LOS/STARTUP.LOS runs at every boot, if it exists.
fn startup() {
    if fat32::is_mounted() && fat32::with(|fs| fs.read_file("/LOS/STARTUP.LOS")).is_ok() {
        keyboard::set_busy(true);
        let _ = run_script("/LOS/STARTUP.LOS");
        keyboard::set_busy(false);
    }
}

// ------------------------------------------------------------------ input

/// Ask a question and return the typed answer.
pub fn ask(prompt: &str) -> String {
    print!("{} > ", prompt);
    let mut l = String::new();
    read_line(&mut l);
    l
}

/// Ask for a typed confirmation; only the exact word YES counts.
pub fn confirm(prompt: &str) -> bool {
    ask(prompt).trim() == "YES"
}

/// Plain line input (no history); used for prompts.
fn read_line(buf: &mut String) {
    loop {
        match keyboard::read_char() {
            '\n' | '\r' => {
                println!();
                return;
            }
            '\u{3}' => {
                println!("^C");
                buf.clear();
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

fn replace_line(buf: &mut String, new: &str) {
    for _ in 0..buf.chars().count() {
        print!("\u{8}");
    }
    buf.clear();
    buf.push_str(new);
    print!("{}", new);
}

/// Command-line input with history (Up/Down) and Tab completion.
fn read_line_ed(buf: &mut String) {
    let hist: Vec<String> = HISTORY.lock().clone();
    let mut idx = hist.len();
    let mut saved = String::new();
    loop {
        let c = keyboard::read_char();
        let code = c as u32;
        if c == '\n' || c == '\r' {
            println!();
            return;
        } else if c == '\u{3}' {
            println!("^C");
            buf.clear();
            return;
        } else if c == '\u{8}' || c == '\u{7f}' {
            if buf.pop().is_some() {
                print!("\u{8}");
            }
        } else if c == '\t' {
            tab_complete(buf);
        } else if code == keyboard::KEY_UP as u32 {
            if idx > 0 {
                if idx == hist.len() {
                    saved = buf.clone();
                }
                idx -= 1;
                replace_line(buf, &hist[idx]);
            }
        } else if code == keyboard::KEY_DOWN as u32 {
            if idx < hist.len() {
                idx += 1;
                let t = if idx == hist.len() { saved.clone() } else { hist[idx].clone() };
                replace_line(buf, &t);
            }
        } else if !c.is_control() && buf.len() < 200 {
            buf.push(c);
            print!("{}", c);
        }
    }
}

fn path_candidates(word: &str) -> Vec<String> {
    let (dirpart, prefix) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    };
    let dir = resolve(if dirpart.is_empty() { "." } else { dirpart });
    let prefix_lc = prefix.to_ascii_lowercase();
    match fat32::with(|fs| fs.list(&dir)) {
        Ok(list) => list
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().starts_with(&prefix_lc))
            .map(|e| format!("{}{}{}", dirpart, e.name, if e.is_dir { "/" } else { "" }))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn tab_complete(buf: &mut String) {
    let word_start = buf.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let word = buf[word_start..].to_string();
    let cands: Vec<String> = if word_start == 0 {
        COMMANDS.iter().filter(|c| c.starts_with(word.as_str())).map(|c| c.to_string()).collect()
    } else {
        path_candidates(&word)
    };
    if cands.is_empty() {
        return;
    }
    let mut common = cands[0].clone();
    for c in &cands[1..] {
        let n = common.chars().zip(c.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
        common = common.chars().take(n).collect();
    }
    let apply = |buf: &mut String, new: &str| {
        for _ in 0..word.chars().count() {
            print!("\u{8}");
        }
        buf.truncate(word_start);
        buf.push_str(new);
        print!("{}", new);
    };
    if cands.len() == 1 {
        let mut full = cands[0].clone();
        if !full.ends_with('/') {
            full.push(' ');
        }
        apply(buf, &full);
    } else if common.chars().count() > word.chars().count() {
        apply(buf, &common);
    } else {
        println!();
        println!("{}", cands.join("  "));
        print_prompt();
        print!("{}", buf);
    }
}

// ------------------------------------------------------------------ power

pub fn do_reboot() -> ! {
    unsafe { Port::<u8>::new(0x64).write(0xFE) };
    crate::hlt_loop()
}

pub fn do_shutdown() -> ! {
    unsafe {
        Port::<u16>::new(0x604).write(0x2000); // QEMU (ACPI)
        Port::<u16>::new(0xB004).write(0x2000); // older QEMU/Bochs
        Port::<u16>::new(0x4004).write(0x3400); // VirtualBox
    }
    println!("Powering off is only supported under QEMU/VirtualBox; it is now safe to halt.");
    crate::hlt_loop()
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
        "keytest" => keytest(),
        "ifconfig" | "ip" => ifconfig_cmd(args),
        "ping" => ping_cmd(args),
        "arp" => arp_cmd(),
        "wget" | "curl" => wget_cmd(args),
        "dhcp" => match { e1000::wait_link(4000); dhcp::run(3, 200) } {
            Ok(m) => println!("{}", m),
            Err(e) => println!("dhcp: {}", e),
        },
        "nslookup" | "host" => match args.get(1) {
            Some(h) => match dns::resolve(h) {
                Ok(ip) => println!("{} has address {}", h, net::fmt_ip(ip)),
                Err(e) => println!("nslookup: {}", e),
            },
            None => println!("usage: nslookup <host>"),
        },
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
        "cd" => {
            let target = args.get(1).copied().unwrap_or("/");
            let ok = fat32::with(|fs| if fs.is_dir(target) { Ok(()) } else { Err(format!("'{}' is not a directory", target)) });
            if report(ok).is_some() {
                *CWD.lock() = normalize(target);
            }
        }
        "pwd" => println!("{}", cwd()),
        "set" => set_cmd(args),
        "unset" => {
            for n in &args[1..] {
                ENV.lock().remove(*n);
            }
        }
        "env" => {
            for (k, v) in ENV.lock().iter() {
                println!("{}={}", k, v);
            }
        }
        "history" => {
            for (i, h) in HISTORY.lock().iter().enumerate() {
                println!("{:>3}  {}", i + 1, h);
            }
        }
        "source" | "sh" | "." => match args.get(1) {
            Some(p) => {
                if let Err(e) = run_script(p) {
                    println!("script: {}", e);
                }
            }
            None => println!("usage: source <script-file>   (lines are commands, # starts a comment)"),
        },
        "exit" => {
            if EMBEDDED.load(Ordering::SeqCst) {
                EXIT.store(true, Ordering::SeqCst);
            } else {
                println!("this is the main console; use 'shutdown' or 'reboot'");
            }
        }
        "edition" => edition_cmd(args),
        "desktop" => {
            if !los::is_installed("los-desktop") {
                println!("Console edition: the desktop is the los-desktop package.  Run:  edition desktop");
            } else {
                desktop::run();
                vga_buffer::clear();
            }
        }
        "bp" => x86_64::instructions::interrupts::int3(),
        "panic" => panic!("panic requested from shell"),
        "reboot" => do_reboot(),
        "shutdown" | "poweroff" => do_shutdown(),
        other => println!("unknown command: {} (try 'help')", other),
    }
}

fn help() {
    println!("Shell:    cd pwd ls cat write mkdir rm df | set NAME=val  unset  env  $NAME | history | source <script>");
    println!("          Up/Down history, Tab completes commands and paths, Ctrl+C / Esc cancels");
    println!("System:   help clear echo ver uptime mem vtop <hex> reboot shutdown exit");
    println!("Edition:  edition [console|desktop]   desktop   (text-mode desktop, package los-desktop)");
    println!("Editor:   edit <file>   (needs: los install los-edit; F2 save, Esc quit)");
    println!("Programs: exec <elf>   los run <pkg>   spawn <prog> run kill <pid> reap");
    println!("Web:      wget <http://host/path> [file]   (HTTP only)");
    println!("Network:  pci  ifconfig [ip mask gw]  dhcp  nslookup <host>  ping <host|ip> [n]  arp");
    println!("Disks:    disks  part list|new|delete|wipe|mkfs  mount [disk [part]]  unmount  install <disk>");
    println!("Packages: los install|remove|list|search|update|available|info|verify|autoremove|run|server");
    println!("Debug:    keytest  syscalls  bp  panic");
}

fn set_cmd(args: &[&str]) {
    if args.len() < 2 {
        for (k, v) in ENV.lock().iter() {
            println!("{}={}", k, v);
        }
        return;
    }
    let (name, value) = match args[1].split_once('=') {
        Some((n, v)) => {
            let mut val = String::from(v);
            for extra in &args[2..] {
                val.push(' ');
                val.push_str(extra);
            }
            (n.to_string(), val)
        }
        None => (args[1].to_string(), args[2..].join(" ")),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        println!("usage: set NAME=value   (names: letters, digits, _)");
        return;
    }
    ENV.lock().insert(name, value);
}

fn edition_cmd(args: &[&str]) {
    match args.get(1).copied() {
        None => println!("edition: {}", edition::name(edition::current())),
        Some("console") => {
            if report(edition::set(edition::Edition::Console)).is_some() {
                println!("Console edition set; the desktop no longer starts at boot.");
            }
        }
        Some("desktop") => {
            if !los::is_installed("los-desktop") {
                if let Err(e) = los::install("los-desktop") {
                    println!("error: {}", e);
                    return;
                }
            }
            if report(edition::set(edition::Edition::Desktop)).is_some() {
                println!("Desktop edition set. It starts at the next boot; type 'desktop' to open it now.");
            }
        }
        Some(_) => println!("usage: edition [console|desktop]"),
    }
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
            println!("      dns {}", net::fmt_ip(net::dns()));
        }
        _ => println!("no network card (VirtualBox: Adapter type 'Intel PRO/1000 MT Desktop', attached to NAT)"),
    }
}

fn ping_cmd(args: &[&str]) {
    let Some(host) = args.get(1) else {
        println!("usage: ping <host|a.b.c.d> [count]   e.g. ping 10.0.2.2   ping example.com");
        return;
    };
    let ip = match dns::resolve(host) {
        Ok(ip) => ip,
        Err(e) => {
            println!("ping: {}", e);
            return;
        }
    };
    if net::parse_ip(host).is_none() {
        println!("{} is {}", host, net::fmt_ip(ip));
    }
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

fn wget_cmd(args: &[&str]) {
    let Some(url) = args.get(1) else {
        println!("usage: wget <http://host[:port]/path> [save-as-file]");
        return;
    };
    match http::get_url(url) {
        Ok(r) => {
            println!("HTTP {}  ({} bytes)", r.status, r.body.len());
            if let Some(file) = args.get(2) {
                if report(fat32::with(|fs| fs.write_file(file, &r.body))).is_some() {
                    println!("saved to {}", file);
                }
            } else {
                let shown = &r.body[..r.body.len().min(1500)];
                println!("{}", String::from_utf8_lossy(shown));
                if r.body.len() > shown.len() {
                    println!("... ({} more bytes; use: wget <url> <file>)", r.body.len() - shown.len());
                }
            }
        }
        Err(e) => println!("wget: {}", e),
    }
}

fn keytest() {
    println!("Press keys (try Left Ctrl + C). Esc quits.");
    loop {
        match keyboard::trace_pop() {
            Some((sc, code)) => {
                let what = match code {
                    0 => String::from("(no character)"),
                    0xFF => String::from("other key"),
                    3 => String::from("Ctrl+C  <- cancel key works"),
                    0x1B => String::from("Esc"),
                    c if (0x20..0x7F).contains(&c) => format!("'{}'", c as char),
                    c => format!("code {:#04x}", c),
                };
                println!("scancode {:#04x} -> {}", sc, what);
                if code == 0x1B {
                    break;
                }
            }
            None => x86_64::instructions::interrupts::enable_and_hlt(),
        }
    }
    keyboard::clear_cancel();
}
