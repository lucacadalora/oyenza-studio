//! Notification-area icon and its menu (also shown on right-click of the island).

use crate::events::WM_APP_TRAY;
use crate::state::OyenzaLink;
use crate::updates::UpdateLine;
use poni_core::settings::{IdleStyle, Settings, Theme};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    pub fn add(hwnd: HWND) -> Result<Tray> {
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            // Ask for the small-icon size so Windows picks poni.ico's own 16-24 px
            // frames; LoadIconW gives the 32 px one and shrinks it (blurry).
            let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            let icon = LoadImageW(hinst, PCWSTR(1 as _), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
                .map(|h| HICON(h.0))
                .or_else(|_| LoadIconW(hinst, PCWSTR(1 as _)))
                .or_else(|_| LoadIconW(None, IDI_APPLICATION))?;
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_APP_TRAY,
                hIcon: icon,
                ..Default::default()
            };
            let tip: Vec<u16> = format!("Oyenza Studio {}", crate::version::VERSION).encode_utf16().collect();
            nid.szTip[..tip.len()].copy_from_slice(&tip);
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                return Err(windows::core::Error::from_win32());
            }
        }
        Ok(Tray { hwnd })
    }

    pub fn remove(self) {
        unsafe {
            let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: 1, ..Default::default() };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    ToggleIdleStyle(IdleStyle),
    SetTheme(Theme),
    ToggleSystemHistory,
    ToggleLyrics,
    TogglePronunciation,
    /// true: hide in full-screen apps; false: stay on top of them.
    SetHideFullscreen(bool),
    /// Show over games (frame rate, GPU and CPU on the island over a game).
    ToggleGameStats,
    /// Show over games for a few seconds at a time instead of all along.
    ToggleGamePeek,
    ToggleSounds,
    TogglePinned,
    ToggleAutostart,
    /// "Settings…": the Settings window.
    ShowSettings,
    OpenSettings,
    ReloadSettings,
    OpenDataFolder,
    /// "About Oyenza Studio 0.2.3 (…)": opens this version's release notes.
    About,
    /// "Lyrics by LRCLIB": opens lrclib.net.
    LyricsCredit,
    AppleMusicToken,
    AppleMusicHelp,
    AppleMusicForget,
    OyenzaSignIn,
    OyenzaCancel,
    OyenzaDisconnect,
    OyenzaOpen,
    ToggleOyenzaAmounts,
    RecordScreen,
    /// "Update to Oyenza Studio 0.2.5…" (`crate::updates`).
    Update,
    /// "Check for updates…": read the update file now (`crate::updates`).
    CheckForUpdates,
    Quit,
}

pub enum MenuItem {
    Item { label: String, cmd: MenuCmd, checked: bool },
    /// A greyed line that only says something ("Connected as …").
    Info { label: String },
    Sub { label: String, items: Vec<MenuItem> },
    Separator,
}

fn item(label: &str, cmd: MenuCmd, checked: bool) -> MenuItem {
    MenuItem::Item { label: label.to_string(), cmd, checked }
}

/// An idle look as the menu names it. In the Oyen theme the eyes are Oyen's own.
fn idle_label(s: IdleStyle, theme: Theme) -> &'static str {
    match s {
        IdleStyle::Clock => "Clock",
        IdleStyle::Eyes if theme == Theme::Oyen => "Oyen",
        IdleStyle::Eyes => "Eyes",
        IdleStyle::Minimal => "Minimal",
        IdleStyle::Hidden => "Hidden until hover",
        IdleStyle::Status => "Status bar",
    }
}

