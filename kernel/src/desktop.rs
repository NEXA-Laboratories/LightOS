//! LightOS Desktop (text mode): menu bar, application tiles, file manager, package manager,
//! system information, power menu and an embedded terminal. Esc leaves to the console shell.
use crate::keyboard::{self, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};
use crate::vga_buffer::{self, put_at, Color, SCREEN_H, SCREEN_W};
use crate::{allocator, ata, e1000, edition, editor, elf, fat32, los, memory, net, process, shell, VERSION};
use alloc::{format, string::{String, ToString}, vec::Vec};

const DESK: Color = Color::Cyan;
const ITEMS: [(&str, &str); 5] = [("Files", "[ F ]"), ("Terminal", " >_  "), ("Packages", "[ P ]"), ("System", "[ S ]"), ("Power", "( o )")];

// ------------------------------------------------------------------ drawing helpers

fn fill(row: usize, col: usize, h: usize, w: usize, bg: Color) {
    for r in row..(row + h).min(SCREEN_H) {
        for c in col..(col + w).min(SCREEN_W) {
            put_at(r, c, ' ', Color::White, bg);
        }
    }
}

fn text(row: usize, col: usize, s: &str, fg: Color, bg: Color) {
    for (i, ch) in s.chars().enumerate() {
        if col + i >= SCREEN_W {
            break;
        }
        put_at(row, col + i, ch, fg, bg);
    }
}

fn frame(row: usize, col: usize, h: usize, w: usize, title: &str, fg: Color, bg: Color) {
    fill(row, col, h, w, bg);
    put_at(row, col, '\u{2554}', fg, bg);
    put_at(row, col + w - 1, '\u{2557}', fg, bg);
    put_at(row + h - 1, col, '\u{255A}', fg, bg);
    put_at(row + h - 1, col + w - 1, '\u{255D}', fg, bg);
    for c in 1..w - 1 {
        put_at(row, col + c, '\u{2550}', fg, bg);
        put_at(row + h - 1, col + c, '\u{2550}', fg, bg);
    }
    for r in 1..h - 1 {
        put_at(row + r, col, '\u{2551}', fg, bg);
        put_at(row + r, col + w - 1, '\u{2551}', fg, bg);
    }
    if !title.is_empty() {
        let t = format!(" {} ", title);
        text(row, col + 2, &t, Color::Yellow, bg);
    }
}

fn hint_bar(s: &str) {
    fill(SCREEN_H - 1, 0, 1, SCREEN_W, Color::LightGray);
    text(SCREEN_H - 1, 1, s, Color::Black, Color::LightGray);
}

