//! Client side of the LOS serial protocol (talks to `los-server` over COM1).
//!
//!   kernel -> LOS/1 LIST          server -> PKG|name|version|author|desc|verified|deps|hash|size ... END
//!   kernel -> LOS/1 GET <name>    server -> DATA|<name>|<escaped payload>   (or ERR|message)
//!
//! Run QEMU with `-serial tcp:127.0.0.1:7878` and start `los_server.py serve`.
use super::{Origin, PackageMeta};
use crate::serial;
use alloc::{format, string::{String, ToString}, vec::Vec};

/// Busy-wait budget per byte before we declare the server absent.
const TIMEOUT: u32 = 1_500_000;

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
