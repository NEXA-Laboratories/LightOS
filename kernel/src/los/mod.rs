//! LOS - the LightOS package manager.
//!
//! install / remove / list / search / update / verify / dependency resolution.
//! Every package is checked against its repository hash before it is installed.
pub mod hash;
pub mod hello_elf;
pub mod remote;
pub mod repo;

use crate::{elf, fat32, process, ui, vga_buffer::{self, Color}};
use alloc::{collections::BTreeMap, format, string::{String, ToString}, vec::Vec};
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Builtin,
    Server,
}

#[derive(Clone, Debug)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub verified: bool,
    pub deps: Vec<String>,
    pub hash: u64,
    pub origin: Origin,
}

#[derive(Clone)]
struct Installed {
    meta: PackageMeta,
    explicit: bool,
    size: usize,
    tick: u64,
    payload: String,
}

struct State {
    index: BTreeMap<String, PackageMeta>,
    installed: BTreeMap<String, Installed>,
    cache: BTreeMap<String, String>,
    synced: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    index: BTreeMap::new(),
    installed: BTreeMap::new(),
    cache: BTreeMap::new(),
    synced: false,
});

pub fn init() {
    let mut st = STATE.lock();
    for p in repo::BUILTIN {
        st.index.insert(p.name.to_string(), p.to_meta());
    }
    load_db(&mut st);
}

// ------------------------------------------------------------------ output helpers

fn say(color: Color, tag: &str, msg: &str) {
    vga_buffer::set_color(color, Color::Black);
    print!("{} ", tag);
    vga_buffer::set_color(Color::LightGray, Color::Black);
    println!("{}", msg);
}
fn ok(msg: &str) {
    say(Color::LightGreen, "✓", msg);
}
fn warn(msg: &str) {
    say(Color::Yellow, "⚠", msg);
}
fn fail(msg: &str) {
    say(Color::LightRed, "error:", msg);
}
fn info_line(msg: &str) {
    say(Color::LightCyan, "[pkg]", msg);
}

// ------------------------------------------------------------------ core logic

fn payload_of(st: &mut State, meta: &PackageMeta) -> Result<String, String> {
    match meta.origin {
        Origin::Builtin => repo::BUILTIN
            .iter()
            .find(|p| p.name == meta.name)
            .map(|p| p.payload.to_string())
            .ok_or_else(|| format!("no payload for '{}'", meta.name)),
        Origin::Server => {
            if let Some(p) = st.cache.get(&meta.name) {
                return Ok(p.clone());
            }
            let p = remote::fetch_payload(&meta.name)?;
            st.cache.insert(meta.name.clone(), p.clone());
            Ok(p)
        }
    }
}

/// Depth-first dependency resolution; `out` ends up in install order (deps first).
fn resolve(st: &State, name: &str, visiting: &mut Vec<String>, out: &mut Vec<String>) -> Result<(), String> {
    if out.iter().any(|n| n == name) {
        return Ok(());
    }
    if visiting.iter().any(|n| n == name) {
        return Err(format!("dependency cycle detected at '{}'", name));
    }
    let meta = st
        .index
        .get(name)
        .ok_or_else(|| format!("package '{}' not found (try 'los update')", name))?;
    visiting.push(name.to_string());
    for d in &meta.deps {
        resolve(st, d, visiting, out)?;
    }
    visiting.pop();
    out.push(name.to_string());
    Ok(())
}

