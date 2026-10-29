//! The Downloader add-on for real, on a real Windows or Mac (CI's Unit tests
//! job and mac.yml's Mac job run it with `--ignored`): the tools come from
//! where the add-on gets them, exactly as `poni_core::addons::DOWNLOADER`
//! pins them for this app, checked by size and SHA-256 and made runnable as
//! the add-on does both. yt-dlp then saves a short Blender Studio film from
//! Blender's own PeerTube, as video and as sound, with the arguments the
//! island gives it (`poni_core::ytdlp::args`), and every line it prints goes
//! through `poni_core::ytdlp::parse`, as the Downloads page reads them:
//!
//! ```text
//! cargo test -p poni --release the_pinned_tools_download_a_real_link -- --ignored --nocapture
//! ```
//!
//! A tool this build doesn't pin yet is left out: FFmpeg's absence (yt-dlp
//! is pointed at an ffmpeg that isn't there, so a copy on the runner is never
//! used either) skips a run that needs it with a warning. The film's site
//! having trouble only warns, like oyenza.trade in the Stocks test; anything
//! else that goes wrong fails.

use crate::services::http::{Client, MtaGuard};
use poni_core::addons::{self, DOWNLOADER};
use poni_core::sha256;
use poni_core::ytdlp::{self, Line, Mode, Problem, Request, Tools};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// OVERGROWN, 70 s from Blender Studio's open movie project, on Blender's
/// own PeerTube: no sign-in, and not YouTube, which turns CI's runners away.
const LINK: &str = "https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605";
/// The add-on's own (`services::addons`).
const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
/// Budget for one tool, as the add-on gives one file.
const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
/// Budget for one download; the film takes seconds.
const RUN_TIMEOUT: Duration = Duration::from_secs(240);

#[test]
#[ignore = "downloads yt-dlp, QuickJS and a short film; CI runs it on a real Windows and Mac"]
fn the_pinned_tools_download_a_real_link() {
    let dir = std::env::temp_dir().join("poni-downloader-live");
    let _ = fs::remove_dir_all(&dir);
    // The tools, and yt-dlp's cache as the add-on keeps one in its own data.
    let (bin, cache) = (dir.join("tools"), dir.join("cache"));
    fs::create_dir_all(&bin).and_then(|_| fs::create_dir_all(&cache)).expect("a temp folder");
    let ffmpeg = fetch(&bin);
    let path = |p: PathBuf| p.to_str().expect("a Unicode temp folder").to_string();
    let (ytdlp, qjs, ffmpeg_exe, cache) = (path(bin.join(addons::YTDLP_EXE)), path(bin.join(addons::QJS_EXE)), path(bin.join(addons::FFMPEG_EXE)), path(cache));
    save_the_film(&Tools { ytdlp: &ytdlp, qjs: &qjs, ffmpeg: &ffmpeg_exe, cache: &cache }, ffmpeg, &dir);
    let _ = fs::remove_dir_all(&dir);
}

/// The add-on's tools from where it gets them, checked as it checks them,
/// into `bin` under its names. True when FFmpeg is among them.
fn fetch(bin: &Path) -> bool {
    let _com = MtaGuard::new();
    let client = Client::new(USER_AGENT).expect("an HTTP client");
    let mut ffmpeg = false;
    for file in DOWNLOADER.files.iter().filter(|f| addons::is_downloader_tool(f.name)) {
        if !file.pinned() {
            println!("::warning::{} isn't pinned in addons::DOWNLOADER yet: the downloads go without it", file.name);
            continue;
        }
        let url = DOWNLOADER.url(file);
        let started = Instant::now();
        let (status, bytes) = client.get_bytes(&url, FETCH_TIMEOUT, file.size + 64 * 1024, |_| {}).unwrap_or_else(|e| panic!("{}: {url}: {e}", file.name));
        assert_eq!(status, 200, "{}: {url}", file.name);
        assert!(file.matches(&bytes), "{} from {url} isn't the pinned file: {} bytes, SHA-256 {}", file.name, bytes.len(), sha256::hex(&sha256::sha256(&bytes)));
        fs::write(bin.join(file.name), &bytes).expect("the tool saves");
        crate::services::addons::make_runnable(&bin.join(file.name)).expect("the tool can run");
        println!("{}: {} bytes from {url} in {:.1} s, as pinned", file.name, bytes.len(), started.elapsed().as_secs_f32());
        ffmpeg |= file.name == addons::FFMPEG_EXE;
    }
    ffmpeg
}

