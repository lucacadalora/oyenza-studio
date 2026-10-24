//! File names from a phone, made safe to create on Windows.
//!
//! The name the person saw on the island is the name the file gets, minus
//! anything Windows can't store or that could point somewhere else: path
//! parts, control and direction-override characters, reserved device names.

use std::path::{Path, PathBuf};

/// Longest name we create, in characters (the extension is kept).
pub const MAX_NAME_CHARS: usize = 160;

/// A single safe file name for `raw`. Never empty, never a path.
pub fn safe_name(raw: &str) -> String {
    // Only the last path part: phones send bare names, but never trust that.
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = last
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 || c == '\u{7f}' => '_',
            // Bidirectional overrides and isolates can disguise an extension.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' => '_',
            c => c,
        })
        .collect();
    // Windows drops trailing dots and spaces; leading dots hide files elsewhere.
    s = s.trim_end_matches(['.', ' ']).trim_start_matches(['.', ' ']).to_string();
    if s.is_empty() {
        s = "file".into();
    }
    if is_reserved(&s) {
        s.insert(0, '_');
    }
    shorten(&s, MAX_NAME_CHARS)
}

/// CON, PRN, AUX, NUL, COM1-9, LPT1-9 (with or without an extension) name
/// devices on Windows, not files.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let b = stem.as_bytes();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (b.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && b[3].is_ascii_digit())
}

/// Cut a name to `max` characters, keeping a short extension.
fn shorten(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let ext_len = ext.chars().count();
    if ext_len > 0 && ext_len < max / 2 {
        let keep = max - ext_len;
        let stem: String = stem.chars().take(keep).collect();
        format!("{}{ext}", stem.trim_end_matches(['.', ' ']))
    } else {
        name.chars().take(max).collect::<String>().trim_end_matches(['.', ' ']).to_string()
    }
}

/// ("photo", ".jpg"); ("archive.tar", ".gz"); ("README", "").
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i <= 12 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A path in `dir` for `name` that doesn't exist yet: `photo.jpg`, then
/// `photo (1).jpg`, `photo (2).jpg`, … `taken` also counts names already
/// chosen in this batch (they don't exist on disk yet).
pub fn unique_path(dir: &Path, name: &str, taken: &[PathBuf]) -> PathBuf {
    let free = |p: &PathBuf| !p.exists() && !taken.iter().any(|t| same_name(t, p));
    let first = dir.join(name);
    if free(&first) {
        return first;
    }
    let (stem, ext) = split_ext(name);
    for n in 1..10_000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if free(&p) {
            return p;
        }
    }
    dir.join(format!("{stem} ({}){ext}", std::process::id()))
}

/// Windows file names are case-insensitive.
fn same_name(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ordinary_names() {
        assert_eq!(safe_name("IMG_2041.HEIC"), "IMG_2041.HEIC");
        assert_eq!(safe_name("Résumé 2026.pdf"), "Résumé 2026.pdf");
        assert_eq!(safe_name("照片.jpg"), "照片.jpg");
    }

    #[test]
    fn strips_paths_and_bad_characters() {
        assert_eq!(safe_name("../../Windows/system.ini"), "system.ini");
        assert_eq!(safe_name("C:\\Users\\a\\b.txt"), "b.txt");
        assert_eq!(safe_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_name("tab\there.txt"), "tab_here.txt");
        // "photo<RLO>gpj.exe" displays as "photoexe.jpg" in some views.
        assert_eq!(safe_name("photo\u{202e}gpj.exe"), "photo_gpj.exe");
        assert_eq!(safe_name("..."), "file");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name("  .hidden "), "hidden");
        assert_eq!(safe_name("trailing. . "), "trailing");
    }

    #[test]
    fn avoids_device_names() {
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("nul.txt"), "_nul.txt");
        assert_eq!(safe_name("com1.log"), "_com1.log");
        assert_eq!(safe_name("LPT9"), "_LPT9");
        assert_eq!(safe_name("COM10.txt"), "COM10.txt");
        assert_eq!(safe_name("console.txt"), "console.txt");
    }

    #[test]
    fn shortens_long_names_keeping_the_extension() {
        let long = format!("{}.jpg", "a".repeat(400));
        let s = safe_name(&long);
        assert_eq!(s.chars().count(), MAX_NAME_CHARS);
        assert!(s.ends_with("a.jpg"));
    }

    #[test]
    fn splits_extensions() {
        assert_eq!(split_ext("photo.jpg"), ("photo", ".jpg"));
        assert_eq!(split_ext("archive.tar.gz"), ("archive.tar", ".gz"));
        assert_eq!(split_ext("README"), ("README", ""));
        assert_eq!(split_ext(".env"), (".env", ""));
        assert_eq!(split_ext("dot."), ("dot.", ""));
    }

    #[test]
    fn numbers_duplicates() {
        let dir = std::env::temp_dir().join(format!("poni-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("photo.jpg"), b"x").unwrap();
        let a = unique_path(&dir, "photo.jpg", &[]);
        assert_eq!(a, dir.join("photo (1).jpg"));
        let b = unique_path(&dir, "PHOTO.jpg", &[a.clone()]);
        // Case-insensitive like Windows: "PHOTO.jpg" collides with photo.jpg on
        // Windows; on a case-sensitive disk only the batch check catches it.
        assert!(b == dir.join("PHOTO.jpg") || b == dir.join("PHOTO (1).jpg") || b == dir.join("PHOTO (2).jpg"));
        assert_ne!(b, a);
        assert_eq!(unique_path(&dir, "new.txt", &[]), dir.join("new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
