//! The QR code the island shows for the phone link (qrcodegen, MIT).

use qrcodegen::{QrCode, QrCodeEcc};

/// A QR code as a square grid of dark/light modules, without the quiet zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    pub size: usize,
    /// Row-major, `true` = dark.
    pub dark: Vec<bool>,
}

impl QrMatrix {
    /// Encode `text` with medium error correction (a screen reflection or a
    /// smudge on the lens still scans). `None` if it doesn't fit a QR code.
    pub fn encode(text: &str) -> Option<QrMatrix> {
        let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
        let size = qr.size() as usize;
        let mut dark = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                dark.push(qr.get_module(x as i32, y as i32));
            }
        }
        Some(QrMatrix { size, dark })
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_case_links_stay_small() {
        // The shape of a real link: address, port and a 26-character secret.
        let url = "HTTP://192.168.100.200:53530/K7Q2M9XWZ4R8T6YB3NPV5HJC2D/";
        let qr = QrMatrix::encode(url).unwrap();
        // Alphanumeric mode fits this in version 3 (29 modules); byte mode
        // (a lower-case link) would need version 4 (33).
        assert_eq!(qr.size, 29);
        let lower = QrMatrix::encode(&url.to_ascii_lowercase()).unwrap();
        assert!(lower.size > qr.size);
        // Finder pattern corners are dark, the module next to them light.
        assert!(qr.get(0, 0) && qr.get(6, 0) && qr.get(0, 6) && !qr.get(1, 1));
        assert!(!qr.get(99, 0));
    }
}