/// The film as video, then as sound, each into its own folder in `dir`.
/// `ffmpeg`: the add-on pins FFmpeg, so a run that needs it must work too.
fn save_the_film(tools: &Tools, ffmpeg: bool, dir: &Path) {
    // The smallest picture the film comes in (8 MB); the sound is about 3 MB,
    // so 70 s of it is well over 500 KB at any quality yt-dlp converts to.
    for (id, mode, max_height, least) in [(1, Mode::Video, Some(480), 1_000_000), (2, Mode::Audio, None, 500_000)] {
        let folder = dir.join(format!("{mode:?}"));
        fs::create_dir_all(&folder).expect("a folder for the film");
        let req = Request { id, url: LINK.to_string(), mode, max_height, folder: folder.to_str().expect("a Unicode temp folder").to_string() };
        let args = ytdlp::args(&req, tools);
        if mode == Mode::Video {
            quickjs_found(tools, &args);
        }
        let run = run(tools.ytdlp, &args);
        println!("{mode:?}: yt-dlp ended with {:?} after {:.1} s, {} lines:", run.code, run.secs, run.lines.len());
        for (raw, line) in &run.lines {
            match line {
                Some(Line::Progress { .. }) => {}
                Some(line) => println!("  {raw}\n    {line:?}"),
                None => println!("  {raw}"),
            }
        }
        if run.code != Some(0) && !ffmpeg && run.lines.iter().any(|(raw, _)| needs_ffmpeg(raw)) {
            println!("::warning::{mode:?} skipped: it needs FFmpeg, which addons::DOWNLOADER doesn't pin yet");
            continue;
        }
        let failed: Vec<&Problem> = run.lines.iter().filter_map(|(_, l)| if let Some(Line::Failed(p)) = l { Some(p) } else { None }).collect();
        // The site's own trouble (its server erroring or turning CI's runner
        // away) says nothing about our code.
        let site_trouble = |p: &Problem| match p {
            Problem::Network | Problem::Limited | Problem::NotFound => true,
            Problem::Other(m) => m.contains("HTTP Error 5") || m.contains("HTTP Error 403"),
            _ => false,
        };
        if let Some(p) = failed.iter().find(|p| site_trouble(p)) {
            println!("::warning::{mode:?}: video.blender.org didn't give the film ({p:?}), so nothing was checked");
            continue;
        }
        assert!(failed.is_empty(), "{mode:?}: yt-dlp failed: {failed:?}");
        assert_eq!(run.code, Some(0), "{mode:?}: yt-dlp's exit code");
        // Where each kind of line is in the output.
        let at = |kind: fn(&Line) -> bool| -> Vec<usize> { run.lines.iter().enumerate().filter(|(_, (_, l))| l.as_ref().is_some_and(kind)).map(|(i, _)| i).collect() };
        let info = at(|l| matches!(l, Line::Info { title, .. } if !title.is_empty()));
        let progress = at(|l| matches!(l, Line::Progress { .. }));
        let done = at(|l| matches!(l, Line::Done { .. }));
        assert!(!info.is_empty(), "{mode:?}: no Info line with a title");
        assert!(!progress.is_empty(), "{mode:?}: no Progress lines");
        assert!(info[0] < progress[0], "{mode:?}: the Info line comes after the first bytes");
        let most = progress.iter().filter_map(|&i| if let Some(Line::Progress { done, .. }) = run.lines[i].1 { Some(done) } else { None }).max();
        assert!(most.is_some_and(|b| b > least), "{mode:?}: the Progress lines count {most:?} bytes at most");
        let Some(&last) = done.last() else { panic!("{mode:?}: no Done line") };
        assert!(progress[progress.len() - 1] < last, "{mode:?}: Progress lines after the Done line");
        let Some(Line::Done { path }) = &run.lines[last].1 else { unreachable!() };
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or_else(|e| panic!("{mode:?}: the Done line's file {path}: {e}"));
        assert!(size > least, "{mode:?}: {path} is only {size} bytes");
        let parent = Path::new(path).parent().and_then(|p| fs::canonicalize(p).ok());
        assert_eq!(parent, Some(fs::canonicalize(&folder).expect("the folder asked for")), "{mode:?}: {path} isn't in the folder asked for");
        println!("{mode:?}: {} Progress lines, then {size} bytes in {path}", progress.len());
    }
}

