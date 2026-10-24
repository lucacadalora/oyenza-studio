//! Notification-area icon and its menu (also shown on right-click of the island).

use crate::events::WM_APP_TRAY;
use crate::state::OyenzaLink;
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
            let tip: Vec<u16> = "Oyen Poni".encode_utf16().collect();
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
    ToggleClipboard,
    ToggleSystemHistory,
    ToggleLyrics,
    TogglePronunciation,
    /// true: hide in full-screen apps; false: stay on top of them.
    SetHideFullscreen(bool),
    /// Show over games (frame rate, GPU and CPU on the island over a game).
    ToggleGameStats,
    ToggleSounds,
    TogglePinned,
    ToggleAutostart,
    OpenSettings,
    ReloadSettings,
    OpenDataFolder,
    About,
    AppleMusicToken,
    AppleMusicHelp,
    AppleMusicForget,
    OyenzaSignIn,
    OyenzaCancel,
    OyenzaDisconnect,
    OyenzaOpen,
    ToggleOyenzaAmounts,
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

pub fn menu_items(s: &Settings, pinned: bool, oyenza: &OyenzaLink) -> Vec<MenuItem> {
    let style = |label: &str, st: IdleStyle| item(label, MenuCmd::ToggleIdleStyle(st), s.idle_style == st);
    let theme = |label: &str, t: Theme| item(label, MenuCmd::SetTheme(t), s.theme == t);
    let fullscreen = |label: &str, hide: bool| item(label, MenuCmd::SetHideFullscreen(hide), s.hide_in_fullscreen == hide);
    // In the Oyen theme the eyes are Oyen's own.
    let eyes = if s.theme == Theme::Oyen { "Oyen" } else { "Eyes" };
    vec![
        item("Keep island open", MenuCmd::TogglePinned, pinned),
        MenuItem::Sub { label: "Theme".into(), items: vec![theme("Classic", Theme::Classic), theme("Oyen", Theme::Oyen), theme("Glass", Theme::Glass)] },
        MenuItem::Sub {
            label: "Idle look".into(),
            items: vec![
                style("Clock", IdleStyle::Clock),
                style(eyes, IdleStyle::Eyes),
                style("Minimal", IdleStyle::Minimal),
                style("Hidden until hover", IdleStyle::Hidden),
            ],
        },
        MenuItem::Separator,
        item("Synced lyrics", MenuCmd::ToggleLyrics, s.lyrics),
        item("Lyrics pronunciation (romaji, pinyin, Korean)", MenuCmd::TogglePronunciation, s.lyrics_pronunciation),
        apple_music_menu(),
        oyenza_menu(s, oyenza),
        item("Clipboard history", MenuCmd::ToggleClipboard, s.clipboard_history),
        item("Keep System page history", MenuCmd::ToggleSystemHistory, s.system_history),
        item("Sounds", MenuCmd::ToggleSounds, s.sounds),
        MenuItem::Sub {
            label: "In full-screen apps".into(),
            items: vec![
                fullscreen("Hide", true),
                fullscreen("Stay on top", false),
                MenuItem::Separator,
                item("Show FPS, GPU and CPU over games", MenuCmd::ToggleGameStats, s.game_stats),
            ],
        },
        item("Start with Windows", MenuCmd::ToggleAutostart, crate::autostart::is_enabled()),
        MenuItem::Separator,
        item("Open settings file", MenuCmd::OpenSettings, false),
        item("Reload settings", MenuCmd::ReloadSettings, false),
        item("Open data folder", MenuCmd::OpenDataFolder, false),
        item("Lyrics by LRCLIB", MenuCmd::About, false),
        MenuItem::Separator,
        item("Quit Oyen Poni", MenuCmd::Quit, false),
    ]
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
        let launches = matches!(cmd, Some(MenuCmd::OpenSettings | MenuCmd::OpenDataFolder | MenuCmd::About | MenuCmd::AppleMusicHelp | MenuCmd::OyenzaOpen));
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
        let items = menu_items(s, false, &OyenzaLink::Off);
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
                ]
            );
        }
    }
}
