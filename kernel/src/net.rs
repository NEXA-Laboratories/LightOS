//! Minimal network stack: Ethernet, ARP, IPv4, ICMP echo (ping).
//! The stack is polled: replies are processed while a command (ping, arp resolve) waits.
use crate::{e1000, process};
use alloc::{format, string::{String, ToString}, vec::Vec};
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;

pub struct Cfg {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub gw: [u8; 4],
}

/// Defaults match VirtualBox NAT and QEMU user networking.
static CFG: Mutex<Cfg> = Mutex::new(Cfg { ip: [10, 0, 2, 15], mask: [255, 255, 255, 0], gw: [10, 0, 2, 2] });
static ARP: Mutex<Vec<([u8; 4], [u8; 6])>> = Mutex::new(Vec::new());
static REPLY: Mutex<Option<(u16, u64)>> = Mutex::new(None); // (icmp seq, tick when received)
static IP_ID: AtomicU16 = AtomicU16::new(1);
static DNS: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 3]);

pub struct UdpMsg {
    pub data: Vec<u8>,
    pub src: [u8; 4],
    pub sport: u16,
}
static UDP_SOCKS: Mutex<Vec<(u16, Vec<UdpMsg>)>> = Mutex::new(Vec::new());

const PING_ID: u16 = 0x4C53;

pub fn fmt_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut it = s.split('.');
    for o in out.iter_mut() {
        *o = it.next()?.parse().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some(out)
}

pub fn config() -> (([u8; 4], [u8; 4], [u8; 4]), Option<[u8; 6]>) {
    let c = CFG.lock();
    ((c.ip, c.mask, c.gw), e1000::with(|n| n.mac))
}

pub fn set_config(ip: [u8; 4], mask: [u8; 4], gw: [u8; 4]) {
    let mut c = CFG.lock();
    c.ip = ip;
    c.mask = mask;
    c.gw = gw;
    ARP.lock().clear();
}

pub fn dns() -> [u8; 4] {
    *DNS.lock()
}
pub fn set_dns(ip: [u8; 4]) {
    *DNS.lock() = ip;
}
pub fn my_ip() -> [u8; 4] {
    CFG.lock().ip
}

/// Send an IP datagram to `dst` (ARP-resolving the next hop).
pub fn ip_send(proto: u8, dst: [u8; 4], payload: &[u8]) -> Result<(), String> {
    send_ip(proto, dst, payload, None)
}

pub fn mac() -> Option<[u8; 6]> {
    e1000::with(|n| n.mac)
}

pub fn arp_table() -> Vec<([u8; 4], [u8; 6])> {
    ARP.lock().clone()
}

pub(crate) fn csum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn my_mac() -> Result<[u8; 6], String> {
    e1000::with(|n| n.mac).ok_or_else(|| "no network card".to_string())
}

fn tx(frame: &[u8]) -> Result<(), String> {
    e1000::with(|n| n.send(frame)).ok_or_else(|| "no network card".to_string())?
}

fn arp_learn(ip: [u8; 4], mac: [u8; 6]) {
    let mut t = ARP.lock();
    if let Some(e) = t.iter_mut().find(|e| e.0 == ip) {
        e.1 = mac;
    } else if t.len() < 32 {
        t.push((ip, mac));
    }
}

fn send_arp(op: u16, target_mac: [u8; 6], target_ip: [u8; 4]) -> Result<(), String> {
    let me = my_mac()?;
    let my_ip = CFG.lock().ip;
    let mut f: Vec<u8> = Vec::with_capacity(60);
    f.extend_from_slice(&if op == 1 { [0xFF; 6] } else { target_mac });
    f.extend_from_slice(&me);
    f.extend_from_slice(&[0x08, 0x06]);
    f.extend_from_slice(&[0, 1, 8, 0, 6, 4]);
    f.extend_from_slice(&op.to_be_bytes());
    f.extend_from_slice(&me);
    f.extend_from_slice(&my_ip);
    f.extend_from_slice(&if op == 1 { [0; 6] } else { target_mac });
    f.extend_from_slice(&target_ip);
    while f.len() < 60 {
        f.push(0);
    }
    tx(&f)
}

/// Ctrl+C pressed?
pub fn cancelled() -> bool {
    crate::keyboard::cancel_requested()
}

pub fn wait_tick() {
    x86_64::instructions::interrupts::enable_and_hlt();
}