/// `update`: a later version the update file offers, or Check for updates…
/// (`crate::updates::menu_line`).
pub fn menu_items(s: &Settings, pinned: bool, oyenza: &OyenzaLink, recorder: bool, update: UpdateLine) -> Vec<MenuItem> {
    // The look the theme draws is the one checked: outside Pill, Status shows the clock.
    let shown = crate::pill::status::shown(s.theme, s.idle_style);
    let style = |st: IdleStyle| item(idle_label(st, s.theme), MenuCmd::ToggleIdleStyle(st), shown == st);
    let theme = |label: &str, t: Theme| item(label, MenuCmd::SetTheme(t), s.theme == t);
    let fullscreen = |label: &str, hide: bool| item(label, MenuCmd::SetHideFullscreen(hide), s.hide_in_fullscreen == hide);
    let mut items = vec![
        item("Settings…", MenuCmd::ShowSettings, false),
        MenuItem::Separator,
        item("Keep island open", MenuCmd::TogglePinned, pinned),
        MenuItem::Sub { label: "Theme".into(), items: vec![theme("Classic", Theme::Classic), theme("Oyen", Theme::Oyen), theme("Glass", Theme::Glass), theme("Pill", Theme::Pill)] },
        // The status bar is listed only in Pill, as on the Settings page: the
        // other themes would draw the clock for it.
        MenuItem::Sub { label: "Idle look".into(), items: crate::pill::status::looks(s.theme).iter().map(|l| style(*l)).collect() },
        MenuItem::Separator,
        item("Synced lyrics", MenuCmd::ToggleLyrics, s.lyrics),
        item("Lyrics pronunciation (romaji, pinyin, Korean)", MenuCmd::TogglePronunciation, s.lyrics_pronunciation),
        apple_music_menu(),
        oyenza_menu(s, oyenza),
        item("Sounds", MenuCmd::ToggleSounds, s.sounds),
        MenuItem::Sub {
            label: "In full-screen apps".into(),
            items: vec![
                fullscreen("Hide", true),
                fullscreen("Stay on top", false),
                MenuItem::Separator,
                item("Show FPS, GPU and CPU over games", MenuCmd::ToggleGameStats, s.game_stats),
                item("Only for a few seconds (no added delay)", MenuCmd::ToggleGamePeek, s.game_peek),
            ],
        },
        item("Start with Windows", MenuCmd::ToggleAutostart, crate::autostart::is_enabled()),
        MenuItem::Separator,
        item("Open settings file", MenuCmd::OpenSettings, false),
        item("Reload settings", MenuCmd::ReloadSettings, false),
        item("Open data folder", MenuCmd::OpenDataFolder, false),
        item("Lyrics by LRCLIB", MenuCmd::LyricsCredit, false),
        // Which version and build this is, as the Mac menu says it.
        item(&crate::version::about_label(), MenuCmd::About, false),
        MenuItem::Separator,
        item("Quit Oyenza Studio", MenuCmd::Quit, false),
    ];
    // Only while the System add-on is on: its page's history, under Sounds.
    if s.system_on() {
        let at = items.iter().position(|i| matches!(i, MenuItem::Item { cmd: MenuCmd::ToggleSounds, .. })).unwrap_or(items.len());
        items.insert(at, item("Keep System page history", MenuCmd::ToggleSystemHistory, s.system_history));
    }
    // Only once the separate recorder app is installed: until then the
    // island doesn't mention recording at all.
    if recorder {
        items.splice(0..0, [item("Record this screen", MenuCmd::RecordScreen, false), MenuItem::Separator]);
    }
    match update {
        // A new version first, until it is installed.
        UpdateLine::Offer(v) => {
            items.splice(0..0, [item(&format!("Update to Oyenza Studio {v}…"), MenuCmd::Update, false), MenuItem::Separator]);
        }
        // Otherwise a look on request, under the version line.
        UpdateLine::Check => {
            let at = items.iter().position(|i| matches!(i, MenuItem::Item { cmd: MenuCmd::About, .. })).map_or(items.len(), |i| i + 1);
            items.insert(at, item("Check for updates…", MenuCmd::CheckForUpdates, false));
        }
        UpdateLine::None => {}
    }
    items
}

/// Opt-in lyrics from Apple Music with the user's own token.
fn apple_music_menu() -> MenuItem {
    let on = crate::paths::apple_music_token_path().exists();
    let mut items = vec![
        item("Use token from clipboard", MenuCmd::AppleMusicToken, false),
        item("How to get the token", MenuCmd::AppleMusicHelp, false),
    ];
    if on {
        items.push(MenuItem::Separator);
        items.push(item("Remove token", MenuCmd::AppleMusicForget, false));
    }
    MenuItem::Sub { label: if on { "Apple Music lyrics: on".into() } else { "Apple Music lyrics".into() }, items }
}

/// Oyenza, optional: sign in with a code from oyenza.app, then what the
/// island may show, and signing out.
fn oyenza_menu(s: &Settings, link: &OyenzaLink) -> MenuItem {
    let items = match link {
        OyenzaLink::Off => vec![item("Sign in to Oyenza…", MenuCmd::OyenzaSignIn, false)],
        OyenzaLink::Connecting { .. } => vec![item("Cancel sign-in", MenuCmd::OyenzaCancel, false)],
        OyenzaLink::Connected { account } => vec![
            MenuItem::Info { label: format!("Connected as {account}") },
            item("Show amounts on the island", MenuCmd::ToggleOyenzaAmounts, s.oyenza_show_amounts),
            item("Open oyenza.app", MenuCmd::OyenzaOpen, false),
            MenuItem::Separator,
            item("Disconnect", MenuCmd::OyenzaDisconnect, false),
        ],
    };
    MenuItem::Sub { label: "Oyenza".into(), items }
}