pub fn install(name: &str) -> Result<(), String> {
    let mut st = STATE.lock();
    let mut order = Vec::new();
    resolve(&st, name, &mut Vec::new(), &mut order)?;

    let todo: Vec<String> = order.into_iter().filter(|n| !st.installed.contains_key(n)).collect();
    if todo.is_empty() {
        if let Some(i) = st.installed.get_mut(name) {
            i.explicit = true;
        }
        info_line(&format!("{} is already installed", name));
        return Ok(());
    }

    // Phase 1: fetch + verify everything first, so a bad package changes nothing.
    let mut staged: Vec<(PackageMeta, String)> = Vec::new();
    for pkg in &todo {
        let meta = st.index.get(pkg).cloned().ok_or_else(|| format!("package '{}' vanished", pkg))?;
        let payload = payload_of(&mut st, &meta)?;
        let h = hash::fnv1a64(payload.as_bytes());
        if h != meta.hash {
            return Err(format!(
                "SECURITY: checksum mismatch for {} (expected {:016x}, got {:016x}); nothing was installed",
                meta.name, meta.hash, h
            ));
        }
        if !meta.verified {
            warn(&format!("{} is NOT verified by the LOS review process - install at your own risk", meta.name));
        }
        staged.push((meta, payload));
    }

    // Phase 2: commit.
    let now = process::ticks();
    for (meta, payload) in staged {
        let size = payload.len();
        if meta.name == name {
            info_line(&format!("Installing {}-{}", meta.name, meta.version));
            println!("   Author: {}", meta.author);
            println!("   Description: {}", meta.description);
        } else {
            info_line(&format!("Installing dependency: {}", meta.name));
        }
        let explicit = meta.name == name;
        let key = meta.name.clone();
        let label = meta.name.clone();
        st.installed.insert(key, Installed { meta, explicit, size, tick: now, payload });
        ok(&format!("Successfully installed {}", label));
    }
    persist(&st);
    Ok(())
}

pub fn remove(name: &str, force: bool) -> Result<(), String> {
    let mut st = STATE.lock();
    if !st.installed.contains_key(name) {
        return Err(format!("package '{}' is not installed", name));
    }
    let dependents: Vec<String> = st
        .installed
        .values()
        .filter(|i| i.meta.name != name && i.meta.deps.iter().any(|d| d == name))
        .map(|i| i.meta.name.clone())
        .collect();
    if !dependents.is_empty() && !force {
        return Err(format!(
            "cannot remove {}: required by {} (use --force to override)",
            name,
            dependents.join(", ")
        ));
    }
    st.installed.remove(name);
    ok(&format!("Removed {}", name));
    persist(&st);
    Ok(())
}

/// Remove dependency-only packages nobody needs any more.
pub fn autoremove() -> usize {
    let mut st = STATE.lock();
    let mut removed = 0;
    loop {
        let orphan = st
            .installed
            .values()
            .find(|i| {
                !i.explicit
                    && !st.installed.values().any(|o| o.meta.name != i.meta.name && o.meta.deps.iter().any(|d| *d == i.meta.name))
            })
            .map(|i| i.meta.name.clone());
        match orphan {
            Some(n) => {
                st.installed.remove(&n);
                ok(&format!("Removed unused dependency {}", n));
                removed += 1;
            }
            None => break,
        }
    }
    if removed > 0 {
        persist(&st);
    }
    removed
}

pub fn list() -> usize {
    let rows: Vec<String> = {
        let st = STATE.lock();
        st.installed
            .values()
            .map(|i| {
                format!(
                    "{} {}-{}  by {}{}  [{} B, t={}]",
                    if i.meta.verified { "✓" } else { "✗" },
                    i.meta.name,
                    i.meta.version,
                    i.meta.author,
                    if i.explicit { "" } else { " (dep)" },
                    i.size,
                    i.tick
                )
            })
            .collect()
    };
    if rows.is_empty() {
        println!("No packages installed.");
    } else {
        ui::table("Installed Packages", None, &rows);
    }
    rows.len()
}

fn table_of<'a>(metas: impl Iterator<Item = &'a PackageMeta>, title: &str) -> usize {
    let rows: Vec<String> = metas
        .map(|m| format!("{:<14} {:<8} {:<9} {}", m.name, m.version, if m.verified { "✓" } else { "✗" }, m.description))
        .collect();
    if rows.is_empty() {
        println!("No matching packages.");
    } else {
        ui::table(title, Some("Package        Version  Verified  Description"), &rows);
    }
    rows.len()
}

