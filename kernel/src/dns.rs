//! DNS resolver (A records over UDP).
use crate::{net, process};
use alloc::{format, string::{String, ToString}, vec::Vec};

fn be16(p: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*p.get(i)?, *p.get(i + 1)?]) as usize)
}

fn skip_name(p: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let l = *p.get(i)? as usize;
        if l == 0 {
            return Some(i + 1);
        }
        if l & 0xC0 == 0xC0 {
            return Some(i + 2);
        }
        i += 1 + l;
    }
}

enum Answer {
    Ip([u8; 4]),
    NoAddress,
    NoSuchHost,
    NotMine,
}

fn parse(p: &[u8], id: u16) -> Option<Answer> {
    if p.len() < 12 {
        return None;
    }
    if be16(p, 0)? != id as usize {
        return Some(Answer::NotMine);
    }
    if be16(p, 2)? & 0x000F == 3 {
        return Some(Answer::NoSuchHost);
    }
    let (qd, an) = (be16(p, 4)?, be16(p, 6)?);
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(p, i)? + 4;
    }
    for _ in 0..an {
        i = skip_name(p, i)?;
        let (ty, rdlen) = (be16(p, i)?, be16(p, i + 8)?);
        let rd = i + 10;
        if ty == 1 && rdlen == 4 && rd + 4 <= p.len() {
            return Some(Answer::Ip([p[rd], p[rd + 1], p[rd + 2], p[rd + 3]]));
        }
        i = rd + rdlen;
    }
    Some(Answer::NoAddress)
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Resolve a host name (or pass a dotted IPv4 address straight through).
pub fn resolve(name: &str) -> Result<[u8; 4], String> {
    if let Some(ip) = net::parse_ip(name) {
        return Ok(ip);
    }
    if !valid(name) {
        return Err(format!("'{}' is not a valid host name", name));
    }
    let server = net::dns();
    let port = 40000 + (process::ticks() % 20000) as u16;
    let id = (process::ticks() as u16) ^ 0x4C53;
    let mut q: Vec<u8> = Vec::new();
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion desired, 1 question
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0, 1, 0, 1]); // type A, class IN

    net::udp_listen(port);
    let mut result = Err("DNS timeout (no answer from the DNS server)".to_string());
    'tries: for _ in 0..3 {
        if let Err(e) = net::udp_send(server, port, 53, &q) {
            result = Err(e);
            break;
        }
        let start = process::ticks();
        while process::ticks() - start < 200 {
            if net::cancelled() {
                result = Err("cancelled".to_string());
                break 'tries;
            }
            net::poll();
            while let Some(m) = net::udp_recv(port) {
                match parse(&m.data, id) {
                    Some(Answer::Ip(ip)) => {
                        result = Ok(ip);
                        break 'tries;
                    }
                    Some(Answer::NoAddress) => {
                        result = Err(format!("no IPv4 address for '{}'", name));
                        break 'tries;
                    }
                    Some(Answer::NoSuchHost) => {
                        result = Err(format!("no such host '{}'", name));
                        break 'tries;
                    }
                    _ => {}
                }
            }
            net::wait_tick();
        }
    }
    net::udp_close(port);
    result
}
