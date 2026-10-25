//! Add-ons: features that download only for the people who switch them on.
//!
//! An add-on's code is part of the app and stays asleep while it is off. The
//! switch downloads the add-on's files (data and sounds) from a pre-release
//! on the app's GitHub repository, and switching it off deletes them. Each
//! file's size and SHA-256 are built into the app, so a changed or broken
//! file is refused; new files come with a new app release (a new `tag`).
//! The apps' `services::addons` (one file, the Windows app's, which the Mac
//! app runs too) does the downloading.

use crate::phonedrop::fmt_bytes;
use crate::settings::Settings;
use crate::sha256;

/// Where the files live: `<RELEASES>/<tag>/<name>`.
pub const RELEASES: &str = "https://github.com/lucacadalora/oyenza-studio/releases/download";

/// One file an add-on downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddonFile {
    pub name: &'static str,
    pub size: u64,
    /// SHA-256, lowercase hex.
    pub sha256: &'static str,
}

impl AddonFile {
    /// True when `bytes` are exactly this file.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && sha256::hex(&sha256::sha256(bytes)) == self.sha256
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addon {
    /// Stable id: the key in settings.json and the folder's name.
    pub id: &'static str,
    pub title: &'static str,
    /// What it does, in one line, for the Add-ons page.
    pub blurb: &'static str,
    /// The same in the Mac app, where `blurb` gives Windows' reason (None:
    /// `blurb` there too).
    pub mac_blurb: Option<&'static str>,
    /// The release that holds this version of the files.
    pub tag: &'static str,
    pub files: &'static [AddonFile],
}

impl Addon {
    /// Every file together, in bytes.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn url(&self, file: &AddonFile) -> String {
        format!("{RELEASES}/{}/{}", self.tag, file.name)
    }

    pub fn find(id: &str) -> Option<&'static Addon> {
        ALL.iter().copied().find(|a| a.id == id)
    }
}

/// Prayer times: the place list and the adzan (`addons/sholat/` in the repo).
pub const PRAYER_TIMES: Addon = Addon {
    id: "sholat",
    title: "Prayer times",
    blurb: "Today's times for your city and a reminder in the island.",
    mac_blurb: None,
    tag: "addon-sholat-1",
    files: &[
        AddonFile { name: "places.json", size: 79_687, sha256: "4d2e1cd6c277b97273f415cf0460579db143fe92ac88f6e4bddb79cdf8cbf69b" },
        AddonFile { name: "adzan.m4a", size: 1_822_513, sha256: "bd5a9db9d23eb2fb7e3368c33afeb08603b71b762549ba8247491c5aac24cb59" },
        AddonFile { name: "NOTICE.txt", size: 869, sha256: "a1762a46d2ff3c7456070f76fe370e02e169f30ee749fea7763568034401185a" },
    ],
};

/// The Japanese readings add-on's dictionary file (`romanize::JaDict`).
pub const JA_READINGS_FILE: &str = "ja-readings.dic.zst";

/// Japanese readings: Oyenza Studio's own reading dictionary, for romaji under
/// kanji on Macs and on PCs where Windows has no Japanese reading data (most
/// PCs outside Japan). Only people who play Japanese songs need it: the first
/// song whose kanji go unread switches it on ([`ja_switches_on`], once;
/// switched off by hand it stays off, and the Music page offers it under the
/// lyrics). The file is
/// `crates/poni-core/data/ja-readings.dic.zst` (tools/make_ja_dict.py), its
/// notice `addons/ja/NOTICE.txt`.
pub const JAPANESE_READINGS: Addon = Addon {
    id: "ja",
    title: "Japanese readings",
    blurb: "Romaji under kanji in Japanese lyrics, where Windows can't read them.",
    mac_blurb: Some("Romaji under kanji in Japanese lyrics."),
    tag: "addon-ja-1",
    files: &[
        AddonFile { name: JA_READINGS_FILE, size: 3_790_036, sha256: "4950e5e42a2a48ad50396304c6bfbb8f9a97b33947e147007943361d65d856c8" },
        AddonFile { name: "NOTICE.txt", size: 4_786, sha256: "fb8cd0420c2a8b04404c17a1a2ada30e0e1bbd73e54ef548bb60ede42198e079" },
    ],
};

/// Every add-on, in the order the Add-ons page lists them.
pub const ALL: [&Addon; 2] = [&PRAYER_TIMES, &JAPANESE_READINGS];