/// yt-dlp needs QuickJS for YouTube alone, which turns CI's runners away.
/// Its debug header says whether it can run the QuickJS `args` gives it.
fn quickjs_found(tools: &Tools, args: &[String]) {
    let js = format!("quickjs:{}", tools.qjs);
    assert!(args.iter().any(|a| a.ends_with(&js)), "the arguments don't give yt-dlp the add-on's QuickJS ({js})");
    let out = Command::new(tools.ytdlp).args(["--ignore-config", "--verbose", "--no-js-runtimes", "--js-runtimes", &js]).output().expect("yt-dlp starts");
    let err = String::from_utf8_lossy(&out.stderr);
    let found = err.lines().find(|l| l.starts_with("[debug] JS runtimes:")).unwrap_or_else(|| panic!("yt-dlp's debug header has no JS runtimes:\n{err}"));
    println!("{found}");
    assert!(found.contains("quickjs"), "yt-dlp can't run the add-on's QuickJS");
}

/// One yt-dlp run: every line of its stdout and stderr with what `parse`
/// makes of it, and its exit code.
struct Run {
    lines: Vec<(String, Option<Line>)>,
    code: Option<i32>,
    secs: f32,
}

fn run(exe: &str, args: &[String]) -> Run {
    let started = Instant::now();
    let mut child = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap_or_else(|e| panic!("{exe} doesn't start: {e}"));
    let (tx, rx) = mpsc::channel();
    let pipes: [Box<dyn Read + Send>; 2] = [Box::new(child.stdout.take().expect("stdout")), Box::new(child.stderr.take().expect("stderr"))];
    for pipe in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            // A lone \r (a line written over the last) ends a line too.
            for chunk in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                for line in chunk.split(|&b| b == b'\r').filter(|l| !l.is_empty()) {
                    let _ = tx.send(String::from_utf8_lossy(line).into_owned());
                }
            }
        });
    }
    drop(tx);
    let mut lines = Vec::new();
    loop {
        match rx.recv_timeout(RUN_TIMEOUT.saturating_sub(started.elapsed())) {
            Ok(line) => {
                let parsed = ytdlp::parse(&line);
                lines.push((line, parsed));
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let last: Vec<&str> = lines.iter().rev().take(10).map(|(l, _)| l.as_str()).collect();
                panic!("yt-dlp still running after {} s; its last lines, newest first:\n{}", RUN_TIMEOUT.as_secs(), last.join("\n"));
            }
        }
    }
    let code = child.wait().expect("yt-dlp ends").code();
    Run { lines, code, secs: started.elapsed().as_secs_f32() }
}

/// yt-dlp's words for a step it can't take without FFmpeg.
fn needs_ffmpeg(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("ffmpeg") && (line.contains("not found") || line.contains("not installed"))
}

