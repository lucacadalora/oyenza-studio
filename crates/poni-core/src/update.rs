//! New versions (docs/updates.md): https://studio.oyenza.com/latest.json says
//! which release is the latest, signed with a key from data/update-keys.txt.
//! The app checks the signature with the system's own crypto
//! (`services::update`); this module reads the file, refuses anything that
//! isn't this repository's own release, and keeps what the app remembers
//! between starts: when it last looked, what it found, what it announced.

use serde::{Deserialize, Serialize};

/// Where every copy looks.
pub const FEED_URL: &str = "https://studio.oyenza.com/latest.json";
/// Downloads only ever come from this repository's releases.
const DOWNLOADS: &str = "https://github.com/lucacadalora/oyenza-studio/releases/download/v";
const RELEASE_PAGE: &str = "https://github.com/lucacadalora/oyenza-studio/releases/tag/v";
/// The file's layout this version reads. A file with a newer one is ignored
/// (a later version of the app will read it).
pub const SCHEMA: u64 = 1;
/// Every hour: releases often come out several a day, and one reaches every
/// copy in use within the hour, for one request of about 1 KB.
pub const CHECK_EVERY_S: u64 = 3600;
/// The first look after start waits this long, out of the way of the start.
pub const FIRST_CHECK_S: u64 = 120;
/// After a look that failed (no network, the site busy).
pub const RETRY_S: u64 = 3600;
/// The headline is one line on the island.
const HEADLINE_MAX: usize = 90;
/// The file is about 1 KB: anything much bigger isn't it.
pub const MAX_FILE_BYTES: u64 = 16 * 1024;

/// The keys this app trusts, built in.
const KEYS: &str = include_str!("../data/update-keys.txt");

/// The public key (an uncompressed P-256 point) with this id, if trusted.
pub fn trusted_key(id: &str) -> Option<[u8; 65]> {
    KEYS.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut parts = l.split_whitespace();
        (parts.next()? == id).then_some(())?;
        let bytes = hex(parts.next()?)?;
        let key: [u8; 65] = bytes.try_into().ok()?;
        (key[0] == 4).then_some(key)
    })
}

/// The file as it arrives, before anything in it is believed: the payload's
/// exact text, its signature (r and s) and the trusted key it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Signed {
    pub payload: String,
    pub signature: [u8; 64],
    pub key: [u8; 65],
}

/// Read the file's outer layer. The caller checks `signature` over the
/// payload's bytes with `key` before calling [`parse_payload`].
pub fn parse_file(text: &str) -> Result<Signed, String> {
    #[derive(Deserialize)]
    struct File {
        payload: String,
        signature: String,
        key: String,
    }
    let f: File = serde_json::from_str(text).map_err(|e| format!("not the update file ({e})"))?;
    let key = trusted_key(&f.key).ok_or_else(|| format!("signed with key {:.16}, which this version doesn't trust", f.key))?;
    let signature: [u8; 64] = base64(&f.signature).and_then(|b| b.try_into().ok()).ok_or("the signature isn't 64 bytes of base64")?;
    Ok(Signed { payload: f.payload, signature, key })
}

/// One download of a release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Download {
    pub url: String,
    pub size: u64,
    /// Lower-case hex.
    pub sha256: String,
}

/// What the file says about the latest release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// The day it came out, "2026-10-25".
    pub date: String,
    /// The first bold line of its notes, for the island.
    pub headline: String,
    /// Its release page.
    pub notes: String,
    pub windows: Option<Download>,
    /// Missing until the DMG is on the release.
    pub mac: Option<Download>,
}

impl Release {
    /// This system's download: the DMG on a Mac, the installer on Windows.
    pub fn download(&self, mac: bool) -> Option<&Download> {
        if mac { self.mac.as_ref() } else { self.windows.as_ref() }
    }
}

impl Download {
    /// Its file's name ("Oyenza-Studio-Setup-0.2.5.exe"): plain letters,
    /// digits and `-_.`, as [`parse_payload`] made sure.
    pub fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or_default()
    }

    /// `bytes` are this file: its size and SHA-256 from the signed file.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && crate::sha256::hex(&crate::sha256::sha256(bytes)) == self.sha256
    }

    /// The version whose release this file is ("0.2.5"), from its address:
    /// [`parse_payload`] made sure it is that release's own.
    pub fn version(&self) -> Option<&str> {
        self.url.strip_prefix(DOWNLOADS)?.split('/').next().filter(|v| parts(v).is_some())
    }
}