pub fn available() -> usize {
    let st = STATE.lock();
    table_of(st.index.values(), "Available Packages")
}

pub fn search(query: &str) -> usize {
    let q = query.to_ascii_lowercase();
    let st = STATE.lock();
    table_of(
        st.index.values().filter(|m| m.name.to_ascii_lowercase().contains(&q) || m.description.to_ascii_lowercase().contains(&q)),
        &format!("Search: {}", query),
    )
}

pub fn info(name: &str) -> Result<(), String> {
    let st = STATE.lock();
    let m = st.index.get(name).ok_or_else(|| format!("package '{}' not found", name))?;
    println!("Name:        {}", m.name);
    println!("Version:     {}", m.version);
    println!("Author:      {}", m.author);
    println!("Description: {}", m.description);
    println!("Verified:    {}", if m.verified { "yes" } else { "NO (unreviewed)" });
    println!("Depends on:  {}", if m.deps.is_empty() { String::from("-") } else { m.deps.join(", ") });
    println!("Source:      {}", if m.origin == Origin::Builtin { "built-in repository" } else { "los-server" });
    println!("Checksum:    fnv1a64:{:016x}", m.hash);
    println!("Installed:   {}", if st.installed.contains_key(name) { "yes" } else { "no" });
    Ok(())
}

/// Sync the package index with los-server. Falls back to the built-in repo if offline.
pub fn update() -> Result<usize, String> {
    let fetched = remote::fetch_index()?;
    let mut st = STATE.lock();
    st.index.retain(|_, m| m.origin == Origin::Builtin);
    st.cache.clear();
    let n = fetched.len();
    for m in fetched {
        st.index.insert(m.name.clone(), m);
    }
    st.synced = true;
    Ok(n)
}

/// Re-check installed packages against the repository hashes.
pub fn verify(name: Option<&str>) -> Result<(), String> {
    let st = STATE.lock();
    let names: Vec<String> = match name {
        Some(n) => {
            if !st.installed.contains_key(n) {
                return Err(format!("package '{}' is not installed", n));
            }
            alloc::vec![n.to_string()]
        }
        None => st.installed.keys().cloned().collect(),
    };
    if names.is_empty() {
        println!("Nothing to verify.");
        return Ok(());
    }
    let mut bad = 0;
    for n in names {
        let meta = st.installed[&n].meta.clone();
        let stored = st.installed[&n].payload.clone();
        if hash::fnv1a64(stored.as_bytes()) == meta.hash {
            ok(&format!("{}-{} checksum OK", meta.name, meta.version));
        } else {
            bad += 1;
            fail(&format!("{} checksum MISMATCH", meta.name));
        }
    }
    if bad > 0 {
        Err(format!("{} package(s) failed verification", bad))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ persistence (FAT32)

const DB_DIR: &str = "/LOS";
const DB_FILE: &str = "/LOS/DB.TXT";

fn persist(st: &State) {
    if !fat32::is_mounted() {
        return;
    }
    let mut out = String::new();
    for i in st.installed.values() {
        let m = &i.meta;
        out.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{:016x}|{}|{}|{}|{}\n",
            m.name,
            m.version,
            m.author,
            m.description,
            if m.verified { 1 } else { 0 },
            m.deps.join(","),
            m.hash,
            if m.origin == Origin::Builtin { "b" } else { "s" },
            if i.explicit { 1 } else { 0 },
            i.tick,
            remote::escape(&i.payload)
        ));
    }
    let r = fat32::with(|fs| {
        if !fs.is_dir(DB_DIR) {
            fs.mkdir(DB_DIR)?;
        }
        fs.write_file(DB_FILE, out.as_bytes())
    });
    if let Err(e) = r {
        warn(&format!("could not save package database: {}", e));
    }
}