/// Whether Japanese readings switch themselves on for a song whose kanji went
/// unread for want of them (`wants`): only while the person reads
/// pronunciations, and only if they never switched the add-on on or off
/// themselves (one switched off by hand stays off).
pub fn ja_switches_on(settings: &Settings, wants: bool) -> bool {
    wants && settings.lyrics_pronunciation && settings.addon_unset(JAPANESE_READINGS.id)
}

/// What the Music page says under a Japanese song whose kanji went unread for
/// want of Japanese readings, by where that add-on stands, and whether a
/// click gets it (switches it on). None once it's in place.
pub fn ja_offer(status: Status) -> Option<(String, bool)> {
    match status {
        Status::Off => Some((format!("Romaji for kanji: get Japanese readings ({})", fmt_bytes(JAPANESE_READINGS.size())), true)),
        Status::Checking => Some(("Checking Japanese readings\u{2026}".to_string(), false)),
        Status::Downloading { done, total } => Some((format!("Getting Japanese readings \u{b7} {} of {}", fmt_bytes(done), fmt_bytes(total)), false)),
        Status::Waiting => Some(("Japanese readings download once you're online".to_string(), false)),
        Status::Failed => Some(("Japanese readings didn't download: see Add-ons".to_string(), false)),
        Status::Ready => None,
    }
}

/// Where an add-on stands, as the Add-ons page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Switched off: none of its files on this PC, nothing running.
    Off,
    /// Switched on; looking at the files already on this PC.
    Checking,
    /// Bytes so far of the add-on's total.
    Downloading { done: u64, total: u64 },
    /// Switched on with no internet: the download starts by itself later.
    Waiting,
    /// Switched on, every file in place.
    Ready,
    /// A file came back different from the one the app expects, so it was
    /// thrown away. Trying again is the person's call (off, then on).
    Failed,
}

impl Status {
    /// The switch shows on for everything but `Off`.
    pub fn is_on(self) -> bool {
        self != Status::Off
    }

    /// The line under the add-on's name; `mac`: in the Mac app.
    pub fn line(self, addon: &Addon, mac: bool) -> String {
        let here = if mac { "this Mac" } else { "this PC" };
        match self {
            Status::Off => addon.mac_blurb.filter(|_| mac).unwrap_or(addon.blurb).to_string(),
            Status::Checking => format!("Checking its files on {here}"),
            Status::Downloading { done, total } => format!("Downloading · {} of {}", fmt_bytes(done), fmt_bytes(total)),
            Status::Waiting => "No internet. It downloads by itself once you're back online.".to_string(),
            Status::Ready => format!("On · {} on {here}", fmt_bytes(addon.size())),
            Status::Failed => "The download didn't come through right. Switch it off and on to try again.".to_string(),
        }
    }