/// The same film through the island's own runner (`services::downloader`):
/// the job object and windowless start (the process group on a Mac), the
/// reader thread and the events the Downloads page gets. Its tools go where the add-on keeps them, under a
/// `PONI_HOME` of its own. A second download, stopped at its first bytes,
/// must end as stopped and leave no unfinished file behind. CI's filter
/// (`the_pinned_tools_download_a_real_link`) runs this one too.
#[test]
#[ignore = "downloads yt-dlp, QuickJS and a short film; CI runs it on a real Windows and Mac"]
fn the_pinned_tools_download_a_real_link_through_the_runner() {
    use crate::events::{AppEvent, EventSink};
    let home = std::env::temp_dir().join("poni-downloader-runner");
    let _ = fs::remove_dir_all(&home);
    std::env::set_var("PONI_HOME", &home);
    let tools = crate::services::addons::file_path(&DOWNLOADER, addons::YTDLP_EXE).parent().expect("the add-on's folder").to_path_buf();
    fs::create_dir_all(&tools).expect("the add-on's folder");
    let ffmpeg = fetch(&tools);
    let folder = crate::paths::media_downloads_dir();
    fs::create_dir_all(&folder).expect("a downloads folder");
    let (tx, rx) = mpsc::channel();
    // No window to wake: the events are read here.
    #[cfg(windows)]
    let sink = EventSink::new(tx, crate::events::SendHwnd(0));
    #[cfg(target_os = "macos")]
    let sink = EventSink::new(tx);
    let runner = super::downloader::DownloaderService::start(sink).expect("the runner starts");
    let request = |id| Request { id, url: LINK.to_string(), mode: Mode::Video, max_height: Some(480), folder: folder.to_str().expect("a Unicode temp folder").to_string() };

    // Saved, start to end.
    runner.run(request(1));
    let (mut info, mut progress, mut done, mut ended) = (false, 0, None, None);
    let deadline = Instant::now() + RUN_TIMEOUT;
    while ended.is_none() {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(AppEvent::Download { id: 1, line }) => match line {
                Line::Info { .. } => info = true,
                Line::Progress { .. } => progress += 1,
                Line::Done { path } => done = Some(path),
                Line::Failed(p) if matches!(p, Problem::Network | Problem::Limited | Problem::NotFound) => {
                    println!("::warning::the runner: video.blender.org didn't give the film ({p:?}), so nothing was checked");
                    return;
                }
                Line::Failed(p) if !ffmpeg && matches!(&p, Problem::Other(m) if m.contains("ffmpeg")) => {
                    println!("::warning::the runner: the film needed FFmpeg, which addons::DOWNLOADER doesn't pin yet");
                    return;
                }
                Line::Failed(p) => panic!("the runner: yt-dlp failed: {p:?}"),
                Line::Stage(_) => {}
            },
            Ok(AppEvent::DownloadEnded { id: 1, ok }) => ended = Some(ok),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("the runner: nothing for {RUN_TIMEOUT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the runner's events stopped"),
        }
    }
    assert!(info && progress > 0, "the runner: Info {info}, {progress} Progress lines");
    assert_eq!(ended, Some(true), "the runner: the download ends well");
    let path = done.expect("the runner: a Done line");
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    assert!(size > 1_000_000 && Path::new(&path).starts_with(&folder), "the runner: {path} ({size} bytes)");
    println!("the runner saved {path} ({size} bytes)");
    let _ = fs::remove_file(&path);

    // Stopped at its first bytes: ends as stopped, nothing unfinished left.
    runner.run(request(2));
    let deadline = Instant::now() + RUN_TIMEOUT;
    let mut stopped = false;
    let ok = loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(AppEvent::Download { id: 2, line: Line::Progress { .. } }) if !stopped => {
                runner.stop(2);
                stopped = true;
            }
            Ok(AppEvent::DownloadEnded { id: 2, ok }) => break ok,
            Ok(_) => {}
            Err(e) => panic!("the runner, stopping: {e:?}"),
        }
    };
    assert!(stopped && !ok, "a stopped download ends as not done");
    let left: Vec<String> = fs::read_dir(&folder).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    assert!(left.iter().all(|n| !n.ends_with(".part") && !n.ends_with(".ytdl") && !n.contains(".part-Frag")), "a stopped download leaves nothing unfinished: {left:?}");
    drop(runner);
    let _ = fs::remove_dir_all(&home);
}
