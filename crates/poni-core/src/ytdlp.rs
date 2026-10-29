//! yt-dlp, the Downloader add-on's engine (`addons::DOWNLOADER`): the
//! arguments the island runs it with, and what each line it prints means.
//! Pure, so both are tested here against yt-dlp's real output
//! (`tests/fixtures/ytdlp/`). The Windows app runs it (`services::downloader`).
//!
//! Besides its own lines (`[Merger] Merging formats into "…"`, `ERROR: …`),
//! yt-dlp prints the JSON lines [`args`] asks for, one object a line:
//! `{"t":"info"}` before a download, `{"t":"dl"}` as it goes, `{"t":"pp"}` as
//! each step after it starts and ends, and `{"t":"done"}` with the finished
//! file. In JSON a title's quotes stay inside its line, and yt-dlp writes
//! letters outside ASCII as `\uXXXX`, so no code page can garble them.

use serde_json::Value;
use std::borrow::Cow;

/// Video (picture and sound) or the sound alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Video,
    Audio,
}

/// One download the person asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub url: String,
    pub mode: Mode,
    /// The tallest picture wanted (None: the best there is).
    pub max_height: Option<u32>,
    /// The folder the finished file goes in.
    pub folder: String,
}

/// Where the add-on's tools are on this PC.
#[derive(Clone, Copy, Debug)]
pub struct Tools<'a> {
    pub ytdlp: &'a str,
    pub qjs: &'a str,
    pub ffmpeg: &'a str,
    /// yt-dlp's cache folder (`--cache-dir`), in the add-on's own data.
    pub cache: &'a str,
}

/// What one line of yt-dlp's output says.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// What is being downloaded (once per item, before its first byte).
    Info { title: String, site: String, thumbnail: Option<String>, duration_s: Option<f64> },
    /// Bytes so far of the part being fetched (`total` None while unknown).
    Progress { done: u64, total: Option<u64>, speed: Option<f64>, eta_s: Option<u64> },
    /// A step after the bytes are in.
    Stage(Stage),
    /// The finished file.
    Done { path: String },
    /// yt-dlp gave up on this item.
    Failed(Problem),
}

/// The steps after downloading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Joining the picture and the sound into one file.
    Merging,
    /// Turning the sound into another format.
    Converting,
    /// Tidying the file (container fixes, metadata).
    Fixing,
}

/// Why a download failed, in the words the page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No video or sound yt-dlp knows how to read at this link.
    Unsupported,
    /// The page or video isn't there (any more).
    NotFound,
    /// Private, or only for members.
    Private,
    /// The site wants a signed-in person (YouTube's "confirm you're not a bot").
    SignIn,
    /// Not shown in this country.
    Region,
    /// The site is limiting this network for now.
    Limited,
    /// No connection, or it dropped.
    Network,
    /// The site's copy protection (DRM): it can't be saved.
    Protected,
    /// Anything else, in yt-dlp's own words (trimmed).
    Other(String),
}

impl Problem {
    /// The line the Downloads page shows for it.
    pub fn message(&self) -> String {
        let says = match self {
            Problem::Unsupported => "No video or sound found at this link",
            Problem::NotFound => "This video isn't there any more",
            Problem::Private => "This video is private",
            Problem::SignIn => "The site wants you signed in",
            Problem::Region => "Not available in your country",
            Problem::Limited => "The site is limiting downloads from this network for now. Try again later",
            Problem::Network => "Can't connect. Check your internet",
            Problem::Protected => "Protected by the site: it can't be saved",
            Problem::Other(said) if !said.trim().is_empty() => said.trim(),
            Problem::Other(_) => "The download failed",
        };
        says.to_string()
    }
}

/// A progress line, `{"t":"dl","id":"…","st":"downloading","done":1024,
/// "total":0,"est":25056.0,"speed":881.5,"eta":0,"frag":0,"frags":18}`: the
/// bytes so far, the size (`est` when yt-dlp can only estimate it, as for HLS
/// and DASH) and the fragments. A number yt-dlp doesn't know prints 0.
const PROGRESS: &str = concat!(
    r#"download:{"t":"dl","id":%(info.id|null)j,"st":"%(progress.status)s","done":%(progress.downloaded_bytes|0)s,"#,
    r#""total":%(progress.total_bytes|0)s,"est":%(progress.total_bytes_estimate|0)s,"speed":%(progress.speed|0)s,"#,
    r#""eta":%(progress.eta|0)s,"frag":%(progress.fragment_index|0)s,"frags":%(progress.fragment_count|0)s}"#,
);
/// A step after the download starting or ending: `{"t":"pp","id":"…",
/// "pp":"Merger","st":"started"}`, `pp` being yt-dlp's name for the step.
const STEP: &str = r#"postprocess:{"t":"pp","id":%(info.id|null)j,"pp":"%(progress.postprocessor)s","st":"%(progress.status)s"}"#;
/// What is being downloaded, before its first byte. `site` is yt-dlp's name
/// for the site ("PeerTube", "generic") and `domain` the page's
/// ("video.blender.org"): `ytdlp_info::site_name` makes one name of them.
const INFO: &str = concat!(
    r#"before_dl:{"t":"info","id":%(id|null)j,"title":%(title|null)j,"site":%(extractor|null)j,"#,
    r#""domain":%(webpage_url_domain|null)j,"thumb":%(thumbnail|null)j,"duration":%(duration|null)j}"#,
);
/// The finished file, once it is where it stays (joined or tidied). Every
/// `j` field above has `|null`: one yt-dlp doesn't know prints `NA`
/// otherwise, which isn't JSON.
const DONE: &str = r#"after_move:{"t":"done","path":%(filepath|null)j}"#;

/// The name a video is saved under: its title, cut to 150 bytes to keep the
/// path short (Windows allows 260 characters), and the site's id for it,
/// which tells two videos with one title apart and lets a stopped download
/// carry on. yt-dlp swaps what Windows refuses in a name (`:` for `：`).
const VIDEO_NAME: &str = "%(title).150B [%(id).50B].%(ext)s";
/// The sound alone has a name of its own: a sound-only file can be an .mp4
/// too (PeerTube's is), and under the video's name yt-dlp would take a video
/// of the same link already in the folder for it ("has already been
/// downloaded"), or two downloads of one link would write one `.part` file.
const AUDIO_NAME: &str = "%(title).150B [%(id).50B] (audio).%(ext)s";

/// The best video, in what Windows plays without extra codecs: H.264 before
/// a sharper picture that needs one, AAC sound. yt-dlp's own `-t mp4` preset
/// order, with MP4 files before other containers: our FFmpeg can't remux an
/// AVI or OGV into MP4 the way the preset does (archive.org offers its films
/// as AVI, OGV and MP4, none with its codecs named).
const VIDEO_SORT: &str = "vcodec:h264,vext,lang,quality,res,fps,hdr:12,acodec:aac";
/// The sound alone: a sound-only format, AAC first (it plays everywhere);
/// else the smallest MP4 with sound. Smallest because PeerTube names no codec
/// for its sound-only file, so yt-dlp takes it for a video (its height is
/// unknown, which counts as the smallest), and a site whose HLS has no
/// sound-only stream needn't send its 1080p (490 MB instead of 22 MB on Mux's
/// test stream). MP4 first, before a smaller OGV that Windows doesn't play.
const AUDIO_SORT: &str = "lang,vext,+res,acodec:aac";

