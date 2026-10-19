//! Client side of the LOS serial protocol (talks to `los-server` over COM1).
//!
//!   kernel -> LOS/1 LIST          server -> PKG|name|version|author|desc|verified|deps|hash|size ... END
//!   kernel -> LOS/1 GET <name>    server -> DATA|<name>|<escaped payload>   (or ERR|message)
//!
//! Run QEMU with `-serial tcp:127.0.0.1:7878` and start `los_server.py serve`.
use super::{Origin, PackageMeta};
use crate::{e1000, http, net, serial};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use alloc::{format, string::{String, ToString}, vec::Vec};

/// los-server over HTTP: 10.0.2.2 is the host as seen from VirtualBox NAT / QEMU user networking.
static SERVER: Mutex<([u8; 4], u16)> = Mutex::new(([10, 0, 2, 2], 8080));
static USE_NET: AtomicBool = AtomicBool::new(false);

pub fn server() -> ([u8; 4], u16) {
    *SERVER.lock()
}

pub fn set_server(ip: [u8; 4], port: u16) {
    *SERVER.lock() = (ip, port);
}

pub fn transport() -> String {
    if USE_NET.load(Ordering::Relaxed) {
        let (ip, port) = server();
        format!("network {}:{}", net::fmt_ip(ip), port)
    } else {
        String::from("serial COM1")
    }
}

fn net_ready() -> bool {
    e1000::with(|n| n.link_up()).unwrap_or(false)
}

fn http_get(path: &str) -> Result<Vec<u8>, String> {
    let (ip, port) = server();
    let host = format!("{}:{}", net::fmt_ip(ip), port);
    let r = http::get(ip, port, &host, path)?;
    if r.status != 200 {
        return Err(format!("server answered HTTP {}", r.status));
    }
    Ok(r.body)
}

/// Busy-wait budget per byte before we declare the server absent.
const TIMEOUT: u32 = 600_000;

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 48 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn parse_pkg(s: &str) -> Option<PackageMeta> {
    let mut it = s.split('|');
    let name = it.next()?;
    let version = it.next()?;
    let author = it.next()?;
    let description = it.next()?;
    let verified = it.next()? == "1";
    let deps = it.next()?;
    let hash = u64::from_str_radix(it.next()?, 16).ok()?;
    if !valid_name(name) {
        return None;
    }
    Some(PackageMeta {
        name: name.to_string(),
        version: version.to_string(),
        author: author.to_string(),
        description: description.to_string(),
        verified,
        deps: if deps.is_empty() { Vec::new() } else { deps.split(',').map(|d| d.to_string()).collect() },
        hash,
        origin: Origin::Server,
    })
}

pub fn fetch_index() -> Result<Vec<PackageMeta>, String> {
    let mut net_err = String::from("no network link");
    if net_ready() {
        let (ip, port) = server();
        println!("  trying HTTP {}:{} (Ctrl+C cancels) ...", net::fmt_ip(ip), port);
        match http_get("/api/v1/index.txt") {
            Ok(body) => {
                let text = String::from_utf8_lossy(&body).to_string();
                let out: Vec<PackageMeta> = text.lines().filter_map(|l| l.strip_prefix("PKG|")).filter_map(parse_pkg).collect();
                USE_NET.store(true, Ordering::Relaxed);
                return Ok(out);
            }
            Err(e) => {
                println!("  HTTP failed: {}", e);
                if crate::keyboard::cancel_requested() {
                    return Err(String::from("cancelled"));
                }
                net_err = e;
            }
        }
    }
    USE_NET.store(false, Ordering::Relaxed);
    if !serial::present() {
        return Err(format!("network: {}; COM1 is not connected", net_err));
    }
    println!("  trying COM1 (Ctrl+C cancels) ...");
    fetch_index_serial().map_err(|e| {
        if crate::keyboard::cancel_requested() {
            String::from("cancelled")
        } else {
            format!("network: {}; serial: {}", net_err, e)
        }
    })
}

fn fetch_index_serial() -> Result<Vec<PackageMeta>, String> {
    serial::drain();
    serial::write_line("LOS/1 LIST");
    let mut out = Vec::new();
    loop {
        let line = serial::read_line(TIMEOUT).ok_or_else(|| String::from("no response from los-server on COM1"))?;
        if line == "END" {
            return Ok(out);
        }
        if let Some(e) = line.strip_prefix("ERR|") {
            return Err(format!("server error: {}", e));
        }
        if let Some(rest) = line.strip_prefix("PKG|") {
            if let Some(m) = parse_pkg(rest) {
                out.push(m);
            }
        }
    }
}

pub fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
}

pub fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn fetch_payload(name: &str) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("invalid package name '{}'", name));
    }
    if USE_NET.load(Ordering::Relaxed) {
        let body = http_get(&format!("/api/v1/packages/{}/payload", name))?;
        return String::from_utf8(body).map_err(|_| String::from("payload is not text"));
    }
    serial::drain();
    serial::write_line(&format!("LOS/1 GET {}", name));
    let prefix = format!("DATA|{}|", name);
    loop {
        let line = serial::read_line(TIMEOUT).ok_or_else(|| String::from("no response from los-server on COM1"))?;
        if let Some(e) = line.strip_prefix("ERR|") {
            return Err(format!("server error: {}", e));
        }
        if let Some(p) = line.strip_prefix(prefix.as_str()) {
            return Ok(unescape(p));
        }
    }
}
