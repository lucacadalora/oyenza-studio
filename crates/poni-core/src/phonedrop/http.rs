//! Just enough HTTP/1.1 for the phone page: one request per connection,
//! bodies sized by `Content-Length` (no chunked uploads), and responses that
//! always close the connection.

use std::io::{self, Read, Write};

/// Largest request head we accept (request line plus headers).
pub const MAX_HEAD: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    /// The path without its query string, e.g. `/TOKEN/info`.
    pub path: String,
    /// Header names in lower case, values trimmed.
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `Content-Length`, or 0 when absent. `Err` for a malformed value.
    pub fn content_length(&self) -> Result<u64, ()> {
        match self.header("content-length") {
            None => Ok(0),
            Some(v) => v.parse::<u64>().map_err(|_| ()),
        }
    }

    pub fn chunked(&self) -> bool {
        self.header("transfer-encoding").is_some_and(|v| !v.eq_ignore_ascii_case("identity"))
    }

    pub fn expects_continue(&self) -> bool {
        self.header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
    }

    /// The path split on `/`, without empty parts.
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HeadError {
    /// The peer closed the connection before sending a full head.
    Closed,
    TooLarge,
    Malformed,
}

/// Parse a request head (everything before the blank line).
pub fn parse_head(raw: &[u8]) -> Result<Head, HeadError> {
    let text = std::str::from_utf8(raw).map_err(|_| HeadError::Malformed)?;
    let mut lines = text.split("\r\n");
    let request = lines.next().ok_or(HeadError::Malformed)?;
    let mut parts = request.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(HeadError::Malformed);
    };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) || !target.starts_with('/') {
        return Err(HeadError::Malformed);
    }
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or(HeadError::Malformed)?;
        if k.is_empty() || k.contains(' ') {
            return Err(HeadError::Malformed);
        }
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    Ok(Head { method: method.to_string(), path, headers })
}

/// Read a request head from `r`. Returns the head and any body bytes that
/// arrived in the same reads.
pub fn read_head(r: &mut impl Read) -> Result<(Head, Vec<u8>), HeadError> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = find_blank_line(&buf) {
            let head = parse_head(&buf[..end])?;
            let rest = buf[end + 4..].to_vec();
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(HeadError::TooLarge);
        }
        let n = match r.read(&mut chunk) {
            Ok(0) => return Err(HeadError::Closed),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(HeadError::Closed),
        };
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_blank_line(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Read exactly `len` body bytes (small bodies only: the caller caps `len`).
pub fn read_body(r: &mut impl Read, mut already: Vec<u8>, len: usize) -> io::Result<Vec<u8>> {
    already.truncate(len);
    let mut rest = vec![0u8; len - already.len()];
    r.read_exact(&mut rest)?;
    already.extend_from_slice(&rest);
    Ok(already)
}

pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Headers every response carries: nothing is cached (the link is secret and
/// short-lived), nothing is sniffed, and the secret never leaves in a Referer.
const COMMON: &str = "Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n";

/// Write a response head. `extra` holds additional header lines, each ending in CRLF.
pub fn write_head(w: &mut impl Write, status: u16, content_type: &str, len: u64, extra: &str) -> io::Result<()> {
    write!(w, "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\n{COMMON}{extra}\r\n", reason(status))
}

pub fn respond(w: &mut impl Write, status: u16, content_type: &str, body: &[u8], extra: &str) -> io::Result<()> {
    write_head(w, status, content_type, body.len() as u64, extra)?;
    w.write_all(body)?;
    w.flush()
}

pub fn respond_json(w: &mut impl Write, status: u16, body: &serde_json::Value) -> io::Result<()> {
    respond(w, status, "application/json; charset=utf-8", body.to_string().as_bytes(), "")
}

/// A bare error status with a one-word text body.
pub fn respond_status(w: &mut impl Write, status: u16) -> io::Result<()> {
    respond(w, status, "text/plain; charset=utf-8", reason(status).as_bytes(), "")
}

/// `Content-Disposition` for a download: a plain ASCII fallback plus the
/// exact UTF-8 name (RFC 6266 / 8187).
pub fn attachment(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("Content-Disposition: attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}\r\n")
}

/// Content type for a file we hand to the phone, from its extension.
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_head() {
        let raw = b"PUT /TOKEN/up/3/0?x=1 HTTP/1.1\r\nHost: 192.168.1.20:53530\r\nContent-Length: 12\r\nUser-Agent: Phone\r\n";
        let h = parse_head(raw).unwrap();
        assert_eq!(h.method, "PUT");
        assert_eq!(h.path, "/TOKEN/up/3/0");
        assert_eq!(h.segments(), vec!["TOKEN", "up", "3", "0"]);
        assert_eq!(h.content_length(), Ok(12));
        assert_eq!(h.header("user-agent"), Some("Phone"));
        assert!(!h.chunked());
    }

    #[test]
    fn rejects_bad_heads() {
        assert_eq!(parse_head(b"GET / HTTP/1.1 extra\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"get / HTTP/1.1\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET http://x/ HTTP/1.1\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET / SPDY/3\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET / HTTP/1.1\r\nno colon here\r\n"), Err(HeadError::Malformed));
        let h = parse_head(b"POST / HTTP/1.1\r\nContent-Length: lots\r\n").unwrap();
        assert!(h.content_length().is_err());
    }

    #[test]
    fn reads_head_and_keeps_body_bytes() {
        let mut data: &[u8] = b"POST /a HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let (h, rest) = read_head(&mut data).unwrap();
        assert_eq!(h.path, "/a");
        assert_eq!(rest, b"hello");
        let mut empty: &[u8] = b"";
        assert_eq!(read_body(&mut empty, rest, 5).unwrap(), b"hello");
        let mut cut: &[u8] = b"GET / HTTP/1.1\r\nHost: x";
        assert_eq!(read_head(&mut cut).unwrap_err(), HeadError::Closed);
    }

    #[test]
    fn caps_the_head() {
        let mut big = b"GET / HTTP/1.1\r\n".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_HEAD + 10));
        let mut r: &[u8] = &big;
        assert_eq!(read_head(&mut r).unwrap_err(), HeadError::TooLarge);
    }

    #[test]
    fn download_names() {
        assert_eq!(
            attachment("Résumé \"final\".pdf"),
            "Content-Disposition: attachment; filename=\"R_sum_ _final_.pdf\"; filename*=UTF-8''R%C3%A9sum%C3%A9%20%22final%22.pdf\r\n"
        );
        assert_eq!(content_type_for("IMG_2041.HEIC"), "image/heic");
        assert_eq!(content_type_for("notes"), "application/octet-stream");
    }

    #[test]
    fn writes_responses() {
        let mut out = Vec::new();
        respond(&mut out, 404, "text/plain", b"Not Found", "").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n"));
        assert!(s.contains("Cache-Control: no-store\r\n"));
        assert!(s.ends_with("\r\n\r\nNot Found"));
    }
}