fn arp_resolve(ip: [u8; 4]) -> Result<[u8; 6], String> {
    for _ in 0..4 {
        if let Some(m) = ARP.lock().iter().find(|e| e.0 == ip).map(|e| e.1) {
            return Ok(m);
        }
        send_arp(1, [0; 6], ip)?;
        let start = process::ticks();
        while process::ticks() - start < 30 {
            if cancelled() {
                return Err("cancelled".to_string());
            }
            poll();
            if let Some(m) = ARP.lock().iter().find(|e| e.0 == ip).map(|e| e.1) {
                return Ok(m);
            }
            wait_tick();
        }
    }
    Err(format!("no ARP reply from {}", fmt_ip(ip)))
}

fn send_ip(proto: u8, dst: [u8; 4], payload: &[u8], dst_mac: Option<[u8; 6]>) -> Result<(), String> {
    let (src, next_hop) = {
        let c = CFG.lock();
        let same = (0..4).all(|i| dst[i] & c.mask[i] == c.ip[i] & c.mask[i]);
        (c.ip, if same { dst } else { c.gw })
    };
    let mac = match dst_mac {
        Some(m) => m,
        None => arp_resolve(next_hop)?,
    };
    send_ip_raw(proto, src, dst, payload, mac)
}

fn send_ip_raw(proto: u8, src: [u8; 4], dst: [u8; 4], payload: &[u8], mac: [u8; 6]) -> Result<(), String> {
    let me = my_mac()?;
    let total = 20 + payload.len();
    let mut f: Vec<u8> = Vec::with_capacity(14 + total);
    f.extend_from_slice(&mac);
    f.extend_from_slice(&me);
    f.extend_from_slice(&[0x08, 0x00]);
    let mut ip = [0u8; 20];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&IP_ID.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    ip[6] = 0x40; // don't fragment
    ip[8] = 64;
    ip[9] = proto;
    ip[12..16].copy_from_slice(&src);
    ip[16..20].copy_from_slice(&dst);
    let c = csum(&ip);
    ip[10..12].copy_from_slice(&c.to_be_bytes());
    f.extend_from_slice(&ip);
    f.extend_from_slice(payload);
    tx(&f)
}

// ------------------------------------------------------------------ UDP

pub fn udp_listen(port: u16) {
    let mut s = UDP_SOCKS.lock();
    if !s.iter().any(|x| x.0 == port) {
        s.push((port, Vec::new()));
    }
}

pub fn udp_close(port: u16) {
    UDP_SOCKS.lock().retain(|x| x.0 != port);
}

pub fn udp_recv(port: u16) -> Option<UdpMsg> {
    let mut s = UDP_SOCKS.lock();
    let q = s.iter_mut().find(|x| x.0 == port)?;
    if q.1.is_empty() {
        None
    } else {
        Some(q.1.remove(0))
    }
}

fn udp_listening(port: u16) -> bool {
    UDP_SOCKS.lock().iter().any(|x| x.0 == port)
}

fn udp_packet(sport: u16, dport: u16, payload: &[u8]) -> Vec<u8> {
    let mut u = Vec::with_capacity(8 + payload.len());
    u.extend_from_slice(&sport.to_be_bytes());
    u.extend_from_slice(&dport.to_be_bytes());
    u.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    u.extend_from_slice(&[0, 0]); // checksum optional in IPv4
    u.extend_from_slice(payload);
    u
}

pub fn udp_send(dst: [u8; 4], sport: u16, dport: u16, payload: &[u8]) -> Result<(), String> {
    send_ip(17, dst, &udp_packet(sport, dport, payload), None)
}

/// Broadcast with an explicit source address (DHCP uses 0.0.0.0 before it has a lease).
pub fn udp_send_broadcast(src: [u8; 4], sport: u16, dport: u16, payload: &[u8]) -> Result<(), String> {
    send_ip_raw(17, src, [255; 4], &udp_packet(sport, dport, payload), [0xFF; 6])
}

