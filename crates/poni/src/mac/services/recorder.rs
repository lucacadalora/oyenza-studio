//! Oyenza Studio Recorder on the Mac: the separate recorder app built from
//! Cap, started through its own link (`poni_core::recorder`). Pausing and
//! stopping stay on the recorder's own bar. The Screen recorder add-on
//! installs it from its disk image and moves it to the Trash
//! ([`install_job`], [`remove_job`]) on the add-on thread. Nothing here runs
//! at rest.

use crate::events::{AppEvent, EventSink};
use crate::services::addons::{self, Job};
use crate::services::http::Client;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::{NSFileManager, NSString, NSURL};
use poni_core::addons::{Status, RECORDER};
use poni_core::recorder;
use std::path::{Path, PathBuf};
use std::process::Command;

/// macOS's own tool for telling Launch Services about an app (so its link
/// scheme works at once) and for forgetting one.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// Whether some app opens `oyenza-recorder:` links, i.e. the recorder is
/// installed. One in the Trash doesn't count. Read when the menu bar's menu
/// is built or a page that offers Record comes on screen.
pub fn installed() -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&format!("{}://", recorder::SCHEME))) else {
        return false;
    };
    NSWorkspace::sharedWorkspace()
        .URLForApplicationToOpenURL(&url)
        .and_then(|app| app.path())
        .is_some_and(|path| {
            let path = PathBuf::from(path.to_string());
            path.exists() && !path.components().any(|c| c.as_os_str() == ".Trash")
        })
}

/// Record the island's screen with the camera, microphone and system sound
/// the recorder used last time.
pub fn record_screen(mtm: MainThreadMarker) {
    match screen_name(mtm) {
        Some(name) => super::open(&recorder::record_screen_link(&name)),
        None => crate::pwarn!("recorder: the island's screen has no name"),
    }
}

/// The screen the island sits on (`window::pick_screen`: the notch screen,
/// else the menu-bar screen), by the name the recorder gives screens.
fn screen_name(mtm: MainThreadMarker) -> Option<String> {
    let island = crate::window::pick_screen(mtm)?;
    NSScreen::screens(mtm)
        .iter()
        .find(|s| {
            let f = s.frame();
            f.origin.x == island.x && f.origin.y == island.y && f.size.width == island.w && f.size.height == island.h
        })
        .map(|s| s.localizedName().to_string())
}

/// Where the add-on puts the app: the Applications folder, or the user's own
/// (`~/Applications`) when the account can't write to the shared one.
fn destinations() -> Vec<PathBuf> {
    let mut d = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        d.push(PathBuf::from(home).join("Applications"));
    }
    d
}

fn app_name() -> String {
    format!("{}.app", recorder::APP_NAME)
}

/// The add-on's switch-on: download the disk image, copy the app out of it
/// into Applications, open it (its first start asks for its permissions),
/// and answer `Ready`, or `Failed`.
pub fn install_job(sink: EventSink) -> Job {
    Box::new(move |client: Result<&Client, &str>| {
        let status = match install(client, &sink) {
            Ok(at) => {
                crate::pinfo!("recorder: installed {} at {}", recorder::VERSION, at.display());
                Status::Ready
            }
            Err(e) => {
                crate::pwarn!("recorder: couldn't install ({e})");
                Status::Failed
            }
        };
        sink.send(AppEvent::Addon { id: RECORDER.id, status });
    })
}