/// The arguments for `req` (the program itself not included).
pub fn args(req: &Request, tools: &Tools) -> Vec<String> {
    let qjs = format!("quickjs:{}", tools.qjs);
    let home = format!("home:{}", req.folder);
    let video = video_formats(req.max_height);
    let mut a = vec![
        // Only these: no config file or plugin of a yt-dlp the person has, and
        // the cache (YouTube's player code, QuickJS's slow answers to it) in
        // the add-on's folder rather than their home.
        "--ignore-config", "--no-plugin-dirs", "--cache-dir", tools.cache,
        // Plain lines for a pipe: no colour or cursor codes (`--no-colors`
        // still writes cursor codes), a line per progress report, UTF-8.
        "--color", "never", "--newline", "--encoding", "utf-8",
        // `--print` turns on quiet and simulate: download all the same, and
        // keep yt-dlp's own lines. A progress line at most every half second:
        // yt-dlp writes one per block it reads (224 in 10 s for a 23 MB film).
        "--no-simulate", "--no-quiet", "--progress", "--progress-delta", "0.5",
        "--progress-template", PROGRESS, "--progress-template", STEP, "--print", INFO, "--print", DONE,
        // One video per link: the one it points at, or a playlist's first (a
        // channel's link would download the whole channel).
        "--no-playlist", "--playlist-items", "1",
        // Today's date on the file (yt-dlp's default now), names Windows takes
        // (its default on Windows; the same names from these Linux tests), and
        // a stopped download carries on from its `.part` file.
        "--no-mtime", "--windows-filenames", "--continue",
        "--ffmpeg-location", tools.ffmpeg,
        // QuickJS answers YouTube's JavaScript; yt-dlp would look for Deno.
        "--no-js-runtimes", "--js-runtimes", qjs.as_str(),
        // The finished files' folder, as yt-dlp's `home` path.
        "-P", home.as_str(),
    ];
    // Our FFmpeg only copies (no encoders): it joins and tidies files, never
    // converts them, so no `-x`, `--audio-format`, `--remux-video` or
    // `--embed-…` here.
    match req.mode {
        // Joined into the first container that holds both codecs: MP4 for
        // H.264 or AV1 with AAC, WebM for VP9 or AV1 with Opus, else MKV,
        // which takes any (a plain `mp4` makes FFmpeg fail on VP8 or Vorbis).
        Mode::Video => a.extend(["-o", VIDEO_NAME, "-f", video.as_str(), "-S", VIDEO_SORT, "--merge-output-format", "mp4/webm/mkv"]),
        // The sound as the site sends it: M4A, MP3, Opus in WebM, or the
        // smallest MP4 when it has no sound alone.
        Mode::Audio => a.extend(["-o", AUDIO_NAME, "-f", "ba/b", "-S", AUDIO_SORT]),
    }
    // A pasted link that starts with `-` is a link, not an option.
    a.extend(["--", req.url.as_str()]);
    a.into_iter().map(String::from).collect()
}

/// The formats to download for a video: the best picture (a file with sound
/// or one without, then joined with the best sound), else the best file with
/// both. A cap lets in pictures that tall or less, and falls back to the best
/// when none is (or no height is known).
fn video_formats(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
        None => "bv*+ba/b".to_string(),
    }
}

/// What `line` (one line of yt-dlp's stdout or stderr, without its line
/// ending) says; None for lines the island doesn't need.
pub fn parse(line: &str) -> Option<Line> {
    if line.contains('\r') {
        console(line)
    } else {
        one(&plain(line))
    }
}

/// A line with `\r`s, read as a console shows it: the text after the last
/// one. yt-dlp writes them (and escape codes) only without `--newline` and
/// `--color never`, and before a download error it gives up on, which reads
/// `ERROR: \r[download] Got error: …` (`report_retry`,
/// yt_dlp/downloader/common.py): that `\r` belongs to the error.
fn console(line: &str) -> Option<Line> {
    let pieces: Vec<Cow<str>> = line.split('\r').map(plain).collect();
    match pieces.iter().position(|p| p.starts_with("ERROR:")) {
        Some(i) => one(&pieces[i..].join(" ")),
        None => one(pieces.iter().rev().find(|p| !p.is_empty())?),
    }
}

/// One line as it shows.
fn one(text: &str) -> Option<Line> {
    if let Some(said) = text.strip_prefix("ERROR:") {
        // Empty when the reader split the error at its `\r`: the reason comes
        // on the next line.
        let said = said.trim();
        return (!said.is_empty()).then(|| Line::Failed(problem(said)));
    }
    if text.starts_with('{') {
        return json(text);
    }
    if let Some(why) = text.strip_prefix("[download] Got error: ") {
        // yt-dlp tries again first ("… Retrying (1/10)..."); without
        // "Retrying" it gave up.
        return (!why.contains("Retrying")).then(|| Line::Failed(problem(why)));
    }
    if let Some((_, why)) = text.strip_prefix("yt-dlp").and_then(|t| t.split_once(": error: ")) {
        // yt-dlp refused an argument ("yt-dlp.exe: error: no such option: …"):
        // the pinned yt-dlp doesn't take one of ours.
        return Some(Line::Failed(Problem::Other(short(why.trim()))));
    }
    // yt-dlp's own line for a step: `[Merger] Merging formats into "…"`.
    let (tag, rest) = text.strip_prefix('[')?.split_once("] ")?;
    match step(tag)? {
        // "[ExtractAudio] Not converting audio …; file is already in target format m4a"
        Stage::Converting if rest.starts_with("Not converting") => None,
        stage => Some(Line::Stage(stage)),
    }
}

/// One of the JSON lines [`args`] asks for.
fn json(text: &str) -> Option<Line> {
    // A field without `|null` prints NA, which isn't JSON.
    let v: Value = serde_json::from_str(text).or_else(|_| serde_json::from_str(&text.replace(":NA,", ":null,").replace(":NA}", ":null}"))).ok()?;
    match v["t"].as_str()? {
        "dl" => Some(progress(&v)),
        "pp" if v["st"] == "started" => step(v["pp"].as_str()?).map(Line::Stage),
        "info" => Some(info(&v)),
        "done" => v["path"].as_str().filter(|p| !p.is_empty()).map(|p| Line::Done { path: p.to_string() }),
        _ => None,
    }
}

/// A `{"t":"dl"}` line. Its 0s stand for what yt-dlp didn't know.
fn progress(v: &Value) -> Line {
    let num = |key: &str| v[key].as_f64().filter(|n| n.is_finite() && *n > 0.0);
    // HLS and DASH never know their size: only the estimate, which swings
    // early on and settles.
    let mut total = num("total").or_else(|| num("est")).map(|t| t.round() as u64);
    let mut done = num("done").map_or(0, |d| d.round() as u64);
    if v["st"] == "finished" {
        // All of it, also for a file that was there already, which finishes
        // with done 0 ("has already been downloaded").
        done = done.max(total.unwrap_or(0));
        total = Some(done).filter(|d| *d > 0);
    }
    Line::Progress { done, total, speed: num("speed"), eta_s: num("eta").map(|e| e.round() as u64) }
}