fn build(menu: HMENU, items: &[MenuItem], cmds: &mut Vec<MenuCmd>) {
    for it in items {
        unsafe {
            match it {
                MenuItem::Item { label, cmd, checked } => {
                    cmds.push(*cmd);
                    let id = cmds.len();
                    let w: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                    let flags = MF_STRING | if *checked { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
                }
                MenuItem::Info { label } => {
                    let w: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED | MF_DISABLED, 0, PCWSTR(w.as_ptr()));
                }
                MenuItem::Sub { label, items } => {
                    if let Ok(sub) = CreatePopupMenu() {
                        build(sub, items, cmds);
                        let w: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(w.as_ptr()));
                    }
                }
                MenuItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
    }
}

/// Show the menu at `pt` (screen pixels); returns the chosen command.
pub fn show_menu(owner: HWND, pt: POINT, items: &[MenuItem]) -> Option<MenuCmd> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let mut cmds = Vec::new();
        build(menu, items, &mut cmds);
        // Required so the menu closes when clicking elsewhere; afterwards the
        // previous app gets its focus back.
        let prev = GetForegroundWindow();
        let _ = SetForegroundWindow(owner);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, owner, None);
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let idx = chosen.0 as usize;
        let cmd = if idx == 0 { None } else { cmds.get(idx - 1).copied() };
        let launches = matches!(
            cmd,
            Some(MenuCmd::ShowSettings | MenuCmd::OpenSettings | MenuCmd::OpenDataFolder | MenuCmd::About | MenuCmd::LyricsCredit | MenuCmd::AppleMusicHelp | MenuCmd::OyenzaOpen)
        );
        if !launches && !prev.is_invalid() && prev != owner && IsWindow(prev).as_bool() && GetForegroundWindow() == owner {
            let _ = SetForegroundWindow(prev);
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fullscreen_choices(s: &Settings) -> Vec<(String, MenuCmd, bool)> {
        let items = menu_items(s, false, &OyenzaLink::Off, false, UpdateLine::None);
        let Some(MenuItem::Sub { items, .. }) = items.into_iter().find(|i| matches!(i, MenuItem::Sub { label, .. } if label == "In full-screen apps")) else {
            panic!("no full-screen submenu");
        };
        items
            .into_iter()
            .filter_map(|i| match i {
                MenuItem::Item { label, cmd, checked } => Some((label, cmd, checked)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_menu_says_which_version_this_is() {
        let items = menu_items(&Settings::default(), false, &OyenzaLink::Off, false, UpdateLine::None);
        let labels: Vec<(String, MenuCmd)> = items
            .into_iter()
            .filter_map(|i| match i {
                MenuItem::Item { label, cmd, .. } => Some((label, cmd)),
                _ => None,
            })
            .collect();
        let at = |cmd: MenuCmd| labels.iter().position(|l| l.1 == cmd);
        let about = at(MenuCmd::About).expect("no About line");
        assert!(labels[about].0.starts_with(&format!("About Oyenza Studio {}", env!("CARGO_PKG_VERSION"))), "{}", labels[about].0);
        // Right above Quit, with the lyrics credit kept above it.
        assert_eq!(at(MenuCmd::Quit), Some(about + 1));
        assert_eq!(at(MenuCmd::LyricsCredit), Some(about - 1));
    }

    #[test]
    fn a_new_version_comes_first_until_it_is_installed() {
        let s = Settings::default();
        let first = |update: UpdateLine| match menu_items(&s, false, &OyenzaLink::Off, true, update).into_iter().next() {
            Some(MenuItem::Item { label, cmd, .. }) => Some((label, cmd)),
            _ => None,
        };
        assert_eq!(first(UpdateLine::Offer("0.2.5".into())), Some(("Update to Oyenza Studio 0.2.5…".to_string(), MenuCmd::Update)));
        assert_eq!(first(UpdateLine::None).map(|(_, cmd)| cmd), Some(MenuCmd::RecordScreen));
        assert_eq!(first(UpdateLine::Check).map(|(_, cmd)| cmd), Some(MenuCmd::RecordScreen));
    }

    #[test]
    fn check_for_updates_sits_under_the_version_line() {
        let cmds = |update: UpdateLine| -> Vec<MenuCmd> {
            menu_items(&Settings::default(), false, &OyenzaLink::Off, false, update)
                .into_iter()
                .filter_map(|i| match i {
                    MenuItem::Item { cmd, .. } => Some(cmd),
                    _ => None,
                })
                .collect()
        };
        let items = cmds(UpdateLine::Check);
        let about = items.iter().position(|c| *c == MenuCmd::About).expect("no About line");
        assert_eq!(&items[about..], &[MenuCmd::About, MenuCmd::CheckForUpdates, MenuCmd::Quit]);
        // Not while a version is on offer (its line comes first), nor with the checks off.
        assert!(!cmds(UpdateLine::Offer("0.2.5".into())).contains(&MenuCmd::CheckForUpdates));
        assert!(!cmds(UpdateLine::None).contains(&MenuCmd::CheckForUpdates));
    }

    #[test]
    fn system_history_is_in_the_menu_only_while_system_is_on() {
        let has = |s: &Settings| menu_items(s, false, &OyenzaLink::Off, false, UpdateLine::None).iter().position(|i| matches!(i, MenuItem::Item { cmd: MenuCmd::ToggleSystemHistory, .. }));
        let mut s = Settings::default();
        assert_eq!(has(&s), None, "System is off by default");
        s.set_addon(poni_core::addons::SYSTEM.id, true);
        let at = has(&s).expect("no System history item");
        // Right above Sounds, where it always was.
        let items = menu_items(&s, false, &OyenzaLink::Off, false, UpdateLine::None);
        assert!(matches!(&items[at + 1], MenuItem::Item { cmd: MenuCmd::ToggleSounds, .. }));
    }

    #[test]
    fn record_this_screen_shows_only_with_the_recorder() {
        let s = Settings::default();
        let first = |recorder: bool| match menu_items(&s, false, &OyenzaLink::Off, recorder, UpdateLine::None).into_iter().next() {
            Some(MenuItem::Item { label, cmd, .. }) => Some((label, cmd)),
            _ => None,
        };
        assert_eq!(first(true), Some(("Record this screen".to_string(), MenuCmd::RecordScreen)));
        assert_ne!(first(false).map(|(_, cmd)| cmd), Some(MenuCmd::RecordScreen));
    }

    fn idle_choices(s: &Settings) -> Vec<(String, bool)> {
        let items = menu_items(s, false, &OyenzaLink::Off, false, UpdateLine::None);
        let Some(MenuItem::Sub { items, .. }) = items.into_iter().find(|i| matches!(i, MenuItem::Sub { label, .. } if label == "Idle look")) else {
            panic!("no idle look submenu");
        };
        items
            .into_iter()
            .filter_map(|i| match i {
                MenuItem::Item { label, checked, .. } => Some((label, checked)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn status_bar_is_listed_only_in_the_pill_theme() {
        let pill = Settings { theme: Theme::Pill, idle_style: IdleStyle::Status, ..Settings::default() };
        let labels: Vec<String> = idle_choices(&pill).into_iter().map(|c| c.0).collect();
        assert_eq!(labels, ["Clock", "Eyes", "Minimal", "Hidden until hover", "Status bar"]);
        assert_eq!(idle_choices(&pill).last(), Some(&("Status bar".to_string(), true)));
        // Another theme draws the clock for the status bar: Clock is checked,
        // and Status comes back with Pill.
        for theme in [Theme::Classic, Theme::Oyen, Theme::Glass] {
            let s = Settings { theme, ..pill.clone() };
            let choices = idle_choices(&s);
            assert_eq!(choices.len(), 4, "{theme:?}");
            assert_eq!(choices[0], ("Clock".to_string(), true), "{theme:?}");
            assert!(choices.iter().skip(1).all(|c| !c.1), "{theme:?}");
        }
    }

    #[test]
    fn start_with_windows_follows_the_full_screen_submenu() {
        // Over maximized windows (Step aside) is gone: nothing sits between them.
        let items = menu_items(&Settings::default(), false, &OyenzaLink::Off, false, UpdateLine::None);
        let fs = items.iter().position(|i| matches!(i, MenuItem::Sub { label, .. } if label == "In full-screen apps")).expect("full-screen submenu");
        assert!(matches!(items.get(fs + 1), Some(MenuItem::Item { cmd: MenuCmd::ToggleAutostart, .. })));
    }

    #[test]
    fn full_screen_submenu_checks_the_current_choice() {
        let mut s = Settings::default();
        for hide in [true, false] {
            s.hide_in_fullscreen = hide;
            let choices = fullscreen_choices(&s);
            assert_eq!(
                choices,
                vec![
                    ("Hide".to_string(), MenuCmd::SetHideFullscreen(true), hide),
                    ("Stay on top".to_string(), MenuCmd::SetHideFullscreen(false), !hide),
                    ("Show FPS, GPU and CPU over games".to_string(), MenuCmd::ToggleGameStats, false),
                    ("Only for a few seconds (no added delay)".to_string(), MenuCmd::ToggleGamePeek, false),
                ]
            );
        }
    }
}
