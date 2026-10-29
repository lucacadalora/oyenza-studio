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
    /// Where it comes from when that isn't the add-on's own release: a tool's
    /// own release, unchanged (the Downloader's yt-dlp and QuickJS). Saved
    /// under `name` all the same.
    pub url: Option<&'static str>,
}

impl AddonFile {
    /// True when `bytes` are exactly this file.
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && sha256::hex(&sha256::sha256(bytes)) == self.sha256
    }

    /// The same for a file read from `r` 64 KB at a time, so checking one on
    /// disk never holds all of it in memory (the Downloader's yt-dlp is 24 MB).
    pub fn matches_read(&self, mut r: impl std::io::Read) -> bool {
        let (mut hash, mut n, mut buf) = (sha256::Sha256::new(), 0u64, vec![0u8; 64 * 1024]);
        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(k) => {
                    n += k as u64;
                    if n > self.size {
                        return false;
                    }
                    hash.update(&buf[..k]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        n == self.size && sha256::hex(&hash.finish()) == self.sha256
    }

    /// Whether this build of the app knows the file: a size and a SHA-256 to
    /// check it against. A placeholder (size 0, no SHA-256) stands for a file
    /// that isn't published yet, and nothing is downloaded for it.
    pub fn pinned(&self) -> bool {
        self.size > 0 && self.sha256.len() == 64 && self.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
    /// Part of Oyenza Studio Pro: it works only for an Oyenza account the
    /// server says has Pro (`GET /island/pro`; `AppState::pro`).
    pub pro: bool,
}

impl Addon {
    /// Every file together, in bytes.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn url(&self, file: &AddonFile) -> String {
        match file.url {
            Some(url) => url.to_string(),
            None => format!("{RELEASES}/{}/{}", self.tag, file.name),
        }
    }

    pub fn find(id: &str) -> Option<&'static Addon> {
        ALL.iter().copied().find(|a| a.id == id)
    }

    /// Whether it has files to download. One without (Script reader) is
    /// already in the app: the switch only shows or hides it, and the
    /// downloader never touches it.
    pub fn downloads(&self) -> bool {
        !self.files.is_empty()
    }

    /// Whether this build can get every file it needs ([`AddonFile::pinned`]).
    /// One that can't stays off ([`Status::Unpinned`]): a build made while a
    /// file is still a placeholder (a new add-on whose files aren't on its
    /// release yet).
    pub fn pinned(&self) -> bool {
        self.files.iter().all(AddonFile::pinned)
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
        AddonFile { name: "places.json", size: 79_687, sha256: "4d2e1cd6c277b97273f415cf0460579db143fe92ac88f6e4bddb79cdf8cbf69b", url: None },
        AddonFile { name: "adzan.m4a", size: 1_822_513, sha256: "bd5a9db9d23eb2fb7e3368c33afeb08603b71b762549ba8247491c5aac24cb59", url: None },
        AddonFile { name: "NOTICE.txt", size: 869, sha256: "a1762a46d2ff3c7456070f76fe370e02e169f30ee749fea7763568034401185a", url: None },
    ],
    pro: false,
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
        AddonFile { name: JA_READINGS_FILE, size: 3_790_036, sha256: "4950e5e42a2a48ad50396304c6bfbb8f9a97b33947e147007943361d65d856c8", url: None },
        AddonFile { name: "NOTICE.txt", size: 4_786, sha256: "fb8cd0420c2a8b04404c17a1a2ada30e0e1bbd73e54ef548bb60ede42198e079", url: None },
    ],
    pro: false,
};

/// Script reader: the Script page, a teleprompter under the webcam. Its
/// code is in the app (a few dozen KB), so there is nothing to download; off,
/// the island has no Script tab, no Alt+Shift+S and nothing running for it.
pub const SCRIPT_READER: Addon = Addon {
    id: "script",
    title: "Script reader",
    blurb: "A teleprompter under your webcam, left out of screen shares.",
    mac_blurb: Some("A teleprompter under your webcam."),
    tag: "",
    files: &[],
    pro: false,
};

/// System: the System page (the processor, graphics, memory, drives,
/// network and power, and the apps using them). A tab for everyone up to
/// 0.2.8; an add-on, off, in the versions after it, so the island's own tabs
/// are the everyday ones. Its code is in the app, so there is nothing to download.
/// Off, there is no System tab, and its readings never start at rest
/// (`Settings::system_history` only counts while it is on). Show over games
/// keeps its own switch: its readout reads the counters while a game is in
/// front, System on or off.
pub const SYSTEM: Addon = Addon {
    id: "system",
    title: "System",
    blurb: "Live CPU, GPU, memory, drives, network and power, and the apps using them.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: false,
};

