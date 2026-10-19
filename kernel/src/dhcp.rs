//! DHCP client (DISCOVER -> OFFER -> REQUEST -> ACK) over the UDP layer.
use crate::{net, process};
use alloc::{format, string::{String, ToString}, vec, vec::Vec};

struct Lease {
    kind: u8,
    ip: [u8; 4],
    mask: [u8; 4],
    gw: [u8; 4],
    dns: [u8; 4],
    server: [u8; 4],
}

fn build(kind: u8, xid: u32, mac: [u8; 6], req: Option<([u8; 4], [u8; 4])>) -> Vec<u8> {
    let mut p = vec![0u8; 236];
    p[0] = 1; // BOOTREQUEST
    p[1] = 1; // ethernet
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10] = 0x80; // flags: please broadcast the reply
    p[28..34].copy_from_slice(&mac);
    p.extend_from_slice(&[0x63, 0x82, 0x53, 0x63]); // magic cookie
    p.extend_from_slice(&[53, 1, kind]);
    if let Some((ip, server)) = req {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip);
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&server);
    }
    p.extend_from_slice(&[55, 4, 1, 3, 6, 51]); // want: mask, router, dns, lease time
    p.push(255);
    while p.len() < 300 {
        p.push(0);
    }
    p
}

fn parse(p: &[u8], xid: u32, mac: &[u8; 6]) -> Option<Lease> {
    if p.len() < 241 || p[0] != 2 || p[4..8] != xid.to_be_bytes() || p[28..34] != mac[..] || p[236..240] != [0x63u8, 0x82, 0x53, 0x63] {
        return None;
    }
    let mut l = Lease { kind: 0, ip: [0; 4], mask: [255, 255, 255, 0], gw: [0; 4], dns: [0; 4], server: [0; 4] };
    l.ip.copy_from_slice(&p[16..20]);
    let mut i = 240;
    while i < p.len() {
        let code = p[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        if i + 1 >= p.len() {
            break;
        }
        let n = p[i + 1] as usize;
        if i + 2 + n > p.len() {
            break;
        }
        let v = &p[i + 2..i + 2 + n];
        match code {
            53 if n >= 1 => l.kind = v[0],
            1 if n >= 4 => l.mask.copy_from_slice(&v[..4]),
            3 if n >= 4 => l.gw.copy_from_slice(&v[..4]),
            6 if n >= 4 => l.dns.copy_from_slice(&v[..4]),
            54 if n >= 4 => l.server.copy_from_slice(&v[..4]),
            _ => {}
        }
        i += 2 + n;
    }
    Some(l)
}

fn wait_for(xid: u32, mac: &[u8; 6], kind: u8, wait_ticks: u64) -> Option<Lease> {
    let start = process::ticks();
    while process::ticks() - start < wait_ticks {
        if net::cancelled() {
            return None;
        }
        net::poll();
        while let Some(m) = net::udp_recv(68) {
            if let Some(l) = parse(&m.data, xid, mac) {
                if l.kind == kind || l.kind == 6 {
                    return Some(l);
                }
            }
        }
        net::wait_tick();
    }
    None
}

/// Ask a DHCP server for an address and apply it. `wait_ticks` is the per-step timeout.
pub fn run(tries: u32, wait_ticks: u64) -> Result<String, String> {
    let mac = net::mac().ok_or_else(|| "no network card".to_string())?;
    net::udp_listen(68);
    let r = (|| {
        for _ in 0..tries {
            if net::cancelled() {
                return Err("cancelled".to_string());
            }
            let xid = (process::ticks() as u32).wrapping_mul(2_654_435_761) ^ 0x4C4F_5321;
            net::udp_send_broadcast([0; 4], 68, 67, &build(1, xid, mac, None))?;
            let Some(offer) = wait_for(xid, &mac, 2, wait_ticks) else { continue };
            if offer.kind != 2 {
                continue;
            }
            net::udp_send_broadcast([0; 4], 68, 67, &build(3, xid, mac, Some((offer.ip, offer.server))))?;
            let Some(ack) = wait_for(xid, &mac, 5, wait_ticks) else { continue };
            if ack.kind != 5 {
                continue;
            }
            net::set_config(ack.ip, ack.mask, if ack.gw == [0; 4] { offer.gw } else { ack.gw });
            if ack.dns != [0; 4] {
                net::set_dns(ack.dns);
            }
            return Ok(format!(
                "DHCP: leased {} (gateway {}, dns {})",
                net::fmt_ip(ack.ip),
                net::fmt_ip(net::config().0 .2),
                net::fmt_ip(net::dns())
            ));
        }
        Err("no DHCP server answered".to_string())
    })();
    net::udp_close(68);
    r
}