/// A `{"t":"info"}` line.
fn info(v: &Value) -> Line {
    let text = |key: &str| v[key].as_str().map(str::trim).unwrap_or_default();
    Line::Info {
        title: text("title").to_string(),
        site: crate::ytdlp_info::site_name(text("site"), text("domain")),
        // A web address only: the page fetches it.
        thumbnail: Some(text("thumb")).filter(|t| t.starts_with("https://") || t.starts_with("http://")).map(String::from),
        duration_s: v["duration"].as_f64().filter(|d| d.is_finite() && *d > 0.0),
    }
}

/// The step yt-dlp's postprocessor `pp` is (its name in `{"t":"pp"}` lines and
/// in its own lines' `[…]` tag). None for those that only move files about
/// (`MoveFiles` runs after every download, `Concat` after a playlist).
fn step(pp: &str) -> Option<Stage> {
    match pp {
        "Merger" => Some(Stage::Merging),
        "ExtractAudio" | "VideoConvertor" => Some(Stage::Converting),
        _ if pp.starts_with("Fixup") || pp == "VideoRemuxer" => Some(Stage::Fixing),
        _ => None,
    }
}

/// `s` without terminal escape codes (`\x1b[K` clears the rest of a console
/// line) and without spaces around it.
fn plain(s: &str) -> Cow<'_, str> {
    if !s.contains('\x1b') {
        return Cow::Borrowed(s.trim());
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
        } else if chars.next_if_eq(&'[').is_some() {
            // Numbers and `;`, then one of `@` to `~` ends the code.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    Cow::Owned(out.trim().to_string())
}

/// What Python, urllib3 and Windows say when the connection itself failed:
/// "Failed to resolve '…' ([Errno 11001] getaddrinfo failed)", "[WinError
/// 10061] No connection could be made because the target machine actively
/// refused it", "Connection to … timed out" (yt-dlp's requests handler,
/// yt_dlp/networking/_requests.py; Windows' socket errors 10051 to 10065).
const NETWORK: &[&str] = &[
    "failed to resolve", "getaddrinfo failed", "name or service not known", "temporary failure in name resolution", "nodename nor servname",
    "failed to establish a new connection", "connection refused", "actively refused", "timed out", "failed to respond",
    "connection reset", "forcibly closed", "connection aborted", "connection was aborted", "network is unreachable",
    "unreachable network", "unreachable host", "no route to host", "remote end closed connection", "incompleteread",
    "unable to connect to proxy", "urlopen error",
];

/// Why yt-dlp gave up, from its message (after `ERROR:`). The reasons are
/// the sites' own words, which yt-dlp passes on, or the network's
/// ([`NETWORK`]). YouTube's player says "Sign in to confirm you’re not a bot"
/// (a curly apostrophe), "Private video. Sign in if you've been granted
/// access to this video", "Join this channel to get access to members-only
/// content like this video…", "Video unavailable. This video has been removed
/// by the uploader" or "The uploader has not made this video available in
/// your country", and yt-dlp adds that the session "has been rate-limited by
/// YouTube" or that YouTube "is requiring a captcha challenge"
/// (yt_dlp/extractor/youtube/_video.py, where it reads `playabilityStatus`).
/// For any site yt-dlp says "This video is DRM protected", "This video is not
/// available from your location due to geo restriction", "This video is only
/// available for registered users" (yt_dlp/extractor/common.py) and
/// "Unsupported URL: …" (yt_dlp/utils/_utils.py).
fn problem(msg: &str) -> Problem {
    let msg = tidy(msg);
    let low = msg.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| low.contains(w));
    if says(&["unsupported url", "is not a valid url", "no suitable extractor", "no video formats found", "requested format is not available"]) {
        Problem::Unsupported
    } else if says(&["drm protect", "drm-protect"]) {
        Problem::Protected
    } else if says(&["private video", "video is private", "members-only", "members only", "join this channel"]) {
        // Before signing in: YouTube asks a private video's viewers to sign in too.
        Problem::Private
    } else if says(&["sign in", "log in", "logged in", "login required", "login details", "registered users", "not a bot", "confirm your age", "age-restricted", "http error 401"]) {
        Problem::SignIn
    } else if says(&["your country", "your location", "geo restrict", "geo-restrict"]) {
        Problem::Region
    } else if says(&["http error 429", "too many requests", "rate-limit", "rate limit", "captcha"]) {
        Problem::Limited
    } else if says(&["http error 404", "http error 410", "video unavailable", "video is unavailable", "has been removed", "no longer available", "does not exist"]) {
        Problem::NotFound
    } else if says(NETWORK) {
        Problem::Network
    } else {
        Problem::Other(short(msg))
    }
}

/// `msg` without what yt-dlp wraps around a reason: the `[extractor] id: `
/// before it (an id has no spaces, unlike "[generic] Unable to …"), a
/// download's "Got error: ", and after it the Python exception it was
/// "caused by", the retries it gave up after and its request for a bug
/// report (`ExtractorError`, `bug_reports_message`: yt_dlp/utils/_utils.py).
fn tidy(msg: &str) -> &str {
    let mut msg = msg.trim();
    if let Some((_, rest)) = msg.strip_prefix('[').and_then(|m| m.split_once("] ")) {
        msg = match rest.split_once(": ") {
            Some((id, after)) if !id.is_empty() && !id.contains(' ') => after,
            _ => rest,
        };
    }
    msg = msg.strip_prefix("Got error: ").unwrap_or(msg);
    for end in [" (caused by ", "; please report this issue", ". Giving up after "] {
        if let Some(i) = msg.find(end) {
            msg = &msg[..i];
        }
    }
    msg.trim()
}