/// Health: the Health page, today's steps against the goal and the week,
/// which Oyenza on the phone sends once signed in (Apple Health or Health
/// Connect). A tab for everyone up to 0.2.8; an add-on, off, in the versions
/// after it, like System. Its code is in the app, so there is nothing to
/// download. Off, there is no Health tab and the Oyenza worker never asks
/// for the steps.
pub const HEALTH: Addon = Addon {
    id: "health",
    title: "Health",
    blurb: "Your steps from Oyenza on your phone: today against your goal, and your week.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: false,
};

/// Stocks: the Stocks page, the IHSG and up to ten IDX stocks with their
/// day's line, from oyenza.trade (`poni_core::stocks`). Its code is in the
/// app, so there is nothing to download. Off, there is no Stocks tab; on, the
/// prices are asked for only while its page is on screen.
pub const STOCKS: Addon = Addon {
    id: "stocks",
    title: "Stocks",
    blurb: "The IHSG and up to 10 IDX stocks from oyenza.trade, 10 minutes behind IDX.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: false,
};

/// Notifications (Windows): other apps' notifications in the island, from
/// the apps the person picks on its page (WhatsApp, Telegram and Phone Link
/// to begin with), read through Windows' notification listener
/// (`poni_core::notifications`). Its code is in the app, so there is nothing
/// to download. Off, there is no Notifications tab and nothing listens.
/// Windows lets only an app with a package identity read notifications, so
/// it works in the Microsoft Store copy; the Mac has no way to read them.
pub const NOTIFICATIONS: Addon = Addon {
    id: "notifications",
    title: "Notifications",
    blurb: "Messages from WhatsApp, Telegram and your phone in the island.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: false,
};

/// Screen recorder: the Record page and the recording pill, the face of
/// Oyenza Studio Recorder (its own install, about 120 MB). Its code is in the
/// island, so there is nothing to download. It shows on the Add-ons page once
/// the recorder is installed, and is on from then until switched off
/// (`Settings::recorder_on`). Off, there is no Record tab, and the recorder
/// shows its own bar and controls.
pub const SCREEN_RECORDER: Addon = Addon {
    id: "recorder",
    title: "Screen recorder",
    blurb: "Record your screen from the island, with Oyenza Studio Recorder.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: false,
};

/// Downloader: the Downloads page, which saves the video or sound of a link
/// with yt-dlp (`ytdlp`, `downloads`). yt-dlp and QuickJS (which yt-dlp needs
/// for YouTube) come from their own releases, unchanged; FFmpeg, which joins
/// a video's picture and sound, is our own small build
/// (`tools/build-ffmpeg-min.sh`, `addons/downloader/`). Not in the Microsoft
/// Store build. Nothing runs at rest: the tools only run while a download
/// does. Part of Studio Pro: its tools download, and it downloads, only for
/// an account the server says has Pro. Each app has its own tools: this
/// one, or [`DOWNLOADER_MAC`] on a Mac.
pub const DOWNLOADER: Addon = if cfg!(target_os = "macos") { DOWNLOADER_MAC } else { DOWNLOADER_WINDOWS };

/// The Downloader on Windows: yt-dlp.exe and QuickJS-ng's Windows build from
/// their own releases, and our ffmpeg.exe.
pub const DOWNLOADER_WINDOWS: Addon = Addon {
    id: "downloader",
    title: "Downloader",
    blurb: "Save the video or sound of a link you paste, with yt-dlp.",
    mac_blurb: None,
    tag: "addon-downloader-1",
    files: &[
        AddonFile {
            name: "yt-dlp.exe",
            size: 23_575_744,
            sha256: "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
            url: Some("https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe"),
        },
        AddonFile {
            name: "qjs.exe",
            size: 2_151_363,
            sha256: "2aeabf0092c3262d6b2609824418f7dd7ed1f1df939f73b2b15645230cac0d77",
            url: Some("https://github.com/quickjs-ng/quickjs/releases/download/v0.17.0/qjs-windows-x86_64.exe"),
        },
        // tools/build-ffmpeg-min.sh's build (FFmpeg 9.0.2, the same bytes every time).
        AddonFile { name: "ffmpeg.exe", size: 2_298_880, sha256: "8a42638bc2c9bed20670a3e89ccb7117345a941e245d8151aa60066cac0b3993", url: None },
        AddonFile { name: "NOTICE.txt", size: 32_624, sha256: "01fb0f44904d1f4789a5bc048fc1b2c29d1a36c25d91469632af03d2402331a0", url: None },
    ],
    pro: true,
};