fn load_db(st: &mut State) {
    if !fat32::is_mounted() {
        return;
    }
    let Ok(bytes) = fat32::with(|fs| fs.read_file(DB_FILE)) else {
        return; // first boot: no database yet
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    for line in text.lines() {
        let f: Vec<&str> = line.splitn(11, '|').collect();
        if f.len() != 11 {
            continue;
        }
        let Ok(hash) = u64::from_str_radix(f[6], 16) else { continue };
        let payload = remote::unescape(f[10]);
        let meta = PackageMeta {
            name: f[0].to_string(),
            version: f[1].to_string(),
            author: f[2].to_string(),
            description: f[3].to_string(),
            verified: f[4] == "1",
            deps: if f[5].is_empty() { Vec::new() } else { f[5].split(',').map(|d| d.to_string()).collect() },
            hash,
            origin: if f[7] == "b" { Origin::Builtin } else { Origin::Server },
        };
        let size = payload.len();
        st.installed.insert(
            meta.name.clone(),
            Installed { meta, explicit: f[8] == "1", size, tick: f[9].parse().unwrap_or(0), payload },
        );
    }
}

pub struct CatalogEntry {
    pub name: String,
    pub version: String,
    pub verified: bool,
    pub installed: bool,
    pub description: String,
}

/// Every package in the current index, with its installed state (for the desktop's package manager).
pub fn catalog() -> Vec<CatalogEntry> {
    let st = STATE.lock();
    st.index
        .values()
        .map(|m| CatalogEntry {
            name: m.name.clone(),
            version: m.version.clone(),
            verified: m.verified,
            installed: st.installed.contains_key(&m.name),
            description: m.description.clone(),
        })
        .collect()
}

/// Package-database lines (same format as persist) for a built-in package and its built-in dependencies.
/// The installer uses this to pre-install packages on a new disk.
pub fn db_lines_for(name: &str) -> Vec<String> {
    fn walk(n: &str, out: &mut Vec<String>) {
        if out.iter().any(|x| x == n) {
            return;
        }
        if let Some(b) = repo::BUILTIN.iter().find(|b| b.name == n) {
            for d in b.deps {
                walk(d, out);
            }
            out.push(n.to_string());
        }
    }
    let mut order: Vec<String> = Vec::new();
    walk(name, &mut order);
    order
        .iter()
        .filter_map(|n| repo::BUILTIN.iter().find(|b| b.name == n.as_str()))
        .map(|b| {
            let m = b.to_meta();
            format!(
                "{}|{}|{}|{}|{}|{}|{:016x}|b|{}|0|{}",
                m.name,
                m.version,
                m.author,
                m.description,
                if m.verified { 1 } else { 0 },
                m.deps.join(","),
                m.hash,
                if m.name == name { 1 } else { 0 },
                remote::escape(b.payload)
            )
        })
        .collect()
}

pub fn is_installed(name: &str) -> bool {
    STATE.lock().installed.contains_key(name)
}

pub fn installed_count() -> usize {
    STATE.lock().installed.len()
}

// ------------------------------------------------------------------ apps

/// App packages carry an ELF64 binary as hex text (lines starting with '#' are comments).
fn decode_app(payload: &str) -> Result<Vec<u8>, String> {
    let mut digits: Vec<u8> = Vec::new();
    for line in payload.lines() {
        if line.starts_with('#') {
            continue;
        }
        digits.extend(line.bytes().filter(|b| !b.is_ascii_whitespace()));
    }
    if digits.len() % 2 != 0 {
        return Err(String::from("app payload is not valid hex"));
    }
    let nib = |b: u8| -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err(String::from("app payload is not valid hex")),
        }
    };
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        out.push((nib(pair[0])? << 4) | nib(pair[1])?);
    }
    Ok(out)
}

/// `los run <pkg>`: execute an installed app package.
pub fn run(name: &str) -> Result<i32, String> {
    let payload = {
        let st = STATE.lock();
        let i = st.installed.get(name).ok_or_else(|| format!("package '{}' is not installed (los install {})", name, name))?;
        if hash::fnv1a64(i.payload.as_bytes()) != i.meta.hash {
            return Err(format!("SECURITY: stored copy of {} fails its checksum; refusing to run", name));
        }
        i.payload.clone()
    };
    let elf_bytes = decode_app(&payload).map_err(|_| format!("'{}' is not an application package", name))?;
    elf::exec(name, &elf_bytes)
}