/// `text` in 120 characters at most, for a line on the page.
fn short(text: &str) -> String {
    const MAX: usize = 120;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader";
    const TOOLS: Tools<'static> = Tools {
        ytdlp: r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\yt-dlp.exe",
        qjs: r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\qjs.exe",
        ffmpeg: r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\ffmpeg.exe",
        cache: r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\cache",
    };
    const FILM: &str = "https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605";
    const OVERGROWN: &str = "OVERGROWN: a feature film project by Blender Studio";
    const OVERGROWN_THUMB: &str = "https://video.blender.org/lazy-static/thumbnails/2d0a6dff-ad41-4e1c-a921-53338b30759c.jpg";

    fn request(mode: Mode, max_height: Option<u32>) -> Request {
        Request { id: 7, url: FILM.into(), mode, max_height, folder: r"C:\Users\Luca Cada\Downloads".into() }
    }

    #[test]
    fn video_arguments() {
        let want = [
            "--ignore-config", "--no-plugin-dirs", "--cache-dir", r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\cache",
            "--color", "never", "--newline", "--encoding", "utf-8",
            "--no-simulate", "--no-quiet", "--progress", "--progress-delta", "0.5",
            "--progress-template", PROGRESS, "--progress-template", STEP, "--print", INFO, "--print", DONE,
            "--no-playlist", "--playlist-items", "1",
            "--no-mtime", "--windows-filenames", "--continue",
            "--ffmpeg-location", r"C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\ffmpeg.exe",
            "--no-js-runtimes", "--js-runtimes", r"quickjs:C:\Users\Luca Cada\AppData\Local\Poni\addons\downloader\qjs.exe",
            "-P", r"home:C:\Users\Luca Cada\Downloads",
            "-o", "%(title).150B [%(id).50B].%(ext)s",
            "-f", "bv*+ba/b",
            "-S", "vcodec:h264,vext,lang,quality,res,fps,hdr:12,acodec:aac",
            "--merge-output-format", "mp4/webm/mkv",
            "--", FILM,
        ];
        assert_eq!(args(&request(Mode::Video, None), &TOOLS), want);
    }

    #[test]
    fn audio_arguments_keep_the_sound_as_it_comes() {
        let audio = args(&request(Mode::Audio, None), &TOOLS);
        let video = args(&request(Mode::Video, None), &TOOLS);
        let name = video.iter().position(|a| a == "-o").unwrap();
        assert_eq!(audio[..name], video[..name], "the same up to the name");
        assert_eq!(audio[name..], ["-o", "%(title).150B [%(id).50B] (audio).%(ext)s", "-f", "ba/b", "-S", "lang,vext,+res,acodec:aac", "--", FILM]);
    }

    #[test]
    fn nothing_asks_our_ffmpeg_to_convert() {
        // It has no encoders: these would fail (or make FFmpeg download).
        let refused = [
            "-x", "--extract-audio", "--audio-format", "--audio-quality", "--remux-video", "--recode-video", "--embed-metadata", "--add-metadata",
            "--embed-thumbnail", "--embed-subs", "--embed-chapters", "--convert-thumbnails", "--convert-subs", "--download-sections", "--downloader",
            "--external-downloader", "--postprocessor-args", "--sponsorblock-remove", "--split-chapters", "--live-from-start",
        ];
        for mode in [Mode::Video, Mode::Audio] {
            for h in [None, Some(480)] {
                let a = args(&request(mode, h), &TOOLS);
                let flags: Vec<&String> = a.iter().take_while(|a| *a != "--").filter(|a| a.starts_with('-')).collect();
                assert!(flags.iter().all(|f| !refused.contains(&f.as_str())), "{mode:?} {h:?}: {flags:?}");
            }
        }
    }

    #[test]
    fn a_height_cap_takes_the_best_picture_that_fits() {
        let formats = |mode, h| {
            let a = args(&request(mode, h), &TOOLS);
            a[a.iter().position(|a| a == "-f").unwrap() + 1].clone()
        };
        assert_eq!(formats(Mode::Video, Some(720)), "bv*[height<=720]+ba/b[height<=720]/bv*+ba/b");
        assert_eq!(formats(Mode::Video, Some(1080)), "bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/b");
        assert_eq!(formats(Mode::Video, None), "bv*+ba/b");
        // Sound has no height.
        assert_eq!(formats(Mode::Audio, Some(720)), "ba/b");
    }

    #[test]
    fn paths_with_spaces_and_odd_links_stay_one_argument_each() {
        let req = Request { url: "--exec=calc.exe".into(), folder: r"D:\Video Saya\Unduhan (2026)".into(), ..request(Mode::Audio, None) };
        let a = args(&req, &TOOLS);
        // std's Command quotes each argument for Windows; none is split or joined here.
        let after = |flag: &str| a[a.iter().position(|a| a == flag).unwrap() + 1].as_str();
        assert_eq!(after("-P"), r"home:D:\Video Saya\Unduhan (2026)");
        assert_eq!(after("--ffmpeg-location"), TOOLS.ffmpeg);
        assert_eq!(after("--cache-dir"), format!(r"{DIR}\cache"));
        // yt-dlp splits a runtime from its path at the first colon only.
        assert_eq!(after("--js-runtimes").split_once(':'), Some(("quickjs", TOOLS.qjs)));
        // The link comes after `--`, last, so yt-dlp can't take it for an option.
        assert_eq!(a[a.len() - 2..], ["--", "--exec=calc.exe"]);
        assert_eq!(a.iter().filter(|a| a.contains("calc.exe")).count(), 1);
    }

    /// `tmpl` as yt-dlp fills it when it knows nothing: a `j` field gives its
    /// `|null`, a number its `|0`, a text `NA`.
    fn filled(tmpl: &str) -> String {
        let (_, mut rest) = tmpl.split_once(':').unwrap();
        let mut out = String::new();
        while let Some(i) = rest.find("%(") {
            out.push_str(&rest[..i]);
            let end = rest[i..].find(')').unwrap() + i;
            let field = &rest[i + 2..end];
            out.push_str(match (rest.as_bytes()[end + 1], field.split_once('|')) {
                (b'j', Some((_, default))) => default,
                (b'j', None) => "NA",
                (_, Some((_, default))) => default,
                (_, None) => "NA",
            });
            rest = &rest[end + 2..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn templates_print_json_even_when_yt_dlp_knows_nothing() {
        for (tmpl, when) in [(PROGRESS, "download:"), (STEP, "postprocess:"), (INFO, "before_dl:"), (DONE, "after_move:")] {
            assert!(tmpl.starts_with(when), "{tmpl}");
            let line = filled(tmpl);
            assert!(serde_json::from_str::<Value>(&line).is_ok(), "{line}");
        }
        assert_eq!(parse(&filled(PROGRESS)), Some(Line::Progress { done: 0, total: None, speed: None, eta_s: None }));
        assert_eq!(parse(&filled(STEP)), None, "a step called NA");
        assert_eq!(parse(&filled(INFO)), Some(Line::Info { title: String::new(), site: String::new(), thumbnail: None, duration_s: None }));
        assert_eq!(parse(&filled(DONE)), None, "no file, no path");
    }

    const BEST: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/peertube_best.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_best.stderr"));
    const ALREADY: (&str, &str) =
        (include_str!("../tests/fixtures/ytdlp/peertube_already_downloaded.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_already_downloaded.stderr"));
    const MERGE: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/dash_merge.stdout"), include_str!("../tests/fixtures/ytdlp/dash_merge.stderr"));
    const DASH_AUDIO: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/dash_audio_only_m4a.stdout"), include_str!("../tests/fixtures/ytdlp/dash_audio_only_m4a.stderr"));
    const HLS_AUDIO: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/hls_ts_audio_m4a.stdout"), include_str!("../tests/fixtures/ytdlp/hls_ts_audio_m4a.stderr"));
    const FILM_AUDIO: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/peertube_audio_m4a.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_audio_m4a.stderr"));
    const STOPPED: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/archive_interrupted.stdout"), include_str!("../tests/fixtures/ytdlp/archive_interrupted.stderr"));
    const RESUMED: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/archive_resumed.stdout"), include_str!("../tests/fixtures/ytdlp/archive_resumed.stderr"));
    const HLS_STOPPED: (&str, &str) =
        (include_str!("../tests/fixtures/ytdlp/peertube_hls_interrupted.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_hls_interrupted.stderr"));
    const HLS_RESUMED: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/peertube_hls_resumed.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_hls_resumed.stderr"));
    const PLAYLIST: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/peertube_playlist.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_playlist.stderr"));
    const DIRECT: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/generic_direct_file.stdout"), include_str!("../tests/fixtures/ytdlp/generic_direct_file.stderr"));
    const QUIET: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/original_flags_quiet.stdout"), include_str!("../tests/fixtures/ytdlp/original_flags_quiet.stderr"));
    const NA: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/original_template_na.stdout"), include_str!("../tests/fixtures/ytdlp/original_template_na.stderr"));
    const NO_NEWLINE: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/nonewline_nocolors.stdout"), include_str!("../tests/fixtures/ytdlp/nonewline_nocolors.stderr"));
    const JSON: (&str, &str) = (include_str!("../tests/fixtures/ytdlp/peertube_json.stdout"), include_str!("../tests/fixtures/ytdlp/peertube_json.stderr"));

    /// A recorded run's lines, parsed: stdout's, then stderr's (the order
    /// between the two wasn't recorded).
    fn lines((stdout, stderr): (&str, &str)) -> Vec<Line> {
        stdout.split('\n').chain(stderr.split('\n')).filter_map(parse).collect()
    }

    fn progress_of(lines: &[Line]) -> Vec<(u64, Option<u64>)> {
        lines.iter().filter_map(|l| if let Line::Progress { done, total, .. } = l { Some((*done, *total)) } else { None }).collect()
    }

    fn count(lines: &[Line], line: &Line) -> usize {
        lines.iter().filter(|l| *l == line).count()
    }

    fn done(path: &str) -> Line {
        Line::Done { path: path.into() }
    }

    fn overgrown() -> Line {
        Line::Info { title: OVERGROWN.into(), site: "PeerTube".into(), thumbnail: Some(OVERGROWN_THUMB.into()), duration_s: Some(70.0) }
    }

    #[test]
    fn a_peertube_film_comes_over_hls_with_an_estimated_size() {
        let got = lines(BEST);
        assert_eq!(got[0], overgrown());
        // HLS never knows its size: the estimate stands in from the first line.
        assert_eq!(got[1], Line::Progress { done: 1024, total: Some(25_056), speed: Some(881.522596475822), eta_s: None });
        let progress = progress_of(&got);
        assert_eq!(progress.len(), 60);
        assert!(progress.iter().all(|(_, total)| total.is_some()));
        assert!(progress.windows(2).all(|w| w[1].0 >= w[0].0), "one part: the bytes only grow");
        assert_eq!(progress[59], (23_256_496, Some(23_256_496)));
        assert_eq!(count(&got, &Line::Stage(Stage::Fixing)), 1, "FixupM3u8; MoveFiles is no step");
        assert_eq!(got.last(), Some(&done("<dir>/OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4")));
        assert!(!got.iter().any(|l| matches!(l, Line::Failed(_))), "PeerTube's HTTP Error 400 is a warning");
    }

    #[test]
    fn a_file_there_already_is_all_done() {
        let path = "<dir>/OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4";
        // Its finished line says done 0 of 23 MB.
        assert_eq!(lines(ALREADY), [overgrown(), Line::Progress { done: 23_256_496, total: Some(23_256_496), speed: None, eta_s: None }, done(path)]);
    }

    #[test]
    fn a_merge_downloads_two_parts_then_joins_them() {
        let got = lines(MERGE);
        assert_eq!(got[0], Line::Info { title: "bbb_30fps".into(), site: "Generic".into(), thumbnail: None, duration_s: None });
        let progress = progress_of(&got);
        assert_eq!(progress.len(), 120);
        assert_eq!(progress.windows(2).filter(|w| w[1].0 < w[0].0).count(), 1, "the sound starts again from 0");
        // The step's start and yt-dlp's own line, after both parts.
        let merging = got.iter().position(|l| *l == Line::Stage(Stage::Merging)).unwrap();
        assert_eq!(count(&got, &Line::Stage(Stage::Merging)), 2);
        assert!(got[..merging].iter().filter(|l| matches!(l, Line::Progress { .. })).count() == 120);
        assert_eq!(got.last(), Some(&done("<dir>/bbb_30fps [bbb_30fps].mp4")));
    }

    #[test]
    fn sound_only_files_are_fixed_and_kept_as_they_are() {
        let got = lines(DASH_AUDIO);
        // FixupM4a and FixupDuplicateMoov, each as a step and in its own words;
        // ExtractAudio starts but doesn't convert an M4A.
        assert_eq!(count(&got, &Line::Stage(Stage::Fixing)), 4);
        assert_eq!(count(&got, &Line::Stage(Stage::Converting)), 1);
        assert_eq!(got.last(), Some(&done("<dir>/bbb_30fps [bbb_30fps].m4a")));
        assert_eq!(progress_of(&got).last(), Some(&(5_278_733, Some(5_278_733))));
    }

    #[test]
    fn extracting_the_sound_reads_as_converting() {
        // Recorded with `-x`, which our arguments don't use: the lines still read.
        for (run, path) in [
            (HLS_AUDIO, "<dir>/x36xhzz [x36xhzz].m4a"),
            (FILM_AUDIO, "<dir>/OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].m4a"),
        ] {
            let got = lines(run);
            // The step's start and "[ExtractAudio] Destination: …".
            assert_eq!(count(&got, &Line::Stage(Stage::Converting)), 2, "{path}");
            let fixing = got.iter().position(|l| *l == Line::Stage(Stage::Fixing)).unwrap();
            let converting = got.iter().position(|l| *l == Line::Stage(Stage::Converting)).unwrap();
            assert!(fixing < converting, "FixupM3u8 first");
            assert_eq!(got.last(), Some(&done(path)));
        }
        assert_eq!(lines(HLS_AUDIO)[0], Line::Info { title: "x36xhzz".into(), site: "Generic".into(), thumbnail: None, duration_s: Some(634.0) });
    }

    #[test]
    fn a_stopped_download_carries_on_where_it_stopped() {
        let stopped = lines(STOPPED);
        let archive = Line::Info {
            title: "Big Buck Bunny".into(),
            // yt-dlp's name for it then ("ArchiveOrg"); ours says "archive.org".
            site: "ArchiveOrg".into(),
            thumbnail: Some("https://archive.org/download/BigBuckBunny_124/BigBuckBunny_124.thumbs/Content/big_buck_bunny_720p_surround_000585.jpg".into()),
            duration_s: Some(596.46),
        };
        assert_eq!(stopped[0], archive);
        assert_eq!(progress_of(&stopped).last(), Some(&(37_747_712, Some(46_935_223))));
        assert_eq!(stopped.last(), Some(&Line::Failed(Problem::Other("Interrupted by user".into()))));
        assert!(!stopped.iter().any(|l| matches!(l, Line::Done { .. })));
        let resumed = lines(RESUMED);
        assert_eq!(progress_of(&resumed)[0], (37_748_736, Some(46_935_223)), "from the byte it had");
        assert_eq!(resumed.last(), Some(&done("<dir>/Big Buck Bunny [BigBuckBunny_124].ogv")));
        // HLS carries on from its fragments.
        assert_eq!(lines(HLS_STOPPED).last(), Some(&Line::Failed(Problem::Other("Interrupted by user".into()))));
        let hls = lines(HLS_RESUMED);
        assert_eq!(progress_of(&hls)[0], (1_915_276, Some(11_493_864)));
        assert_eq!(progress_of(&hls).last(), Some(&(23_257_889, Some(23_257_889))));
    }

    #[test]
    fn a_playlist_says_each_item_and_its_file() {
        let got = lines(PLAYLIST);
        let titles: Vec<&str> = got.iter().filter_map(|l| if let Line::Info { title, .. } = l { Some(title.as_str()) } else { None }).collect();
        assert_eq!(
            titles,
            [
                "Compositor updates for Blender 4.2 #b3d #software #animation #compositing",
                "What are some of the things you are looking forward to the most from Blender 4.2 Eevee? #b3d #eevee",
            ]
        );
        let files: Vec<&Line> = got.iter().filter(|l| matches!(l, Line::Done { .. })).collect();
        assert_eq!(files.len(), 2);
        assert_eq!(
            *files[1],
            done("<dir>/What are some of the things you are looking forward to the most from Blender 4.2 Eevee？ #b3d #eevee [1gw3XpwmiDXAsXshQrH7mo].mp4")
        );
        assert_eq!(count(&got, &Line::Stage(Stage::Fixing)), 2, "Concat at the end is no step");
    }

    #[test]
    fn a_direct_link_knows_its_size() {
        let got = lines(DIRECT);
        assert_eq!(progress_of(&got)[0], (1024, Some(2_799_967)));
        assert_eq!(got.last(), Some(&done("<dir>/1effe9c9-882d-4eec-8469-f3445d319cd2-0 [1effe9c9-882d-4eec-8469-f3445d319cd2-0].mp4")));
    }

    #[test]
    fn older_lines_still_read() {
        // Before `"st"`: the progress reads the same.
        let quiet = lines(QUIET);
        assert_eq!(quiet[0], overgrown());
        assert_eq!(progress_of(&quiet), [(1024, Some(2_799_967)), (3072, Some(2_799_967)), (7168, Some(2_799_967)), (10_241, Some(2_799_967)), (10_241, Some(10_241))]);
        // Before `|null`: NA where yt-dlp knew nothing.
        assert_eq!(lines(NA)[0], Line::Info { title: "bbb_30fps".into(), site: "Generic".into(), thumbnail: None, duration_s: None });
        // `-J`'s one line is no line of ours.
        assert_eq!(lines(JSON), []);
    }

    #[test]
    fn a_console_line_reads_as_it_shows() {
        // Without `--newline` (and with `--no-colors`), each progress line
        // redraws the last one: `\r\x1b[K{…}\r\x1b[K{…}…\r`.
        let got = lines(NO_NEWLINE);
        assert_eq!(got[1], Line::Progress { done: 10_241, total: Some(10_241), speed: Some(18352.165473349243), eta_s: None });
        assert_eq!(got.len(), 3);
        // Read at each `\r` instead, as the runner does: every one of them.
        let each: Vec<Line> = NO_NEWLINE.0.split(|c| c == '\n' || c == '\r').filter_map(parse).collect();
        assert_eq!(progress_of(&each).len(), 5);
        // A Windows line ending, and a `\r` before a warning.
        assert_eq!(parse("{\"t\":\"done\",\"path\":\"C:\\\\Users\\\\Luca Cada\\\\Downloads\\\\x.mp4\"}\r"), Some(done(r"C:\Users\Luca Cada\Downloads\x.mp4")));
        assert_eq!(parse("\rWARNING: [youtube] dQw4w9WgXcQ: nsig extraction failed: Some formats may be missing; Re-extracting data"), None);
        assert_eq!(parse("\x1b[0;33mWARNING:\x1b[0m some formats may be missing"), None);
    }

    #[test]
    fn reading_at_each_cr_too_changes_nothing() {
        // The runner ends a line at `\r` as well as `\n`.
        for run in [BEST, ALREADY, MERGE, DASH_AUDIO, HLS_AUDIO, FILM_AUDIO, STOPPED, RESUMED, HLS_STOPPED, HLS_RESUMED, PLAYLIST, DIRECT, QUIET, NA, JSON] {
            let each: Vec<Line> = run.0.split(|c| c == '\n' || c == '\r').chain(run.1.split(|c| c == '\n' || c == '\r')).filter_map(parse).collect();
            assert_eq!(each, lines(run));
        }
    }

    #[test]
    fn recorded_failures_say_why() {
        for (stderr, problem) in [
            (include_str!("../tests/fixtures/ytdlp/error_unsupported_url.stderr"), Problem::Unsupported),
            // A page PeerTube doesn't have is its app's page: no video there.
            (include_str!("../tests/fixtures/ytdlp/error_page_404.stderr"), Problem::Unsupported),
            (include_str!("../tests/fixtures/ytdlp/error_peertube_404.stderr"), Problem::NotFound),
            (include_str!("../tests/fixtures/ytdlp/error_dns.stderr"), Problem::Network),
            (include_str!("../tests/fixtures/ytdlp/error_format_unavailable.stderr"), Problem::Unsupported),
            (include_str!("../tests/fixtures/ytdlp/error_not_a_url.stderr"), Problem::Unsupported),
            // A playlist's private item comes as https://video.blender.org/w/None.
            (include_str!("../tests/fixtures/ytdlp/peertube_playlist_private_item.stderr"), Problem::Unsupported),
            (include_str!("../tests/fixtures/ytdlp/error_bad_option.stderr"), Problem::Other("no such option: --no-such-flag".into())),
        ] {
            let got: Vec<Line> = stderr.split('\n').filter_map(parse).collect();
            assert_eq!(got, [Line::Failed(problem)], "{stderr}");
        }
        for stdout in [
            include_str!("../tests/fixtures/ytdlp/error_unsupported_url.stdout"),
            include_str!("../tests/fixtures/ytdlp/error_dns.stdout"),
            include_str!("../tests/fixtures/ytdlp/error_peertube_404.stdout"),
            include_str!("../tests/fixtures/ytdlp/version.stdout"),
        ] {
            assert_eq!(stdout.split('\n').filter_map(parse).count(), 0, "{stdout}");
        }
    }

    fn failed(line: &str) -> Option<Problem> {
        match parse(line) {
            Some(Line::Failed(p)) => Some(p),
            other => panic!("{line:?} gave {other:?}"),
        }
    }

    #[test]
    fn failures_recorded_on_this_server() {
        // yt-dlp 2026.10.26 on Linux, 29 Oct 2026: a page on a closed port,
        // an address that never answers (TEST-NET-1), a file that isn't there
        // and a closed port once downloading (`--load-info-json`, `--retries 1`).
        for (line, problem) in [
            (
                "ERROR: [generic] video: Unable to download webpage: HTTPConnection(host='127.0.0.1', port=9): Failed to establish a new connection: [Errno 111] Connection refused (caused by TransportError(\"HTTPConnection(host='127.0.0.1', port=9): Failed to establish a new connection: [Errno 111] Connection refused\"))",
                Problem::Network,
            ),
            (
                "ERROR: [generic] video: Unable to download webpage: (<HTTPConnection(host='192.0.2.1', port=80) at 0x7695f291b620>, 'Connection to 192.0.2.1 timed out. (connect timeout=3.0)') (caused by TransportError(\"(<HTTPConnection(host='192.0.2.1', port=80) at 0x7695f291b620>, 'Connection to 192.0.2.1 timed out. (connect timeout=3.0)')\"))",
                Problem::Network,
            ),
            ("ERROR: [generic] nope: Unable to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)", Problem::NotFound),
            ("ERROR: unable to download video data: HTTP Error 404: Not Found", Problem::NotFound),
            ("ERROR: Unsupported URL: https://video.blender.org/about/instance", Problem::Unsupported),
        ] {
            assert_eq!(failed(line), Some(problem), "{line}");
        }
        // Giving up after its retries, yt-dlp writes a `\r` into the error.
        let gave_up = "ERROR: \r[download] Got error: HTTPConnection(host='127.0.0.1', port=9): Failed to establish a new connection: [Errno 111] Connection refused. Giving up after 1 retries";
        assert_eq!(parse(gave_up), Some(Line::Failed(Problem::Network)));
        // Read at the `\r`, as the runner does, the reason is on its second line.
        let (first, second) = gave_up.split_once('\r').unwrap();
        assert_eq!((parse(first), parse(second)), (None, Some(Line::Failed(Problem::Network))));
        // Its retries before that are no failure.
        assert_eq!(parse("[download] Got error: HTTPConnection(host='127.0.0.1', port=9): Failed to establish a new connection: [Errno 111] Connection refused. Retrying (1/1)..."), None);
        assert_eq!(parse("[download] Got error: HTTP Error 503: Service Unavailable. Retrying fragment 3 (2/10)..."), None);
        assert_eq!(
            parse("ERROR: \r[download] Got error: HTTP Error 403: Forbidden. Giving up after 10 retries"),
            Some(Line::Failed(Problem::Other("HTTP Error 403: Forbidden".into())))
        );
    }

    #[test]
    fn what_sites_and_windows_say() {
        // YouTube's words (the server's reasons, yt_dlp/extractor/youtube/_video.py),
        // as yt-dlp prints them; this server can't reach YouTube to record them.
        let cookies = "Use --cookies-from-browser or --cookies for the authentication. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies. Also see  https://github.com/yt-dlp/yt-dlp/wiki/Extractors#exporting-youtube-cookies  for tips on effectively exporting YouTube cookies";
        for (line, problem) in [
            (format!("ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you’re not a bot. {cookies}"), Problem::SignIn),
            (format!("ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm your age. This video may be inappropriate for some users. {cookies}"), Problem::SignIn),
            (format!("ERROR: [youtube] dQw4w9WgXcQ: Private video. Sign in if you've been granted access to this video. {cookies}"), Problem::Private),
            ("ERROR: [youtube] dQw4w9WgXcQ: Join this channel to get access to members-only content like this video, and other exclusive perks.".into(), Problem::Private),
            ("ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. This video has been removed by the uploader".into(), Problem::NotFound),
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. This video is no longer available because the YouTube account associated with this video has been terminated.".into(),
                Problem::NotFound,
            ),
            ("ERROR: [youtube] dQw4w9WgXcQ: The uploader has not made this video available in your country".into(), Problem::Region),
            ("ERROR: [youtube] dQw4w9WgXcQ: This video is DRM protected".into(), Problem::Protected),
            (
                "ERROR: [youtube] dQw4w9WgXcQ: This content isn't available, try again later. The current session has been rate-limited by YouTube for up to an hour. It is recommended to use `-t sleep` to add a delay between video requests to avoid exceeding the rate limit. For more information, refer to  https://github.com/yt-dlp/yt-dlp/wiki/Extractors#this-content-isnt-available-try-again-later".into(),
                Problem::Limited,
            ),
            ("ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. YouTube is requiring a captcha challenge before playback".into(), Problem::Limited),
            // Any site (yt_dlp/extractor/common.py, yt_dlp/extractor/unsupported.py).
            ("ERROR: [vimeo] 76979871: This video is not available from your location due to geo restriction".into(), Problem::Region),
            ("ERROR: [vimeo] 76979871: This video is only available for registered users. Use --cookies, --cookies-from-browser, --username and --password".into(), Problem::SignIn),
            ("ERROR: [Netflix] 80117470: The requested site is known to use DRM protection. It will NOT be supported.".into(), Problem::Protected),
            ("ERROR: unable to download video data: HTTP Error 429: Too Many Requests".into(), Problem::Limited),
            ("ERROR: [generic] clip: Unable to download webpage: HTTP Error 410: Gone (caused by <HTTPError 410: Gone>)".into(), Problem::NotFound),
            // Windows' own words for the network (Python passes them on).
            (
                "ERROR: [generic] clip: Unable to download webpage: HTTPSConnection(host='downloads.example.invalid', port=443): Failed to resolve 'downloads.example.invalid' ([Errno 11001] getaddrinfo failed) (caused by TransportError(…))".into(),
                Problem::Network,
            ),
            ("ERROR: [generic] clip: Unable to download webpage: [WinError 10061] No connection could be made because the target machine actively refused it".into(), Problem::Network),
            (
                "ERROR: unable to download video data: [WinError 10060] A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond".into(),
                Problem::Network,
            ),
            ("ERROR: \r[download] Got error: [WinError 10054] An existing connection was forcibly closed by the remote host. Giving up after 10 retries".into(), Problem::Network),
            ("ERROR: [generic] clip: Unable to download webpage: <urlopen error [Errno 11001] getaddrinfo failed>".into(), Problem::Network),
        ] {
            assert_eq!(failed(&line), Some(problem), "{line}");
        }
        // A geographic block carries on over two lines of its own, which are no failure.
        assert_eq!(parse("This video is available in Germany, Austria."), None);
        assert_eq!(parse("You might want to use a VPN or a proxy server (with --proxy) to workaround."), None);
    }

    #[test]
    fn anything_else_is_said_in_yt_dlps_words_trimmed() {
        let other = |line: &str| match failed(line) {
            Some(Problem::Other(said)) => said,
            p => panic!("{line}: {p:?}"),
        };
        assert_eq!(other("ERROR: [youtube] dQw4w9WgXcQ: This live event will begin in a few moments."), "This live event will begin in a few moments.");
        assert_eq!(
            other("ERROR: [generic] watch?v=1: Unable to download webpage: HTTP Error 500: Internal Server Error (caused by <HTTPError 500: Internal Server Error>)"),
            "Unable to download webpage: HTTP Error 500: Internal Server Error"
        );
        assert_eq!(
            other("ERROR: [vimeo] 76979871: Unable to extract info section; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U"),
            "Unable to extract info section"
        );
        // No id: nothing taken off the front.
        assert_eq!(other("ERROR: [generic] Unable to read the page's video list"), "Unable to read the page's video list");
        assert_eq!(other("ERROR: Postprocessing: Conversion failed!"), "Postprocessing: Conversion failed!");
        let long = other(&format!("ERROR: [generic] clip: {}", "Ünïcødé the site says something very long about why ".repeat(5)));
        assert_eq!(long.chars().count(), 120);
        assert!(long.ends_with('…') && long.starts_with("Ünïcødé the site"), "{long}");
        // An error yt-dlp left empty says nothing on its own.
        assert_eq!(parse("ERROR:"), None);
        assert_eq!(parse("ERROR:   "), None);
    }

    #[test]
    fn steps_in_yt_dlps_own_words() {
        for (line, stage) in [
            (r#"[Merger] Merging formats into "C:\Users\Luca Cada\Downloads\Fête des Lumières — 日本語 [abc123].mp4""#, Some(Stage::Merging)),
            (r"[ExtractAudio] Destination: C:\Users\Luca Cada\Downloads\Fête [abc123] (audio).m4a", Some(Stage::Converting)),
            (r"[ExtractAudio] Not converting audio C:\Users\Luca Cada\Downloads\Fête [abc123] (audio).m4a; could not find a mapping for m4a", None),
            (r#"[FixupM4a] Correcting container of "C:\Users\Luca Cada\Downloads\x [abc123] (audio).m4a""#, Some(Stage::Fixing)),
            (r#"[FixupM3u8] Fixing MPEG-TS in MP4 container of "/tmp/OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4""#, Some(Stage::Fixing)),
            (r#"[FixupDuplicateMoov] Fixing duplicate MOOV atoms of "x.m4a""#, Some(Stage::Fixing)),
            (r#"[MoveFiles] Moving file "a" to "b""#, None),
            (r"[download] Destination: C:\Users\Luca Cada\Downloads\x.mp4", None),
            ("[download] C:\\Users\\Luca Cada\\Downloads\\x.mp4 has already been downloaded", None),
            ("[info] b29290cc-dc51-4a12-bcb2-2aa5fece7605: Downloading 1 format(s): 6", None),
            ("[PeerTube] b29290cc-dc51-4a12-bcb2-2aa5fece7605: Downloading video JSON", None),
            (r"Deleting original file C:\Users\Luca Cada\Downloads\x.f137.mp4 (pass -k to keep)", None),
            ("WARNING: [PeerTube] b29290cc-dc51-4a12-bcb2-2aa5fece7605: Unable to download webpage: HTTP Error 400: Bad Request", None),
            ("WARNING: Unable to extract metadata: ffprobe not found. Please install or provide the path using --ffmpeg-location", None),
            ("", None),
            ("{not json", None),
            (r#"{"t":"pp","id":"x","pp":"Merger","st":"finished"}"#, None),
            (r#"{"t":"later","id":"x"}"#, None),
        ] {
            assert_eq!(parse(line), stage.map(Line::Stage), "{line}");
        }
    }

    #[test]
    fn info_and_files_keep_their_letters() {
        // Recorded with these arguments and our FFmpeg (29 Oct 2026), joining
        // two local files (`--load-info-json`): `j` writes non-ASCII as \u
        // escapes, yt-dlp's own lines as UTF-8, and the name swaps `"` for `＂`.
        let info = r#"{"t":"info","id":"local-mp4","title":"F\u00eate \u2014 \u65e5\u672c\u8a9e \"merge\" test","site":"generic","domain":"example.invalid","thumb":null,"duration":3}"#;
        assert_eq!(
            parse(info),
            Some(Line::Info { title: "Fête — 日本語 \"merge\" test".into(), site: "example.invalid".into(), thumbnail: None, duration_s: Some(3.0) })
        );
        assert_eq!(parse(r#"[Merger] Merging formats into "<dir>/Fête — 日本語 ＂merge＂ test [local-mp4].mp4""#), Some(Line::Stage(Stage::Merging)));
        assert_eq!(
            parse(r#"{"t":"done","path":"<dir>/F\u00eate \u2014 \u65e5\u672c\u8a9e \uff02merge\uff02 test [local-mp4].mp4"}"#),
            Some(done("<dir>/Fête — 日本語 ＂merge＂ test [local-mp4].mp4"))
        );
        // The sound alone, under its own name (Audio on the Blender film).
        assert_eq!(
            parse(r#"{"t":"done","path":"<dir>/OVERGROWN\uff1a a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605] (audio).mp4"}"#),
            Some(done("<dir>/OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605] (audio).mp4"))
        );
        // The site's own names, as the preview says them.
        let site = |site: &str, domain: &str| match parse(&format!(r#"{{"t":"info","title":"x","site":"{site}","domain":"{domain}","thumb":null,"duration":null}}"#)) {
            Some(Line::Info { site, .. }) => site,
            other => panic!("{other:?}"),
        };
        assert_eq!(site("youtube", "youtube.com"), "YouTube");
        assert_eq!(site("archive.org", "archive.org"), "Internet Archive");
        assert_eq!(site("PeerTube", "video.blender.org"), "PeerTube");
        // A picture that isn't on the web isn't fetched.
        assert!(matches!(parse(r#"{"t":"info","title":"x","site":"generic","thumb":"file:///C:/Windows/win.ini"}"#), Some(Line::Info { thumbnail: None, .. })));
        assert_eq!(
            parse(r#"{"t":"done","path":"C:\\Users\\Luca Cada\\Downloads\\F\u00eate \u65e5\u672c\u8a9e\uff1a live [abc123] (audio).m4a"}"#),
            Some(done(r"C:\Users\Luca Cada\Downloads\Fête 日本語： live [abc123] (audio).m4a"))
        );
    }

    #[test]
    fn progress_numbers() {
        let p = |json: &str| parse(json).unwrap();
        // A float `eta` (HLS), rounded.
        assert_eq!(
            p(r#"{"t":"dl","id":"x","st":"downloading","done":2961804,"total":0,"est":27414360.0,"speed":662782.4887582941,"eta":68.66347540461298,"frag":2,"frags":18}"#),
            Line::Progress { done: 2_961_804, total: Some(27_414_360), speed: Some(662782.4887582941), eta_s: Some(69) }
        );
        // Nothing known yet.
        assert_eq!(
            p(r#"{"t":"dl","id":"x","st":"downloading","done":0,"total":0,"est":0,"speed":0,"eta":0,"frag":0,"frags":0}"#),
            Line::Progress { done: 0, total: None, speed: None, eta_s: None }
        );
        // `null`s instead of 0s read the same.
        assert_eq!(
            p(r#"{"t":"dl","st":"downloading","done":1024,"total":null,"est":null,"speed":null,"eta":null}"#),
            Line::Progress { done: 1024, total: None, speed: None, eta_s: None }
        );
        // Finished with no size known: what came is all of it.
        assert_eq!(p(r#"{"t":"dl","st":"finished","done":705,"total":0,"est":0,"speed":84331.06,"eta":0}"#), Line::Progress { done: 705, total: Some(705), speed: Some(84331.06), eta_s: None });
    }

    #[test]
    fn messages_for_the_page() {
        for (problem, says) in [
            (Problem::Unsupported, "No video or sound found at this link"),
            (Problem::NotFound, "This video isn't there any more"),
            (Problem::Private, "This video is private"),
            (Problem::SignIn, "The site wants you signed in"),
            (Problem::Region, "Not available in your country"),
            (Problem::Limited, "The site is limiting downloads from this network for now. Try again later"),
            (Problem::Network, "Can't connect. Check your internet"),
            (Problem::Protected, "Protected by the site: it can't be saved"),
            (Problem::Other("HTTP Error 403: Forbidden".into()), "HTTP Error 403: Forbidden"),
            (Problem::Other(" yt-dlp couldn't start. ".into()), "yt-dlp couldn't start."),
            (Problem::Other(String::new()), "The download failed"),
        ] {
            assert_eq!(problem.message(), says);
        }
    }
}
