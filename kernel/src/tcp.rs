//! TCP client: one connection at a time, stop-and-wait sending, in-order receive.
//! Enough for HTTP: connect, send a request, read the response until the peer closes.
use crate::{net, process};
use alloc::{format, string::{String, ToString}, vec::Vec};
use spin::Mutex;

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;
const MSS: usize = 1360;

#[derive(Clone, Copy, PartialEq)]
enum St {
    SynSent,
    Established,
    Closed,
}

struct Conn {
    st: St,
    dst: [u8; 4],
    lport: u16,
    rport: u16,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    rx: Vec<u8>,
    peer_fin: bool,
    reset: bool,
}

static CONN: Mutex<Option<Conn>> = Mutex::new(None);

fn be16(p: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([p[i], p[i + 1]])
}
fn be32(p: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]])
}

fn build(c: &Conn, flags: u8, seq: u32, payload: &[u8], syn_opt: bool) -> Vec<u8> {
    let hdr = if syn_opt { 24 } else { 20 };
    let mut s: Vec<u8> = Vec::with_capacity(hdr + payload.len());
    s.extend_from_slice(&c.lport.to_be_bytes());
    s.extend_from_slice(&c.rport.to_be_bytes());
    s.extend_from_slice(&seq.to_be_bytes());
    let ack = if flags & ACK != 0 { c.rcv_nxt } else { 0 };
    s.extend_from_slice(&ack.to_be_bytes());
    s.push(((hdr / 4) as u8) << 4);
    s.push(flags);
    s.extend_from_slice(&8192u16.to_be_bytes()); // window
    s.extend_from_slice(&[0, 0, 0, 0]); // checksum, urgent
    if syn_opt {
        s.extend_from_slice(&[2, 4]);
        s.extend_from_slice(&(MSS as u16).to_be_bytes());
    }
    s.extend_from_slice(payload);
    let mut pseudo: Vec<u8> = Vec::with_capacity(12 + s.len());
    pseudo.extend_from_slice(&net::my_ip());
    pseudo.extend_from_slice(&c.dst);
    pseudo.extend_from_slice(&[0, 6]);
    pseudo.extend_from_slice(&(s.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(&s);
    let ck = net::csum(&pseudo);
    s[16..18].copy_from_slice(&ck.to_be_bytes());
    s
}

/// Build under the lock, transmit after releasing it (sending may poll the NIC).
fn emit(flags: u8, seq: u32, payload: &[u8], syn_opt: bool) -> Result<(), String> {
    let (seg, dst) = {
        let g = CONN.lock();
        let c = g.as_ref().ok_or_else(|| "no connection".to_string())?;
        (build(c, flags, seq, payload, syn_opt), c.dst)
    };
    net::ip_send(6, dst, &seg)
}

/// Called by the IP layer for every TCP segment addressed to us.
pub fn on_segment(src: [u8; 4], seg: &[u8]) {
    if seg.len() < 20 {
        return;
    }
    let (sport, dport, seq, ack) = (be16(seg, 0), be16(seg, 2), be32(seg, 4), be32(seg, 8));
    let off = ((seg[12] >> 4) as usize) * 4;
    let flags = seg[13];
    if off < 20 || off > seg.len() {
        return;
    }
    let data = &seg[off..];
    let reply: Option<(Vec<u8>, [u8; 4])> = {
        let mut g = CONN.lock();
        let Some(c) = g.as_mut() else { return };
        if src != c.dst || sport != c.rport || dport != c.lport {
            return;
        }
        if flags & RST != 0 {
            c.reset = true;
            c.st = St::Closed;
            return;
        }
        match c.st {
            St::SynSent => {
                if flags & (SYN | ACK) == (SYN | ACK) && ack == c.snd_nxt {
                    c.rcv_nxt = seq.wrapping_add(1);
                    c.snd_una = ack;
                    c.st = St::Established;
                    Some((build(c, ACK, c.snd_nxt, &[], false), c.dst))
                } else {
                    None
                }
            }
            St::Established => {
                if flags & ACK != 0 && (ack.wrapping_sub(c.snd_una) as i32) > 0 && (ack.wrapping_sub(c.snd_nxt) as i32) <= 0 {
                    c.snd_una = ack;
                }
                let mut need_ack = flags & SYN != 0; // retransmitted SYN-ACK
                if !data.is_empty() {
                    if seq == c.rcv_nxt {
                        c.rx.extend_from_slice(data);
                        c.rcv_nxt = c.rcv_nxt.wrapping_add(data.len() as u32);
                    }
                    need_ack = true; // also acts as a duplicate ACK for out-of-order data
                }
                if flags & FIN != 0 && seq.wrapping_add(data.len() as u32) == c.rcv_nxt {
                    c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                    c.peer_fin = true;
                    need_ack = true;
                }
                if need_ack {
                    Some((build(c, ACK, c.snd_nxt, &[], false), c.dst))
                } else {
                    None
                }
            }
            St::Closed => None,
        }
    };
    if let Some((s, dst)) = reply {
        let _ = net::ip_send(6, dst, &s);
    }
}

/// Poll the NIC until `cond` holds for the connection, or `ticks` (10 ms each) pass.
fn wait_until(ticks: u64, cond: impl Fn(&Conn) -> bool) -> bool {
    let start = process::ticks();
    loop {
        if net::cancelled() {
            return false;
        }
        net::poll();
        if let Some(c) = CONN.lock().as_ref() {
            if cond(c) {
                return true;
            }
        }
        if process::ticks() - start >= ticks {
            return false;
        }
        net::wait_tick();
    }
}

pub fn connect(dst: [u8; 4], rport: u16) -> Result<(), String> {
    if CONN.lock().is_some() {
        return Err("a TCP connection is already open".to_string());
    }
    let iss = (process::ticks() as u32).wrapping_mul(2_654_435_761) ^ 0x1234_5678;
    let lport = 49152 + (process::ticks() % 10000) as u16;
    *CONN.lock() = Some(Conn {
        st: St::SynSent,
        dst,
        lport,
        rport,
        snd_una: iss,
        snd_nxt: iss.wrapping_add(1),
        rcv_nxt: 0,
        rx: Vec::new(),
        peer_fin: false,
        reset: false,
    });
    for _ in 0..4 {
        if let Err(e) = emit(SYN, iss, &[], true) {
            *CONN.lock() = None;
            return Err(e);
        }
        if wait_until(100, |c| c.st != St::SynSent) {
            let (st, reset) = {
                let g = CONN.lock();
                let c = g.as_ref().unwrap();
                (c.st, c.reset)
            };
            if st == St::Established {
                return Ok(());
            }
            *CONN.lock() = None;
            return Err(if reset { "connection refused".to_string() } else { "connection failed".to_string() });
        }
        if net::cancelled() {
            *CONN.lock() = None;
            return Err("cancelled".to_string());
        }
    }
    *CONN.lock() = None;
    Err(format!("connection to {}:{} timed out", net::fmt_ip(dst), rport))
}

pub fn send(data: &[u8]) -> Result<(), String> {
    for chunk in data.chunks(MSS) {
        let seq = {
            let mut g = CONN.lock();
            let c = g.as_mut().ok_or_else(|| "no connection".to_string())?;
            if c.st != St::Established {
                return Err("connection closed".to_string());
            }
            let s = c.snd_nxt;
            c.snd_nxt = c.snd_nxt.wrapping_add(chunk.len() as u32);
            s
        };
        let target = seq.wrapping_add(chunk.len() as u32);
        let mut acked = false;
        for _ in 0..6 {
            emit(PSH | ACK, seq, chunk, false)?;
            if wait_until(100, |c| c.snd_una == target || c.reset) {
                acked = true;
                break;
            }
        }
        if !acked {
            return Err(if net::cancelled() { "cancelled".to_string() } else { "send timed out (no ACK)".to_string() });
        }
        if CONN.lock().as_ref().map_or(true, |c| c.reset) {
            return Err("connection reset by peer".to_string());
        }
    }
    Ok(())
}

/// Wait up to `ticks` for data or a close; returns what arrived and whether the peer has closed.
pub fn recv(ticks: u64) -> (Vec<u8>, bool) {
    wait_until(ticks, |c| !c.rx.is_empty() || c.peer_fin || c.reset);
    let mut g = CONN.lock();
    match g.as_mut() {
        Some(c) => (core::mem::take(&mut c.rx), c.peer_fin || c.reset),
        None => (Vec::new(), true),
    }
}

pub fn close() {
    let sent = {
        let mut g = CONN.lock();
        match g.as_mut() {
            Some(c) if c.st == St::Established => {
                let s = c.snd_nxt;
                c.snd_nxt = c.snd_nxt.wrapping_add(1);
                Some(s)
            }
            _ => None,
        }
    };
    if let Some(seq) = sent {
        if emit(FIN | ACK, seq, &[], false).is_ok() {
            let target = seq.wrapping_add(1);
            wait_until(50, |c| c.snd_una == target || c.reset);
        }
    }
    *CONN.lock() = None;
}
