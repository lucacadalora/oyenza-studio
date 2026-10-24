//! Phone drop: a small web page Oyen Poni serves on the local network while
//! the Shelf's phone panel is open. A phone scans the island's QR code, opens
//! the page in its browser, and sends files to the Shelf or saves Shelf files.
//!
//! It is not AirDrop. AirDrop needs Apple's AWDL Wi-Fi link, which Windows
//! Wi-Fi drivers can't run, so iPhones never list a Windows PC. This works
//! with any phone on the same Wi-Fi and needs no app on it.
//!
//! The design follows Windfall's phone page (MIT, github.com/UvejsGj/Windfall,
//! `docs/phone-page.md` on its `phone-page` branch):
//!
//! * a fresh secret link for every session; every route sits under it;
//! * the phone first sends the list of files (`/ask`), the person at the PC
//!   answers on the island, and only then are file bodies read;
//! * the list is binding: each upload must match the size it announced, and
//!   files land under the names the person saw (made safe for Windows);
//! * a transfer arrives whole or not at all (staged, then moved into place).
//!
//! Everything here uses `std` only, so the whole server runs in the tests on
//! any OS. The Windows crate picks the address, makes the secret and the
//! inbox folder, and shows the questions on the island.

pub mod http;
pub mod names;
pub mod qr;
pub mod server;
pub mod token;

pub use qr::QrMatrix;
pub use server::{Ask, AskFile, Config, Event, Server};

/// Short, human label for a phone from its browser's User-Agent.
pub fn device_label(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    if ua.contains("iphone") {
        "iPhone"
    } else if ua.contains("ipad") {
        "iPad"
    } else if ua.contains("android") {
        if ua.contains("mobile") {
            "Android phone"
        } else {
            "Android tablet"
        }
    } else if ua.contains("macintosh") {
        // iPads ask for the desktop site by default and say "Macintosh".
        "iPad or Mac"
    } else if ua.contains("windows") {
        "Windows PC"
    } else {
        "Phone"
    }
}

/// "4.1 MB", "820 KB", "12 bytes": sizes as the island and the page show them.
pub fn fmt_bytes(n: u64) -> String {
    const K: f64 = 1000.0;
    let f = n as f64;
    if n < 1000 {
        format!("{n} {}", if n == 1 { "byte" } else { "bytes" })
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        let mb = f / (K * K);
        if mb < 10.0 {
            format!("{mb:.1} MB")
        } else {
            format!("{mb:.0} MB")
        }
    } else {
        format!("{:.1} GB", f / (K * K * K))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_phones() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 27_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Mobile/15E148 Safari/604.1";
        assert_eq!(device_label(iphone), "iPhone");
        let pixel = "Mozilla/5.0 (Linux; Android 16; Pixel 10) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Mobile Safari/537.36";
        assert_eq!(device_label(pixel), "Android phone");
        assert_eq!(device_label("Mozilla/5.0 (Linux; Android 16; SM-X910) AppleWebKit/537.36 Chrome/141.0 Safari/537.36"), "Android tablet");
        assert_eq!(device_label("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"), "iPad or Mac");
        assert_eq!(device_label("curl/8.9"), "Phone");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(fmt_bytes(0), "0 bytes");
        assert_eq!(fmt_bytes(1), "1 byte");
        assert_eq!(fmt_bytes(999), "999 bytes");
        assert_eq!(fmt_bytes(820_400), "820 KB");
        assert_eq!(fmt_bytes(4_120_000), "4.1 MB");
        assert_eq!(fmt_bytes(84_000_000), "84 MB");
        assert_eq!(fmt_bytes(1_260_000_000), "1.3 GB");
    }
}