    /// Download progress 0..1, while downloading.
    pub fn progress(self) -> Option<f32> {
        match self {
            Status::Downloading { done, total } if total > 0 => Some((done as f64 / total as f64).clamp(0.0, 1.0) as f32),
            Status::Downloading { .. } => Some(0.0),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_readings_switch_on_once_for_a_song_that_needs_them() {
        let mut s = Settings::default();
        assert!(ja_switches_on(&s, true), "the first Japanese song with unread kanji");
        assert!(!ja_switches_on(&s, false), "a song that reads without them");
        s.lyrics_pronunciation = false;
        assert!(!ja_switches_on(&s, true), "pronunciations off: nothing to read them for");
        s.lyrics_pronunciation = true;
        s.set_addon(JAPANESE_READINGS.id, true);
        assert!(!ja_switches_on(&s, true), "already on");
        s.set_addon(JAPANESE_READINGS.id, false);
        assert!(!ja_switches_on(&s, true), "switched off by hand: stays off");
        // Another add-on's switch doesn't count as an answer.
        let mut s = Settings::default();
        s.set_addon(PRAYER_TIMES.id, false);
        assert!(ja_switches_on(&s, true));
    }

    #[test]
    fn prayer_times_files_are_the_ones_in_the_repo() {
        // The pinned fingerprints must match addons/sholat/, which is what the release holds.
        let repo: [(&str, &[u8]); 3] = [
            ("places.json", include_bytes!("../../../addons/sholat/places.json")),
            ("adzan.m4a", include_bytes!("../../../addons/sholat/adzan.m4a")),
            ("NOTICE.txt", include_bytes!("../../../addons/sholat/NOTICE.txt")),
        ];
        assert_eq!(PRAYER_TIMES.files.len(), repo.len());
        for (f, (name, bytes)) in PRAYER_TIMES.files.iter().zip(repo) {
            assert_eq!(f.name, name);
            assert!(f.matches(bytes), "{name}: size {} sha256 {}", bytes.len(), sha256::hex(&sha256::sha256(bytes)));
        }
        let mut broken = repo[2].1.to_vec();
        broken[0] ^= 1;
        assert!(!PRAYER_TIMES.files[2].matches(&broken));
        assert!(!PRAYER_TIMES.files[2].matches(&repo[2].1[1..]));
    }

    #[test]
    fn japanese_readings_files_are_the_ones_in_the_repo() {
        // The dictionary make_ja_dict.py builds, and the notice that travels with it.
        let repo: [(&str, &[u8]); 2] = [
            (JA_READINGS_FILE, include_bytes!("../data/ja-readings.dic.zst")),
            ("NOTICE.txt", include_bytes!("../../../addons/ja/NOTICE.txt")),
        ];
        assert_eq!(JAPANESE_READINGS.files.len(), repo.len());
        for (f, (name, bytes)) in JAPANESE_READINGS.files.iter().zip(repo) {
            assert_eq!(f.name, name);
            assert!(f.matches(bytes), "{name}: size {} sha256 {}", bytes.len(), sha256::hex(&sha256::sha256(bytes)));
        }
        let ja = Addon::find("ja").unwrap();
        assert_eq!(ja.url(&ja.files[0]), "https://github.com/lucacadalora/oyenza-studio/releases/download/addon-ja-1/ja-readings.dic.zst");
        assert_eq!(Status::Off.line(ja, false), ja.blurb);
        assert_eq!(Status::Ready.line(ja, false), "On · 3.8 MB on this PC");
        // The Mac app has no Windows reason to give, and says Mac.
        assert_eq!(Status::Off.line(ja, true), "Romaji under kanji in Japanese lyrics.");
        assert_eq!(Status::Checking.line(ja, true), "Checking its files on this Mac");
        assert_eq!(Status::Ready.line(ja, true), "On · 3.8 MB on this Mac");
    }

    #[test]
    fn the_music_page_offers_japanese_readings_until_they_are_here() {
        assert_eq!(ja_offer(Status::Off), Some(("Romaji for kanji: get Japanese readings (3.8 MB)".to_string(), true)));
        let (line, click) = ja_offer(Status::Downloading { done: 1_700_000, total: JAPANESE_READINGS.size() }).unwrap();
        assert_eq!(line, "Getting Japanese readings · 1.7 MB of 3.8 MB");
        assert!(!click);
        for s in [Status::Checking, Status::Waiting, Status::Failed] {
            assert!(ja_offer(s).is_some_and(|(_, click)| !click), "{s:?} says where it stands, nothing to click");
        }
        assert_eq!(ja_offer(Status::Ready), None);
    }

    #[test]
    fn files_come_from_the_release() {
        let a = Addon::find("sholat").unwrap();
        assert_eq!(a.url(&a.files[1]), "https://github.com/lucacadalora/oyenza-studio/releases/download/addon-sholat-1/adzan.m4a");
        assert_eq!(a.size(), 1_903_069);
        assert!(Addon::find("nope").is_none());
    }

    #[test]
    fn lines_say_what_is_happening() {
        let a = &PRAYER_TIMES;
        assert_eq!(Status::Off.line(a, false), a.blurb);
        assert_eq!(Status::Off.line(a, true), a.blurb, "no Mac line of its own: the same");
        assert_eq!(Status::Downloading { done: 1_300_000, total: a.size() }.line(a, false), "Downloading · 1.3 MB of 1.9 MB");
        assert_eq!(Status::Ready.line(a, false), "On · 1.9 MB on this PC");
        assert!(!Status::Off.is_on() && Status::Waiting.is_on() && Status::Failed.is_on());
        assert_eq!(Status::Downloading { done: 1, total: 4 }.progress(), Some(0.25));
        assert_eq!(Status::Downloading { done: 0, total: 0 }.progress(), Some(0.0));
        assert_eq!(Status::Ready.progress(), None);
    }
}