fn install(client: Result<&Client, &str>, sink: &EventSink) -> Result<PathBuf, String> {
    let file = recorder::installer(true);
    let dir = crate::paths::addons_dir().join(RECORDER.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {} ({e})", dir.display()))?;
    let dmg = dir.join(file.name);
    addons::fetch_installer(client?, sink, RECORDER.id, file, &recorder::installer_url(file), &dmg)?;
    sink.send(AppEvent::Addon { id: RECORDER.id, status: Status::Installing });
    let mount = dir.join("mount");
    let _ = std::fs::create_dir_all(&mount);
    run("hdiutil", &["attach".as_ref(), "-nobrowse".as_ref(), "-readonly".as_ref(), "-noautoopen".as_ref(), "-mountpoint".as_ref(), mount.as_os_str(), dmg.as_os_str()])?;
    let copied = copy_app(&mount);
    if run("hdiutil", &["detach".as_ref(), mount.as_os_str(), "-quiet".as_ref()]).is_err() {
        let _ = run("hdiutil", &["detach".as_ref(), mount.as_os_str(), "-force".as_ref(), "-quiet".as_ref()]);
    }
    let _ = std::fs::remove_dir_all(&dir);
    copied
}

/// Copy the app out of the mounted image into the first Applications folder
/// that takes it (an older copy there goes to the Trash first), and tell
/// Launch Services, so its link works at once.
fn copy_app(mount: &Path) -> Result<PathBuf, String> {
    let source = mount.join(app_name());
    if !source.exists() {
        return Err(format!("the disk image has no {}", app_name()));
    }
    let mut why = String::new();
    for folder in destinations() {
        let _ = std::fs::create_dir_all(&folder);
        let target = folder.join(app_name());
        if target.exists() && trash(&target).is_err() {
            why = format!("an older copy at {} couldn't go to the Trash", target.display());
            continue;
        }
        match run("ditto", &[source.as_os_str(), target.as_os_str()]) {
            Ok(()) => {
                let _ = run(LSREGISTER, &["-f".as_ref(), target.as_os_str()]);
                if let Err(e) = run("open", &[target.as_os_str()]) {
                    crate::pwarn!("recorder: installed, but couldn't open it ({e})");
                }
                return Ok(target);
            }
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// The add-on's switch-off: move the app to the Trash (the recordings, in its
/// data folder, stay) and answer `Off`.
pub fn remove_job(sink: EventSink) -> Job {
    Box::new(move |_client: Result<&Client, &str>| {
        let _ = std::fs::remove_dir_all(crate::paths::addons_dir().join(RECORDER.id));
        sink.send(AppEvent::Addon { id: RECORDER.id, status: remove() });
    })
}

/// Every copy in the Applications folders to the Trash, forgotten by Launch
/// Services: `Off`, or `Ready` when one stays.
fn remove() -> Status {
    let mut status = Status::Off;
    for target in destinations().into_iter().map(|d| d.join(app_name())).filter(|t| t.exists()) {
        match trash(&target) {
            Ok(trashed) => {
                crate::pinfo!("recorder: {} moved to the Trash", target.display());
                let _ = run(LSREGISTER, &["-u".as_ref(), trashed.as_os_str()]);
            }
            Err(e) => {
                crate::pwarn!("recorder: couldn't move {} to the Trash ({e})", target.display());
                status = Status::Ready;
            }
        }
    }
    status
}

/// Move `path` to the Trash; returns where it went.
fn trash(path: &Path) -> Result<PathBuf, String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut trashed = None;
    NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, Some(&mut trashed)).map_err(|e| e.localizedDescription().to_string())?;
    Ok(trashed.and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())).unwrap_or_default())
}

/// Run a command to its end; an error says what it printed.
fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("could not run {program} ({e})"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} ended with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real thing, run by hand on CI's Mac (`mac.yml`, input
    /// `recorder_install`): the pinned disk image downloads and checks out,
    /// the app lands in Applications and opens, and moving it to the Trash
    /// takes it away again while it runs.
    #[test]
    #[ignore]
    fn recorder_add_on_installs_and_uninstalls() {
        assert!(recorder::pinned(), "pin a published recorder release first (tools/pin_recorder.py)");
        let client = Client::new("OyenzaStudio-test (+https://github.com/lucacadalora/oyenza-studio)").expect("an HTTP client");
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = EventSink::new(tx);
        let started = std::time::Instant::now();
        let at = install(Ok(&client), &sink).expect("the install");
        println!("installed at {} in {} s", at.display(), started.elapsed().as_secs());
        assert!(at.join("Contents/Info.plist").exists());
        let said: Vec<String> = rx.try_iter().filter_map(|e| if let AppEvent::Addon { status, .. } = e { Some(format!("{status:?}")) } else { None }).collect();
        println!("statuses: {said:?}");
        assert!(said.iter().any(|s| s.starts_with("Downloading")) && said.iter().any(|s| s == "Installing"));
        assert!(!crate::paths::addons_dir().join(RECORDER.id).exists(), "the disk image is deleted once it has run");
        std::thread::sleep(std::time::Duration::from_secs(8));
        assert_eq!(remove(), Status::Off);
        assert!(!at.exists(), "the app went to the Trash");
    }
}