fn handle(f: &[u8]) {
    let my_ip = CFG.lock().ip;
    match u16::from_be_bytes([f[12], f[13]]) {
        0x0806 if f.len() >= 42 => {
            let op = u16::from_be_bytes([f[20], f[21]]);
            let mut sha = [0u8; 6];
            sha.copy_from_slice(&f[22..28]);
            let mut spa = [0u8; 4];
            spa.copy_from_slice(&f[28..32]);
            arp_learn(spa, sha);
            if op == 1 && f[38..42] == my_ip {
                let _ = send_arp(2, sha, spa);
            }
        }
        0x0800 if f.len() >= 34 => {
            let ihl = (f[14] & 0x0F) as usize * 4;
            let total = u16::from_be_bytes([f[16], f[17]]) as usize;
            if ihl < 20 || 14 + total > f.len() || total < ihl + 8 {
                return;
            }
            let proto = f[23];
            let dst = &f[30..34];
            let body = &f[14 + ihl..14 + total];
            let udp_port = u16::from_be_bytes([body[2], body[3]]);
            let for_us = dst == my_ip || dst == [255u8, 255, 255, 255];
            if !for_us && !(proto == 17 && udp_listening(udp_port)) {
                return;
            }
            let mut src = [0u8; 4];
            src.copy_from_slice(&f[26..30]);
            let mut smac = [0u8; 6];
            smac.copy_from_slice(&f[6..12]);
            match proto {
                1 => match body[0] {
                    8 => {
                        // echo request -> echo reply with the same payload
                        let mut r = body.to_vec();
                        r[0] = 0;
                        r[2] = 0;
                        r[3] = 0;
                        let c = csum(&r);
                        r[2..4].copy_from_slice(&c.to_be_bytes());
                        let _ = send_ip(1, src, &r, Some(smac));
                    }
                    0 => {
                        let id = u16::from_be_bytes([body[4], body[5]]);
                        let seq = u16::from_be_bytes([body[6], body[7]]);
                        if id == PING_ID {
                            *REPLY.lock() = Some((seq, process::ticks()));
                        }
                    }
                    _ => {}
                },
                6 => crate::tcp::on_segment(src, body),
                17 => {
                    let sport = u16::from_be_bytes([body[0], body[1]]);
                    let len = (u16::from_be_bytes([body[4], body[5]]) as usize).min(body.len());
                    if len >= 8 {
                        let mut socks = UDP_SOCKS.lock();
                        if let Some(q) = socks.iter_mut().find(|x| x.0 == udp_port) {
                            if q.1.len() < 8 {
                                q.1.push(UdpMsg { data: body[8..len].to_vec(), src, sport });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Process everything the NIC has received so far.
pub fn poll() {
    let mut buf = [0u8; 2048];
    loop {
        match e1000::with(|n| n.recv(&mut buf)).flatten() {
            Some(len) => {
                if len >= 14 {
                    handle(&buf[..len]);
                }
            }
            None => break,
        }
    }
}

pub fn ping(dst: [u8; 4], count: u32) {
    println!("PING {} with 32 bytes of data:", fmt_ip(dst));
    let mut got = 0;
    for seq in 1..=count {
        if cancelled() {
            println!("^C");
            break;
        }
        *REPLY.lock() = None;
        let mut icmp = [0u8; 40];
        icmp[0] = 8;
        icmp[4..6].copy_from_slice(&PING_ID.to_be_bytes());
        icmp[6..8].copy_from_slice(&(seq as u16).to_be_bytes());
        for (i, b) in icmp[8..].iter_mut().enumerate() {
            *b = b'a' + (i % 23) as u8;
        }
        let c = csum(&icmp);
        icmp[2..4].copy_from_slice(&c.to_be_bytes());
        let t0 = process::ticks();
        if let Err(e) = send_ip(1, dst, &icmp, None) {
            println!("send failed: {}", e);
            break;
        }
        let mut replied = false;
        while process::ticks() - t0 < 100 {
            if cancelled() {
                break;
            }
            poll();
            let r = *REPLY.lock();
            if let Some((s, t)) = r {
                if s == seq as u16 {
                    let ms = (t - t0) * 1000 / process::TICK_HZ;
                    if ms == 0 {
                        println!("Reply from {}: bytes=32 seq={} time<10ms", fmt_ip(dst), seq);
                    } else {
                        println!("Reply from {}: bytes=32 seq={} time={}ms", fmt_ip(dst), seq, ms);
                    }
                    got += 1;
                    replied = true;
                    break;
                }
            }
            wait_tick();
        }
        if !replied {
            if cancelled() {
                println!("^C");
                break;
            }
            println!("Request timed out (seq={}).", seq);
        }
        if seq < count {
            let used = process::ticks() - t0;
            if used < 100 {
                process::sleep_ms((100 - used) * 1000 / process::TICK_HZ);
            }
        }
    }
    println!("{} sent, {} received", count, got);
}