/// The Downloader on a Mac: yt-dlp's own Mac build (one file for Apple
/// silicon and Intel) from its release, unchanged, the same version as
/// Windows'. QuickJS-ng's own Mac builds are one per chip and need macOS 26,
/// so QuickJS-ng (the same version) and FFmpeg (the same build as Windows')
/// are ours, one file each for both chips and macOS 14 and later, from
/// mac.yml's downloader-tools job, on the add-on's own pre-release
/// (`addons/downloader-mac/`).
pub const DOWNLOADER_MAC: Addon = Addon {
    tag: "addon-downloader-mac-1",
    files: &[
        AddonFile {
            name: "yt-dlp",
            size: 37_466_304,
            sha256: "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa",
            url: Some("https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_macos"),
        },
        // mac.yml's downloader-tools job (QuickJS-ng v0.17.0; FFmpeg 9.0.2, tools/build-ffmpeg-min.sh darwin).
        AddonFile { name: "qjs", size: 2_456_496, sha256: "d4526ffc81f4251e7e835d789a441e367b03ba1700bd111e9ce1a1a5bec0134a", url: None },
        AddonFile { name: "ffmpeg", size: 4_307_968, sha256: "fb47d9aa82cf1e61aa49b54d1989fffef98571d1cec3d0f9a5133e0b63468724", url: None },
        AddonFile { name: "NOTICE.txt", size: 33_065, sha256: "aac524dad1d9ce62dbf07b9e8a5bd597461d21443e75feca087fb207fe231704", url: None },
    ],
    ..DOWNLOADER_WINDOWS
};

/// The Downloader's tools, as this app saves them in the add-on's folder.
pub const YTDLP_EXE: &str = if cfg!(target_os = "macos") { "yt-dlp" } else { "yt-dlp.exe" };
pub const QJS_EXE: &str = if cfg!(target_os = "macos") { "qjs" } else { "qjs.exe" };
pub const FFMPEG_EXE: &str = if cfg!(target_os = "macos") { "ffmpeg" } else { "ffmpeg.exe" };

/// Whether `name` is one of the Downloader's programs, which a Mac only runs
/// with its execute bit set (`services::addons`).
pub fn is_downloader_tool(name: &str) -> bool {
    [YTDLP_EXE, QJS_EXE, FFMPEG_EXE].contains(&name)
}

/// IELTS: Oyen's IELTS Speaking coach, the IELTS page (`crate::ielts`):
/// Oyen asks the questions out loud, listens to the answers through the
/// microphone and gives a band score, through the Oyenza account. Its code
/// is in the app, so there is nothing to download. Off, there is no IELTS
/// tab, and nothing of it runs: no microphone, no player, no calls. Part of
/// Studio Pro: the Add-ons page lists it only for an account the server says
/// has Pro, and its tab goes once the server says an account hasn't.
pub const IELTS: Addon = Addon {
    id: "ielts",
    title: "IELTS",
    blurb: "Practise IELTS Speaking out loud: Oyen asks, listens and gives you a band.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: true,
};

/// Meeting notes: the Notes page (`crate::notes`). One click and Oyen listens
/// to the microphone and the computer's own sound (a call in Meet, Zoom,
/// Teams; on a Mac through a Core Audio tap, macOS 14.2 and later), then the
/// Oyenza account writes the notes. Its code is in the
/// app, so there is nothing to download. Off, there is no Notes tab and
/// nothing of it runs: no recording, no calls. Part of Studio Pro: the page
/// records only for an account the server says has Pro, and the server
/// writes notes only for one.
pub const MEETING_NOTES: Addon = Addon {
    id: "notes",
    title: "Meeting notes",
    blurb: "One click and Oyen listens to your meeting, Meet, Zoom or Teams, and writes the notes.",
    mac_blurb: None,
    tag: "",
    files: &[],
    pro: true,
};