/// Read the payload, once its signature checked out. Refuses a newer
/// layout, and downloads or a notes page outside this repository's own
/// release of that version.
pub fn parse_payload(text: &str) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Payload {
        schema: u64,
        version: String,
        #[serde(default)]
        date: String,
        #[serde(default)]
        headline: String,
        notes: String,
        windows: Option<Download>,
        mac: Option<Download>,
    }
    let p: Payload = serde_json::from_str(text).map_err(|e| format!("the payload doesn't read ({e})"))?;
    if p.schema != SCHEMA {
        return Err(format!("layout {} (this version reads {SCHEMA})", p.schema));
    }
    if parts(&p.version).is_none() {
        return Err(format!("\"{:.20}\" isn't a version", p.version));
    }
    if p.notes != format!("{RELEASE_PAGE}{}", p.version) {
        return Err("its notes aren't this repository's release page".into());
    }
    let prefix = format!("{DOWNLOADS}{}/", p.version);
    for d in p.windows.iter().chain(&p.mac) {
        let name = d.url.strip_prefix(&prefix).unwrap_or_default();
        let plain = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if !plain || d.size == 0 || d.sha256.len() != 64 || !d.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(format!("a download isn't one of v{}'s own files", p.version));
        }
    }
    let date = if p.date.len() == 10 && p.date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }) { p.date } else { String::new() };
    Ok(Release { headline: headline(&p.headline, &p.version), version: p.version, date, notes: p.notes, windows: p.windows, mac: p.mac })
}

/// The headline as the island shows it: one line, control characters out,
/// at most HEADLINE_MAX characters.
fn headline(text: &str, version: &str) -> String {
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut line = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        line = format!("Oyenza Studio {version}");
    }
    if line.chars().count() > HEADLINE_MAX {
        line = line.chars().take(HEADLINE_MAX - 1).collect::<String>().trim_end().to_string() + "…";
    }
    line
}

/// "0.2.10" as numbers; None if it isn't 2 to 4 dot-separated numbers.
fn parts(v: &str) -> Option<Vec<u64>> {
    let p: Vec<u64> = v.split('.').map(|s| if !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit()) { s.parse().ok() } else { None }).collect::<Option<_>>()?;
    (2..=4).contains(&p.len()).then_some(p)
}

/// Is version `a` later than `b`? ("0.2.10" is later than "0.2.9".) A
/// string that isn't a version is never later.
pub fn newer(a: &str, b: &str) -> bool {
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

/// What the app remembers between starts (`update.json` in its data folder).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    /// When the file was last read (Unix seconds), whatever it said.
    pub checked: u64,
    /// The latest release it described (signature checked).
    pub latest: Option<Release>,
    /// The version the island has announced: once per version.
    pub announced: String,
    /// The version that ran before this one.
    pub ran: String,
    /// Install on next start (Windows): the version whose Setup runs before
    /// the app opens next time. Cleared as it runs, so a Setup that fails
    /// doesn't keep the app from opening.
    pub install_on_start: String,
}

impl Memory {
    /// When to read the file next (Unix seconds): a couple of minutes after
    /// this run began (`started`), but not before an hour has passed since
    /// the last look, so starting the app often costs nothing.
    pub fn next_check(&self, started: u64) -> u64 {
        (started + FIRST_CHECK_S).max(self.checked.saturating_add(CHECK_EVERY_S))
    }

    /// The release to offer: a later one than `running`, with a download for
    /// this system.
    pub fn offer(&self, running: &str, mac: bool) -> Option<&Release> {
        self.latest.as_ref().filter(|r| newer(&r.version, running) && r.download(mac).is_some())
    }

    /// Should the island tell about this offer? Once per version.
    pub fn should_announce(&self, running: &str, mac: bool) -> bool {
        self.offer(running, mac).is_some_and(|r| r.version != self.announced)
    }

    /// The version running now is later than the one that ran before: the
    /// island says "Updated to" once. A first run (nothing remembered) doesn't.
    pub fn updated(&self, running: &str) -> bool {
        !self.ran.is_empty() && newer(running, &self.ran)
    }

    /// The release Install on next start asked for, while it is still the
    /// latest and later than `running`.
    pub fn pending_install(&self, running: &str) -> Option<&Release> {
        self.latest.as_ref().filter(|r| !self.install_on_start.is_empty() && r.version == self.install_on_start && newer(&r.version, running))
    }