fn uptime_str() -> String {
    let s = process::ticks() / process::TICK_HZ;
    format!("up {:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn draw_clock() {
    let t = uptime_str();
    text(0, SCREEN_W - t.len() - 1, &t, Color::Black, Color::LightGray);
}

/// Wait for a key; returns None after `refresh` ticks so callers can redraw (clock etc).
fn wait_key(refresh: u64) -> Option<u8> {
    let start = process::ticks();
    loop {
        if let Some(c) = keyboard::poll_char() {
            return Some(c as u32 as u8);
        }
        if process::ticks() - start >= refresh {
            return None;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

fn key() -> u8 {
    keyboard::read_char() as u32 as u8
}

fn press_any_key() {
    print!("\nPress any key to return to the desktop...");
    key();
}

/// One-line text input at the bottom of the screen. Esc cancels.
fn input(label: &str) -> Option<String> {
    let mut s = String::new();
    loop {
        fill(SCREEN_H - 2, 0, 1, SCREEN_W, Color::Black);
        text(SCREEN_H - 2, 1, &format!("{}: {}_", label, s), Color::White, Color::Black);
        match key() {
            b'\n' => return if s.trim().is_empty() { None } else { Some(s.trim().to_string()) },
            0x1B => return None,
            0x08 | 0x7F => {
                s.pop();
            }
            k if (0x20..0x7F).contains(&k) && s.len() < 60 => s.push(k as char),
            _ => {}
        }
    }
}

fn ask_yes(msg: &str) -> bool {
    fill(SCREEN_H - 2, 0, 1, SCREEN_W, Color::Red);
    text(SCREEN_H - 2, 1, &format!("{} (y/n)", msg), Color::White, Color::Red);
    matches!(key(), b'y' | b'Y')
}

// ------------------------------------------------------------------ home screen

fn draw_home(sel: usize) {
    fill(1, 0, SCREEN_H - 2, SCREEN_W, DESK);
    // menu bar
    fill(0, 0, 1, SCREEN_W, Color::LightGray);
    text(0, 1, "LightOS", Color::Black, Color::LightGray);
    let mut col = 10;
    for (i, (name, _)) in ITEMS.iter().enumerate() {
        let label = format!(" {} ", name);
        if i == sel {
            text(0, col, &label, Color::White, Color::Blue);
        } else {
            text(0, col, &label, Color::Black, Color::LightGray);
        }
        col += label.len() + 1;
    }
    draw_clock();

    let title = "Welcome to LightOS Desktop";
    text(3, (SCREEN_W - title.len()) / 2, title, Color::White, DESK);
    let sub = format!("version {}   edition {}   {} package(s) installed", VERSION, edition::name(edition::current()), los::installed_count());
    text(5, (SCREEN_W.saturating_sub(sub.len())) / 2, &sub, Color::Black, DESK);

    for (i, (name, icon)) in ITEMS.iter().enumerate() {
        let col = 1 + i * 16;
        let (fg, bg) = if i == sel { (Color::Black, Color::LightGray) } else { (Color::White, Color::Blue) };
        frame(8, col, 7, 14, "", fg, bg);
        text(10, col + (14 - icon.len()) / 2, icon, fg, bg);
        text(12, col + (14 - name.len()) / 2, name, fg, bg);
    }
    hint_bar("Left/Right select   Enter open   1-5 quick open   Esc console shell");
}

pub fn run() {
    keyboard::flush();
    let mut sel = 0usize;
    draw_home(sel);
    loop {
        match wait_key(50) {
            None => draw_clock(),
            Some(k) => {
                let mut open_it = false;
                match k {
                    KEY_LEFT => sel = (sel + ITEMS.len() - 1) % ITEMS.len(),
                    KEY_RIGHT | b'\t' => sel = (sel + 1) % ITEMS.len(),
                    b'1'..=b'5' => {
                        sel = (k - b'1') as usize;
                        open_it = true;
                    }
                    b'\n' => open_it = true,
                    0x1B => {
                        keyboard::clear_cancel();
                        return;
                    }
                    _ => {}
                }
                if open_it {
                    match sel {
                        0 => files_app(),
                        1 => {
                            vga_buffer::clear();
                            shell::run_embedded();
                        }
                        2 => packages_app(),
                        3 => system_app(),
                        _ => power_app(),
                    }
                    keyboard::clear_cancel();
                }
                draw_home(sel);
            }
        }
    }
}

// ------------------------------------------------------------------ Files

struct Entry {
    name: String,
    is_dir: bool,
    size: u32,
}

fn join(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{}", name) } else { format!("{}/{}", dir, name) }
}

fn parent(dir: &str) -> String {
    match dir.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => dir[..i].to_string(),
    }
}

fn list_dir(path: &str) -> Result<Vec<Entry>, String> {
    let mut v: Vec<Entry> = Vec::new();
    if path != "/" {
        v.push(Entry { name: String::from(".."), is_dir: true, size: 0 });
    }
    for e in fat32::with(|fs| fs.list(path))? {
        v.push(Entry { name: e.name, is_dir: e.is_dir, size: e.size });
    }
    Ok(v)
}

fn files_app() {
    let mut path = shell::current_dir();
    let mut sel = 0usize;
    let mut top = 0usize;
    let mut msg = String::new();
    const ROWS: usize = 18;
    loop {
        let entries = match list_dir(&path) {
            Ok(e) => e,
            Err(e) => {
                msg = e;
                path = String::from("/");
                list_dir("/").unwrap_or_default()
            }
        };
        if sel >= entries.len() {
            sel = entries.len().saturating_sub(1);
        }
        if sel < top {
            top = sel;
        }
        if sel >= top + ROWS {
            top = sel + 1 - ROWS;
        }
        fill(1, 0, SCREEN_H - 2, SCREEN_W, DESK);
        fill(0, 0, 1, SCREEN_W, Color::LightGray);
        text(0, 1, "LightOS > Files", Color::Black, Color::LightGray);
        frame(1, 0, 22, SCREEN_W, &path, Color::White, Color::Blue);
        for (i, e) in entries.iter().enumerate().skip(top).take(ROWS) {
            let row = 3 + (i - top);
            let (fg, bg) = if i == sel { (Color::Black, Color::LightGray) } else { (Color::White, Color::Blue) };
            fill(row, 2, 1, SCREEN_W - 4, bg);
            let size = if e.is_dir { String::from("<DIR>") } else { format!("{} B", e.size) };
            text(row, 3, &format!("{:<34}{}", e.name, size), fg, bg);
        }
        if entries.is_empty() {
            text(3, 3, "(empty)", Color::White, Color::Blue);
        }
        text(21, 2, &msg, Color::Yellow, Color::Blue);
        hint_bar("Enter open  Bksp up  N new file  M new folder  D delete  Esc back");
        msg.clear();

        match key() {
            KEY_UP => sel = sel.saturating_sub(1),
            KEY_DOWN => {
                if sel + 1 < entries.len() {
                    sel += 1;
                }
            }
            0x08 => path = parent(&path),
            b'\n' => {
                if let Some(e) = entries.get(sel) {
                    let p = join(&path, &e.name);
                    if e.name == ".." {
                        path = parent(&path);
                        sel = 0;
                    } else if e.is_dir {
                        path = p;
                        sel = 0;
                    } else {
                        open_file(&p, &mut msg);
                    }
                }
            }
            b'n' | b'N' => {
                if let Some(name) = input("New file name") {
                    open_file(&join(&path, &name), &mut msg);
                }
            }
            b'm' | b'M' => {
                if let Some(name) = input("New folder name") {
                    if let Err(e) = fat32::with(|fs| fs.mkdir(&join(&path, &name))) {
                        msg = e;
                    }
                }
            }
            b'd' | b'D' => {
                if let Some(e) = entries.get(sel) {
                    if e.name != ".." && ask_yes(&format!("Delete {}?", e.name)) {
                        if let Err(er) = fat32::with(|fs| fs.remove(&join(&path, &e.name))) {
                            msg = er;
                        }
                    }
                }
            }
            0x1B => {
                shell::set_current_dir(&path);
                return;
            }
            _ => {}
        }
    }
}

fn open_file(p: &str, msg: &mut String) {
    if p.to_ascii_uppercase().ends_with(".ELF") {
        vga_buffer::clear();
        let name = p.rsplit('/').next().unwrap_or(p);
        match fat32::with(|fs| fs.read_file(p)).and_then(|d| elf::exec(name, &d)) {
            Ok(code) => println!("\n{} exited with code {}", name, code),
            Err(e) => println!("\nerror: {}", e),
        }
        press_any_key();
    } else if los::is_installed("los-edit") {
        if let Err(e) = editor::edit(p) {
            *msg = e;
        }
    } else {
        *msg = String::from("install los-edit to edit files (Packages app)");
    }
}

// ------------------------------------------------------------------ Packages

fn packages_app() {
    let mut sel = 0usize;
    let mut top = 0usize;
    const ROWS: usize = 16;
    loop {
        let cat = los::catalog();
        if sel >= cat.len() {
            sel = cat.len().saturating_sub(1);
        }
        if sel < top {
            top = sel;
        }
        if sel >= top + ROWS {
            top = sel + 1 - ROWS;
        }
        fill(1, 0, SCREEN_H - 2, SCREEN_W, DESK);
        fill(0, 0, 1, SCREEN_W, Color::LightGray);
        text(0, 1, "LightOS > Packages", Color::Black, Color::LightGray);
        frame(1, 0, 22, SCREEN_W, "Package manager", Color::White, Color::Blue);
        text(3, 3, "    Package        Version  Verified  Description", Color::Yellow, Color::Blue);
        for (i, p) in cat.iter().enumerate().skip(top).take(ROWS) {
            let row = 4 + (i - top);
            let (fg, bg) = if i == sel { (Color::Black, Color::LightGray) } else { (Color::White, Color::Blue) };
            fill(row, 2, 1, SCREEN_W - 4, bg);
            let line = format!(
                "{} {:<14} {:<8} {:<8}  {}",
                if p.installed { "[x]" } else { "[ ]" },
                p.name,
                p.version,
                if p.verified { "yes" } else { "NO" },
                p.description
            );
            text(row, 3, &line, fg, bg);
        }
        hint_bar("I install  R remove  U update list  Up/Down select  Esc back");
        match key() {
            KEY_UP => sel = sel.saturating_sub(1),
            KEY_DOWN => {
                if sel + 1 < cat.len() {
                    sel += 1;
                }
            }
            b'i' | b'I' | b'\n' => {
                if let Some(p) = cat.get(sel) {
                    vga_buffer::clear();
                    if let Err(e) = los::install(&p.name) {
                        println!("error: {}", e);
                    }
                    press_any_key();
                }
            }
            b'r' | b'R' => {
                if let Some(p) = cat.get(sel) {
                    vga_buffer::clear();
                    if let Err(e) = los::remove(&p.name, false) {
                        println!("error: {}", e);
                    }
                    press_any_key();
                }
            }
            b'u' | b'U' => {
                vga_buffer::clear();
                los::command(&["update"]);
                press_any_key();
            }
            0x1B => return,
            _ => {}
        }
        keyboard::clear_cancel();
    }
}

// ------------------------------------------------------------------ System

fn system_app() {
    loop {
        fill(1, 0, SCREEN_H - 2, SCREEN_W, DESK);
        fill(0, 0, 1, SCREEN_W, Color::LightGray);
        text(0, 1, "LightOS > System", Color::Black, Color::LightGray);
        draw_clock();
        frame(1, 0, 22, SCREEN_W, "System information", Color::White, Color::Blue);
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("LightOS {}   edition: {}   {}", VERSION, edition::name(edition::current()), uptime_str()));
        lines.push(format!(
            "Memory: {} MiB usable, heap {} KiB used / {} KiB free",
            memory::usable_bytes() / (1024 * 1024),
            allocator::used() / 1024,
            allocator::free() / 1024
        ));
        lines.push(format!("Processes: {}   Packages installed: {}", process::snapshot().len(), los::installed_count()));
        lines.push(String::new());
        lines.push(String::from("Disks:"));
        for d in ata::disks() {
            lines.push(format!("  disk {}  {:<10} {:>6} MiB  {}", d.index, d.position(), d.sectors / 2048, d.model()));
        }
        lines.push(String::new());
        let ((ip, mask, gw), mac) = net::config();
        lines.push(String::from("Network:"));
        match (mac, e1000::with(|n| n.link_up())) {
            (Some(m), Some(up)) => {
                lines.push(format!("  link {}   MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", if up { "up" } else { "DOWN" }, m[0], m[1], m[2], m[3], m[4], m[5]));
                lines.push(format!("  ip {}  mask {}  gateway {}  dns {}", net::fmt_ip(ip), net::fmt_ip(mask), net::fmt_ip(gw), net::fmt_ip(net::dns())));
            }
            _ => lines.push(String::from("  no network card")),
        }
        for (i, l) in lines.iter().enumerate().take(19) {
            text(3 + i, 3, l, Color::White, Color::Blue);
        }
        hint_bar("Esc back (refreshes every second)");
        if let Some(k) = wait_key(100) {
            if k == 0x1B {
                return;
            }
        }
    }
}

// ------------------------------------------------------------------ Power

fn power_app() {
    let items = ["Shut down", "Restart", "Back to desktop"];
    let mut sel = 0usize;
    loop {
        fill(1, 0, SCREEN_H - 2, SCREEN_W, DESK);
        fill(0, 0, 1, SCREEN_W, Color::LightGray);
        text(0, 1, "LightOS > Power", Color::Black, Color::LightGray);
        frame(8, 25, 9, 30, "Power", Color::White, Color::Blue);
        for (i, it) in items.iter().enumerate() {
            let (fg, bg) = if i == sel { (Color::Black, Color::LightGray) } else { (Color::White, Color::Blue) };
            fill(10 + i * 2, 28, 1, 24, bg);
            text(10 + i * 2, 30, it, fg, bg);
        }
        hint_bar("Up/Down select   Enter confirm   Esc back");
        match key() {
            KEY_UP => sel = (sel + items.len() - 1) % items.len(),
            KEY_DOWN => sel = (sel + 1) % items.len(),
            b'\n' => match sel {
                0 => shell::do_shutdown(),
                1 => shell::do_reboot(),
                _ => return,
            },
            0x1B => return,
            _ => {}
        }
    }
}