// ------------------------------------------------------------------ shell front-end

fn usage() {
    println!("LOS package manager {}", crate::VERSION);
    println!("  los install <pkg>...   install packages (+ dependencies)");
    println!("  los remove <pkg>...    remove packages (--force ignores dependents)");
    println!("  los list               list installed packages");
    println!("  los search <query>     search name/description");
    println!("  los update             sync index from los-server (HTTP, falls back to COM1)");
    println!("  los server [ip[:port]] show/set the los-server address (default 10.0.2.2:8080)");
    println!("  los available          show the current index");
    println!("  los info <pkg>         package details");
    println!("  los verify [pkg]       re-check installed checksums");
    println!("  los autoremove         drop unused dependencies");
    println!("  los run <pkg>          run an installed app package (ELF64)");
    println!("  los submit             publish (done on the host, see below)");
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        fail(&e);
    }
}

pub fn command(args: &[&str]) {
    let Some(&sub) = args.first() else {
        usage();
        return;
    };
    let force = args.iter().any(|a| *a == "--force");
    let pos: Vec<&str> = args[1..].iter().copied().filter(|a| !a.starts_with("--")).collect();
    match sub {
        "install" | "add" => {
            if pos.is_empty() {
                println!("usage: los install <package>");
            }
            for p in pos {
                report(install(p));
            }
        }
        "remove" | "rm" | "uninstall" => {
            if pos.is_empty() {
                println!("usage: los remove <package>");
            }
            for p in pos {
                report(remove(p, force));
            }
        }
        "list" | "ls" => {
            list();
        }
        "search" => {
            if pos.is_empty() {
                println!("usage: los search <query>");
            } else {
                search(&pos.join(" "));
            }
        }
        "available" | "avail" => {
            available();
        }
        "update" | "sync" => {
            info_line("Contacting los-server (network, then COM1) ...");
            match update() {
                Ok(n) => {
                    ok(&format!("Repository updated: {} package(s) from los-server ({})", n, remote::transport()));
                    available();
                }
                Err(e) => {
                    warn(&format!("{}; using the built-in repository", e));
                    available();
                }
            }
        }
        "server" => match pos.first() {
            Some(a) => {
                let (h, p) = match a.rsplit_once(':') {
                    Some((h, p)) => (h, p.parse::<u16>().ok()),
                    None => (*a, Some(8080)),
                };
                match (crate::net::parse_ip(h), p) {
                    (Some(ip), Some(port)) => {
                        remote::set_server(ip, port);
                        ok(&format!("los-server is now {}:{}", crate::net::fmt_ip(ip), port));
                    }
                    _ => println!("usage: los server <a.b.c.d[:port]>"),
                }
            }
            None => {
                let (ip, port) = remote::server();
                println!("los-server: {}:{}  (last used: {})", crate::net::fmt_ip(ip), port, remote::transport());
            }
        },
        "info" => match pos.first() {
            Some(p) => report(info(p)),
            None => println!("usage: los info <package>"),
        },
        "verify" => report(verify(pos.first().copied())),
        "run" => match pos.first() {
            Some(p) => match run(p) {
                Ok(code) => println!("{} exited with code {}", p, code),
                Err(e) => fail(&e),
            },
            None => println!("usage: los run <package>"),
        },
        "autoremove" => {
            let n = autoremove();
            println!("{} package(s) removed", n);
        }
        "submit" => {
            println!("Packages are published from a host machine, not from inside the OS:");
            println!("    python3 los_server.py submit <dir-with-package.toml>");
            println!("The server runs the automatic security check; a maintainer then");
            println!("marks the package verified.");
        }
        "version" | "--version" => println!("LOS {}", crate::VERSION),
        "help" | "--help" | "-h" => usage(),
        other => {
            println!("los: unknown command '{}'", other);
            usage();
        }
    }
}