    /// What the file just said, read at `now` (Unix seconds).
    pub fn read(&mut self, release: Release, now: u64) {
        self.checked = now;
        self.latest = Some(release);
    }
}

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Standard base64 (with or without padding).
pub(crate) fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "0.2.3";
    const SHA: &str = "8f30919c3c13e7074ae0381d9b478e1ef5fb802476b637f5febefe949d26e26b";

    fn payload(version: &str, extra: &str) -> String {
        format!(
            r#"{{"schema":1,"version":"{version}","date":"2026-10-25","headline":"Sing along from the closed island","notes":"https://github.com/lucacadalora/oyenza-studio/releases/tag/v{version}","windows":{{"url":"https://github.com/lucacadalora/oyenza-studio/releases/download/v{version}/Oyenza-Studio-Setup-{version}.exe","size":2457657,"sha256":"{SHA}"}}{extra}}}"#
        )
    }

    #[test]
    fn the_built_in_keys_are_trusted_and_others_are_not() {
        let k = trusted_key("4cb3280eaea65b50").expect("signing key");
        assert_eq!(k[0], 4);
        assert!(trusted_key("f835f77c60bd2b7f").is_some(), "spare key");
        assert_eq!(trusted_key("0000000000000000"), None);
        assert_eq!(trusted_key(""), None);
    }

    #[test]
    fn the_file_reads_its_signature_and_key() {
        let sig = "A".repeat(85) + "Q==";
        let file = format!(r#"{{"about":"…","payload":"{{}}","signature":"{sig}","key":"4cb3280eaea65b50"}}"#);
        let s = parse_file(&file).unwrap();
        assert_eq!(s.payload, "{}");
        assert_eq!(s.signature.len(), 64);
        assert!(parse_file(&file.replace("4cb3280eaea65b50", "deadbeefdeadbeef")).unwrap_err().contains("doesn't trust"));
        assert!(parse_file(&file.replace(&sig, "AAAA")).is_err());
        assert!(parse_file("<html>").is_err());
    }

    #[test]
    fn a_release_reads_with_its_downloads() {
        let r = parse_payload(&payload(V, "")).unwrap();
        assert_eq!((r.version.as_str(), r.date.as_str(), r.headline.as_str()), (V, "2026-10-25", "Sing along from the closed island"));
        assert_eq!(r.download(false).map(|d| d.size), Some(2457657));
        assert_eq!(r.download(true), None, "no DMG yet");
        let mac = r#","mac":{"url":"https://github.com/lucacadalora/oyenza-studio/releases/download/v0.2.3/Oyenza-Studio-0.2.3-mac.dmg","size":3667341,"sha256":"fabc80efc387be83d7f82702bede7c3d2470f7aa6ef62cc2a8cd095a425f3fb9"}"#;
        let r = parse_payload(&payload(V, mac)).unwrap();
        assert_eq!(r.download(true).map(|d| d.size), Some(3667341));
        // Each download knows its release's version (a Mac checks the app in the DMG against it).
        assert_eq!(r.download(true).and_then(|d| d.version()), Some(V));
        assert_eq!(r.download(false).and_then(|d| d.version()), Some(V));
        let elsewhere = Download { url: "https://example.com/v0.2.3/x.dmg".into(), size: 1, sha256: SHA.into() };
        assert_eq!(elsewhere.version(), None);
    }

    #[test]
    fn a_payload_pointing_elsewhere_is_refused() {
        let p = payload(V, "");
        // Downloads from another place, another version's folder, or a path that climbs out.
        assert!(parse_payload(&p.replace("https://github.com/lucacadalora/oyenza-studio/releases/download", "https://example.com/x")).is_err());
        assert!(parse_payload(&p.replace("download/v0.2.3/", "download/v0.2.2/")).is_err());
        assert!(parse_payload(&p.replace("Oyenza-Studio-Setup-0.2.3.exe", "../../x/evil.exe")).is_err());
        // Notes elsewhere, a bad hash, a newer layout, not a version.
        assert!(parse_payload(&p.replace("releases/tag/v0.2.3", "releases/tag/v9.9.9")).is_err());
        assert!(parse_payload(&p.replace(SHA, &SHA.to_uppercase())).is_err());
        assert!(parse_payload(&p.replace("\"schema\":1", "\"schema\":2")).unwrap_err().contains("layout 2"));
        assert!(parse_payload(&payload("latest", "")).is_err());
    }

    #[test]
    fn the_headline_is_one_short_line() {
        let long = "word ".repeat(40);
        let r = parse_payload(&payload(V, "").replace("Sing along from the closed island", &format!("{long}\\nmore"))).unwrap();
        assert!(r.headline.chars().count() <= HEADLINE_MAX && r.headline.ends_with('…') && !r.headline.contains('\n'), "{}", r.headline);
        let r = parse_payload(&payload(V, "").replace("Sing along from the closed island", "")).unwrap();
        assert_eq!(r.headline, "Oyenza Studio 0.2.3");
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("0.3.0", "0.2.99"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.2.3", "0.2.3"));
        assert!(!newer("0.2.2", "0.2.3"));
        assert!(!newer("latest", "0.2.3"));
        assert!(newer("0.2.3", ""));
    }

    #[test]
    fn checks_wait_for_an_hour_but_start_soon_after_a_long_break() {
        let start = 1_000_000;
        let m = Memory::default();
        assert_eq!(m.next_check(start), start + FIRST_CHECK_S, "never looked: two minutes after start");
        let m = Memory { checked: start - 600, ..Memory::default() };
        assert_eq!(m.next_check(start), start - 600 + CHECK_EVERY_S, "looked ten minutes ago: 50 minutes to go");
        let m = Memory { checked: start - 2 * CHECK_EVERY_S, ..Memory::default() };
        assert_eq!(m.next_check(start), start + FIRST_CHECK_S);
    }

    #[test]
    fn an_offer_is_announced_once_and_only_when_it_is_later() {
        let mut m = Memory::default();
        m.read(parse_payload(&payload("0.2.5", "")).unwrap(), 5);
        assert_eq!(m.checked, 5);
        assert_eq!(m.offer("0.2.4", false).map(|r| r.version.as_str()), Some("0.2.5"));
        assert!(m.offer("0.2.5", false).is_none(), "already on it");
        assert!(m.offer("0.2.4", true).is_none(), "no DMG for a Mac yet");
        assert!(m.should_announce("0.2.4", false));
        m.announced = "0.2.5".into();
        assert!(!m.should_announce("0.2.4", false));
        assert!(m.offer("0.2.4", false).is_some(), "still offered in the menu and Settings");
    }

    #[test]
    fn updated_is_said_after_a_newer_version_starts() {
        let mut m = Memory::default();
        assert!(!m.updated("0.2.5"), "first run");
        m.ran = "0.2.4".into();
        assert!(m.updated("0.2.5"));
        assert!(!m.updated("0.2.4"));
        assert!(!m.updated("0.2.3"), "an older copy started again");
    }

    #[test]
    fn install_on_next_start_only_for_the_latest_later_version() {
        let mut m = Memory::default();
        m.read(parse_payload(&payload("0.2.5", "")).unwrap(), 5);
        assert!(m.pending_install("0.2.4").is_none(), "not asked for");
        m.install_on_start = "0.2.5".into();
        assert_eq!(m.pending_install("0.2.4").map(|r| r.version.as_str()), Some("0.2.5"));
        assert!(m.pending_install("0.2.5").is_none(), "already on it");
        m.read(parse_payload(&payload("0.2.6", "")).unwrap(), 6);
        assert!(m.pending_install("0.2.4").is_none(), "a later one came out meanwhile: it asks again");
    }

    #[test]
    fn a_download_knows_its_file() {
        let r = parse_payload(&payload(V, "")).unwrap();
        let d = r.download(false).unwrap();
        assert_eq!(d.file_name(), "Oyenza-Studio-Setup-0.2.3.exe");
        let bytes = b"not the installer";
        let real = Download { url: d.url.clone(), size: bytes.len() as u64, sha256: crate::sha256::hex(&crate::sha256::sha256(bytes)) };
        assert!(real.matches(bytes));
        assert!(!real.matches(b"not the installeR"), "one byte changed");
        assert!(!d.matches(bytes), "another size and hash");
    }

    #[test]
    fn memory_survives_a_save() {
        let mut m = Memory { announced: "0.2.5".into(), ran: "0.2.4".into(), install_on_start: "0.2.5".into(), ..Memory::default() };
        m.read(parse_payload(&payload("0.2.5", "")).unwrap(), 42);
        let back: Memory = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(serde_json::from_str::<Memory>("{}").unwrap(), Memory::default());
    }

    #[test]
    fn base64_reads_the_signature() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64("aGVs bG8="), None);
    }
}
