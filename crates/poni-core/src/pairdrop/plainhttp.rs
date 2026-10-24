//! A plain-HTTP [`Post`](super::Post) for a meeting server on this machine
//! (`http://127.0.0.1:port`): the tests and the `pairdrop_pc` example use it.
//! Oyen Poni itself talks HTTPS through Windows (`services::pairdrop`).

use super::meet::Post;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub struct PlainHttp {
    /// `host:port`
    host: String,
}

impl PlainHttp {
    /// `base` like `http://127.0.0.1:8797`.
    pub fn new(base: &str) -> Option<PlainHttp> {
        let host = base.strip_prefix("http://")?.trim_end_matches('/');
        (!host.is_empty() && !host.contains('/')).then(|| PlainHttp { host: host.to_string() })
    }
}

impl Post for PlainHttp {
    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<(u16, Value), String> {
        let addr = self.host.to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
        let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        s.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let json = body.to_string();
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.host,
            json.len()
        );
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(json.as_bytes())).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header end")?;
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let status: u16 = head.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or("no status")?;
        let mut body = raw[split + 4..].to_vec();
        if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
            body = unchunk(&body).ok_or("bad chunked body")?;
        }
        let v = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap_or(Value::Null) };
        Ok((status, v))
    }
}

fn unchunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = b.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&b[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_chunked_bodies() {
        assert_eq!(unchunk(b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n").unwrap(), b"Wikipedia");
        assert!(unchunk(b"zz\r\n").is_none());
    }

    #[test]
    fn takes_only_plain_local_bases() {
        assert!(PlainHttp::new("http://127.0.0.1:8797").is_some());
        assert!(PlainHttp::new("https://drop.oyenza.com").is_none());
        assert!(PlainHttp::new("http://127.0.0.1:8797/x").is_none());
    }
}