/// Every add-on, in the order the Add-ons page lists them.
pub const ALL: [&Addon; 11] =
    [&PRAYER_TIMES, &JAPANESE_READINGS, &SCRIPT_READER, &SYSTEM, &HEALTH, &STOCKS, &NOTIFICATIONS, &IELTS, &MEETING_NOTES, &SCREEN_RECORDER, &DOWNLOADER];

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
        Status::Failed | Status::Unpinned => Some(("Japanese readings didn't download: see Add-ons".to_string(), false)),
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
    /// Asked to switch on, but this build can't get its files yet
    /// ([`Addon::pinned`]): it stays off, and nothing downloads.
    Unpinned,
}

impl Status {
    /// The switch shows on for everything but `Off`, and `Unpinned`, which
    /// stays off.
    pub fn is_on(self) -> bool {
        !matches!(self, Status::Off | Status::Unpinned)
    }

    /// The line under the add-on's name; `mac`: in the Mac app.
    pub fn line(self, addon: &Addon, mac: bool) -> String {
        let here = if mac { "this Mac" } else { "this PC" };
        match self {
            Status::Off => addon.mac_blurb.filter(|_| mac).unwrap_or(addon.blurb).to_string(),
            Status::Checking => format!("Checking its files on {here}"),
            Status::Downloading { done, total } => format!("Downloading · {} of {}", fmt_bytes(done), fmt_bytes(total)),
            Status::Waiting => "No internet. It downloads by itself once you're back online.".to_string(),
            Status::Ready if !addon.downloads() => "On".to_string(),
            Status::Ready => format!("On · {} on {here}", fmt_bytes(addon.size())),
            Status::Failed => "The download didn't come through right. Switch it off and on to try again.".to_string(),
            Status::Unpinned => "This version can't get its files yet, so it stays off.".to_string(),
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
    fn script_reader_is_built_in() {
        assert!(!SCRIPT_READER.downloads() && SCRIPT_READER.size() == 0);
        assert!(PRAYER_TIMES.downloads() && JAPANESE_READINGS.downloads());
        assert_eq!(Status::Ready.line(&SCRIPT_READER, false), "On");
        assert_eq!(Status::Off.line(&SCRIPT_READER, true), "A teleprompter under your webcam.");
        assert_eq!(Addon::find("script"), Some(&SCRIPT_READER));
    }

    #[test]
    fn pro_is_ielts_meeting_notes_and_the_downloader() {
        let pro: Vec<&str> = ALL.iter().filter(|a| a.pro).map(|a| a.id).collect();
        assert_eq!(pro, vec!["ielts", "notes", "downloader"]);
    }

    #[test]
    fn built_in_add_ons_are_off_until_switched_on() {
        for a in [&SYSTEM, &HEALTH, &STOCKS, &NOTIFICATIONS, &IELTS, &MEETING_NOTES] {
            assert!(!a.downloads() && a.size() == 0, "{}", a.id);
            assert_eq!(Addon::find(a.id), Some(a));
            assert_eq!(Status::Ready.line(a, false), "On");
            assert!(!Settings::default().addon_on(a.id), "{} is off for everyone until they switch it on", a.id);
        }
        // Clipboard history left the app after 0.2.8: no add-on by that id.
        assert_eq!(Addon::find("clipboard"), None);
    }

    #[test]
    fn screen_recorder_is_on_once_the_recorder_is_installed() {
        assert!(!SCREEN_RECORDER.downloads() && SCREEN_RECORDER.size() == 0);
        assert_eq!(Addon::find("recorder"), Some(&SCREEN_RECORDER));
        let mut s = Settings::default();
        assert!(!s.recorder_on(false), "no recorder: nothing to show");
        assert!(s.recorder_on(true), "installing the recorder is the switch");
        s.set_addon(SCREEN_RECORDER.id, false);
        assert!(!s.recorder_on(true), "switched off by hand: stays off");
        s.set_addon(SCREEN_RECORDER.id, true);
        assert!(s.recorder_on(true) && !s.recorder_on(false));
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

    #[test]
    fn add_ons_are_listed_in_order_each_with_its_own_id() {
        // The Add-ons page's order: the Downloader last.
        let ids: Vec<&str> = ALL.iter().map(|a| a.id).collect();
        assert_eq!(ids, ["sholat", "ja", "script", "system", "health", "stocks", "notifications", "ielts", "notes", "recorder", "downloader"]);
        for (i, a) in ALL.iter().enumerate() {
            // The key in settings.json and the folder's name.
            assert!(!a.id.is_empty() && a.id.bytes().all(|b| b.is_ascii_lowercase()), "{}", a.id);
            assert!(ALL[..i].iter().all(|b| b.id != a.id && b.title != a.title), "{} twice", a.id);
            assert_eq!(Addon::find(a.id), Some(*a));
            assert_eq!(a.downloads(), !a.tag.is_empty(), "{}: a release exactly when there are files", a.id);
            // Each file a name of its own in the folder, and none of the
            // downloader's own there (its `.ok` stamp, a `.part` on its way).
            for (j, f) in a.files.iter().enumerate() {
                assert!(a.files[..j].iter().all(|g| g.name != f.name), "{}: {} twice", a.id, f.name);
                assert!(!f.name.starts_with('.') && !f.name.ends_with(".part") && !f.name.contains(['/', '\\']), "{}: {}", a.id, f.name);
            }
        }
    }

    #[test]
    fn the_downloader_takes_its_tools_from_their_own_releases() {
        // Each app has its own tools, under the names it runs them by.
        assert_eq!(Addon::find("downloader"), Some(&DOWNLOADER));
        let names = |d: &Addon| d.files.iter().map(|f| f.name).collect::<Vec<_>>();
        assert_eq!(names(&DOWNLOADER), [YTDLP_EXE, QJS_EXE, FFMPEG_EXE, "NOTICE.txt"]);
        assert_eq!(names(&DOWNLOADER_WINDOWS), ["yt-dlp.exe", "qjs.exe", "ffmpeg.exe", "NOTICE.txt"]);
        assert_eq!(names(&DOWNLOADER_MAC), ["yt-dlp", "qjs", "ffmpeg", "NOTICE.txt"]);
        assert!(names(&DOWNLOADER)[..3].iter().all(|n| is_downloader_tool(n)) && !is_downloader_tool("NOTICE.txt"));
        // One add-on in both apps (its switch, its row), each with its own
        // pre-release, so each keeps its own folder.
        assert_eq!(DOWNLOADER_MAC, Addon { tag: DOWNLOADER_MAC.tag, files: DOWNLOADER_MAC.files, ..DOWNLOADER_WINDOWS });
        assert_ne!(DOWNLOADER_MAC.tag, DOWNLOADER_WINDOWS.tag);
        // yt-dlp, and QuickJS on Windows: their projects' own releases,
        // unchanged, each from a fixed tag (a `latest` link would change
        // under the pin); yt-dlp the same version on both.
        let w = &DOWNLOADER_WINDOWS;
        let m = &DOWNLOADER_MAC;
        let mut ytdlp_tags = Vec::new();
        for (d, f, project, asset) in
            [(w, &w.files[0], "yt-dlp/yt-dlp", "yt-dlp.exe"), (w, &w.files[1], "quickjs-ng/quickjs", "qjs-windows-x86_64.exe"), (m, &m.files[0], "yt-dlp/yt-dlp", "yt-dlp_macos")]
        {
            let url = d.url(f);
            let tag = url.strip_prefix(&format!("https://github.com/{project}/releases/download/")).and_then(|rest| rest.strip_suffix(&format!("/{asset}")));
            assert!(tag.is_some_and(|t| !t.is_empty() && !t.contains('/')), "{url}");
            assert!(f.pinned(), "{}: a tool from elsewhere is always pinned", f.name);
            if project == "yt-dlp/yt-dlp" {
                ytdlp_tags.extend(tag.map(str::to_string));
            }
        }
        assert_eq!(ytdlp_tags[0], ytdlp_tags[1], "yt-dlp: the same version on Windows and the Mac");
        // Our own builds and the notices: each add-on's own pre-release.
        let ours = "https://github.com/lucacadalora/oyenza-studio/releases/download";
        assert_eq!(w.url(&w.files[2]), format!("{ours}/addon-downloader-1/ffmpeg.exe"));
        assert_eq!(w.url(&w.files[3]), format!("{ours}/addon-downloader-1/NOTICE.txt"));
        for (f, name) in m.files[1..].iter().zip(["qjs", "ffmpeg", "NOTICE.txt"]) {
            assert_eq!(m.url(f), format!("{ours}/addon-downloader-mac-1/{name}"));
        }
    }

    #[test]
    fn an_add_on_with_a_placeholder_file_stays_off() {
        const SOON: Addon = Addon {
            id: "soon",
            title: "Soon",
            blurb: "Not out yet.",
            mac_blurb: None,
            tag: "addon-soon-1",
            files: &[
                AddonFile { name: "tool.exe", size: 2_151_363, sha256: "2aeabf0092c3262d6b2609824418f7dd7ed1f1df939f73b2b15645230cac0d77", url: None },
                AddonFile { name: "NOTICE.txt", size: 0, sha256: "", url: None },
            ],
            pro: false,
        };
        let tool = SOON.files[0];
        assert!(tool.pinned() && !SOON.files[1].pinned() && !SOON.pinned());
        // Half a pin is none.
        for f in [
            AddonFile { sha256: "", ..tool },
            AddonFile { size: 0, ..tool },
            AddonFile { sha256: &tool.sha256[1..], ..tool },
            AddonFile { sha256: "2AEABF0092C3262D6B2609824418F7DD7ED1F1DF939F73B2B15645230CAC0D77", ..tool },
        ] {
            assert!(!f.pinned(), "{f:?}");
        }
        // Everything in the app has its files pinned (built-ins have none to
        // pin), in both apps.
        for a in ALL.iter().copied().chain([&DOWNLOADER_WINDOWS, &DOWNLOADER_MAC]) {
            assert!(a.pinned(), "{} ({})", a.id, a.tag);
        }
        // The switch stays off and the line says why; nothing comes in.
        let s = Status::Unpinned;
        assert!(!s.is_on() && s.progress().is_none());
        assert_eq!(s.line(&SOON, false), "This version can't get its files yet, so it stays off.");
        assert_eq!(s.line(&SOON, true), s.line(&SOON, false));
        assert!(ja_offer(s).is_some_and(|(_, click)| !click));
    }

    #[test]
    fn the_downloaders_notices_are_the_ones_in_the_repo() {
        let windows: &[u8] = include_bytes!("../../../addons/downloader/NOTICE.txt");
        let mac: &[u8] = include_bytes!("../../../addons/downloader-mac/NOTICE.txt");
        for (d, bytes) in [(DOWNLOADER_WINDOWS, windows), (DOWNLOADER_MAC, mac)] {
            let notice = d.files.iter().find(|f| f.name == "NOTICE.txt").unwrap();
            assert!(notice.matches(bytes), "{}: size {} sha256 {}", d.tag, bytes.len(), sha256::hex(&sha256::sha256(bytes)));
            assert_eq!(d.url(notice), format!("https://github.com/lucacadalora/oyenza-studio/releases/download/{}/NOTICE.txt", d.tag));
        }
    }

    #[test]
    fn a_file_on_disk_is_checked_a_piece_at_a_time() {
        // The dictionary (3.8 MB) in 64 KB reads, against the same pin.
        let dict: &[u8] = include_bytes!("../data/ja-readings.dic.zst");
        let f = &JAPANESE_READINGS.files[0];
        assert!(f.matches_read(dict));
        let mut changed = dict.to_vec();
        *changed.last_mut().unwrap() ^= 1;
        assert!(!f.matches_read(&changed[..]));
        assert!(!f.matches_read(&dict[..dict.len() - 1]), "a byte short");
        assert!(!f.matches_read(std::io::Read::chain(dict, &b"!"[..])), "a byte more");
        // A file that can't be read to its end isn't the file.
        struct Unplugged;
        impl std::io::Read for Unplugged {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("unplugged"))
            }
        }
        assert!(!f.matches_read(Unplugged));
    }

    #[test]
    fn a_big_add_on_reads_well() {
        // About the Downloader's size with FFmpeg: whole megabytes from 10 MB up.
        const BIG: Addon = Addon {
            id: "big",
            title: "Big",
            blurb: "Big tools.",
            mac_blurb: None,
            tag: "addon-big-1",
            files: &[AddonFile { name: "big.exe", size: 30_100_000, sha256: "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d", url: None }],
            pro: false,
        };
        assert_eq!(fmt_bytes(BIG.size()), "30 MB", "its size on the page while it's off");
        assert_eq!(Status::Downloading { done: 12_345_678, total: BIG.size() }.line(&BIG, false), "Downloading · 12 MB of 30 MB");
        assert_eq!(Status::Ready.line(&BIG, false), "On · 30 MB on this PC");
    }
}
