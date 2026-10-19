//! Tiny HTTP/1.1 client (GET only, no TLS, no redirects).
use crate::{dns, net, tcp};
use alloc::{format, string::{String, ToString}, vec::Vec};

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Split "http://host[:port]/path" into (host, port, path).
pub fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else if url.starts_with("https://") {
        return Err("HTTPS is not supported yet (use http://)".to_string());
    } else {
        url
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port".to_string())?),
        None => (hostport, 80),
    };
    if host.is_empty() {
        return Err("missing host".to_string());
    }
    Ok((host.to_string(), port, path.to_string()))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn header_value(head: &str, name: &str) -> Option<String> {
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = find(b, b"\r\n") else { break };
        let size_str = core::str::from_utf8(&b[..nl]).unwrap_or("0");
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        b = &b[nl + 2..];
        if size == 0 || b.len() < size {
            if size != 0 {
                out.extend_from_slice(b); // truncated: keep what we have
            }
            break;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size..];
        if b.starts_with(b"\r\n") {
            b = &b[2..];
        }
    }
    out
}

/// GET `path` from `ip:port`, sending `host` in the Host header.
pub fn get(ip: [u8; 4], port: u16, host: &str, path: &str) -> Result<Response, String> {
    tcp::connect(ip, port)?;
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: LightOS/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path,
        host,
        crate::VERSION
    );
    let result = (|| -> Result<Response, String> {
        tcp::send(req.as_bytes())?;
        let mut buf: Vec<u8> = Vec::new();
        let mut closed = false;
        while !closed {
            let (d, c) = tcp::recv(300);
            if d.is_empty() && !c {
                break; // 3 s of silence
            }
            buf.extend_from_slice(&d);
            closed = c;
            if buf.len() > 2 * 1024 * 1024 {
                return Err("response too large".to_string());
            }
            // stop early once Content-Length bytes have arrived
            if let Some(hend) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..hend]).to_string();
                if let Some(cl) = header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok()) {
                    if buf.len() >= hend + 4 + cl {
                        break;
                    }
                }
            }
        }
        if net::cancelled() {
            return Err("cancelled".to_string());
        }
        let hend = find(&buf, b"\r\n\r\n").ok_or_else(|| "no HTTP response (timeout)".to_string())?;
        let head = String::from_utf8_lossy(&buf[..hend]).to_string();
        let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| "bad HTTP status line".to_string())?;
        let mut body = buf[hend + 4..].to_vec();
        if header_value(&head, "transfer-encoding").map_or(false, |v| v.to_ascii_lowercase().contains("chunked")) {
            body = dechunk(&body);
        } else if let Some(cl) = header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok()) {
            body.truncate(cl);
        }
        Ok(Response { status, body })
    })();
    tcp::close();
    result
}

/// GET a full URL, resolving the host name.
pub fn get_url(url: &str) -> Result<Response, String> {
    let (host, port, path) = parse_url(url)?;
    let ip = dns::resolve(&host)?;
    let hostport = if port == 80 { host.clone() } else { format!("{}:{}", host, port) };
    get(ip, port, &hostport, &path)
}
