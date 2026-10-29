//! The menu bar item (Oyen's mark) and its menu, also shown on a right-click
//! of the island. It carries what works on the Mac so far; lyrics and the
//! Apple Music token join it with the music slice.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, sel, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_core_foundation::CGFloat;
use objc2_core_graphics::{CGBlendMode, CGContext};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSize, NSString};
use crate::state::OyenzaLink;
use crate::updates::UpdateLine;
use poni_core::settings::{IdleStyle, Settings, Theme};
use std::cell::RefCell;

pub use crate::services::login::Status as Login;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    /// "Settings…": the Settings window.
    ShowSettings,
    TogglePinned,
    SetTheme(Theme),
    SetIdleStyle(IdleStyle),
    ToggleSounds,
    /// true: hide in full-screen apps; false: stay on top of them.
    SetHideFullscreen(bool),
    ToggleLogin,
    ToggleLyrics,
    TogglePronunciation,
    AppleMusicToken,
    AppleMusicHelp,
    AppleMusicForget,
    OyenzaSignIn,
    OyenzaCancel,
    ToggleOyenzaAmounts,
    OyenzaOpen,
    OyenzaDisconnect,
    OpenSettings,
    ReloadSettings,
    OpenDataFolder,
    About,
    RecordScreen,
    /// "Update to Oyenza Studio 0.2.5…" (`crate::updates`).
    Update,
    /// "Check for updates…": read the update file now (`crate::updates`).
    CheckForUpdates,
    Quit,
}

pub enum MenuItem {
    Item { label: String, cmd: MenuCmd, checked: bool, key: &'static str },
    Sub { label: String, items: Vec<MenuItem> },
    /// A line to read, not to pick (greyed out).
    Info { label: String },
    Separator,
}

fn item(label: &str, cmd: MenuCmd, checked: bool) -> MenuItem {
    MenuItem::Item { label: label.to_string(), cmd, checked, key: "" }
}

/// `update`: a later version the update file offers, or Check for updates…
/// (`crate::updates::menu_line`).
pub fn menu_items(s: &Settings, pinned: bool, login: Login, recorder: bool, oyenza: &OyenzaLink, update: UpdateLine) -> Vec<MenuItem> {
    let theme = |label: &str, t: Theme| item(label, MenuCmd::SetTheme(t), s.theme == t);
    let fullscreen = |label: &str, hide: bool| item(label, MenuCmd::SetHideFullscreen(hide), s.hide_in_fullscreen == hide);
    // The theme's own idle looks (the status bar is Pill's), the one it
    // draws ticked: outside Pill a picked status bar shows as the clock.
    let shown = crate::pill::status::shown(s.theme, s.idle_style);
    let style = |st: IdleStyle| {
        let label = match st {
            IdleStyle::Clock => "Clock",
            // In the Oyen theme the eyes are Oyen's own.
            IdleStyle::Eyes if s.theme == Theme::Oyen => "Oyen",
            IdleStyle::Eyes => "Eyes",
            IdleStyle::Minimal => "Minimal",
            IdleStyle::Hidden => "Hidden until hover",
            IdleStyle::Status => "Status bar",
        };
        item(label, MenuCmd::SetIdleStyle(st), shown == st)
    };
    let mut items = vec![
        MenuItem::Item { label: "Settings…".into(), cmd: MenuCmd::ShowSettings, checked: false, key: "," },
        MenuItem::Separator,
        item("Keep island open", MenuCmd::TogglePinned, pinned),
        MenuItem::Sub {
            label: "Theme".into(),
            items: vec![theme("Classic", Theme::Classic), theme("Oyen", Theme::Oyen), theme("Glass", Theme::Glass), theme("Pill", Theme::Pill)],
        },
        MenuItem::Sub { label: "Idle look".into(), items: crate::pill::status::looks(s.theme).iter().map(|l| style(*l)).collect() },
        item("Synced lyrics", MenuCmd::ToggleLyrics, s.lyrics),
        item("Lyrics pronunciation (romaji, pinyin, Korean)", MenuCmd::TogglePronunciation, s.lyrics_pronunciation),
        apple_music_menu(),
        oyenza_menu(s, oyenza),
        item("Sounds", MenuCmd::ToggleSounds, s.sounds),
        MenuItem::Sub { label: "In full-screen apps".into(), items: vec![fullscreen("Hide", true), fullscreen("Stay on top", false)] },
        match login {
            Login::NeedsApproval => item("Start at login (allow in System Settings)", MenuCmd::ToggleLogin, false),
            Login::On => item("Start at login", MenuCmd::ToggleLogin, true),
            _ => item("Start at login", MenuCmd::ToggleLogin, false),
        },
        MenuItem::Separator,
        item("Open settings file", MenuCmd::OpenSettings, false),
        item("Reload settings", MenuCmd::ReloadSettings, false),
        item("Open data folder", MenuCmd::OpenDataFolder, false),
        item(&crate::version::about_label(), MenuCmd::About, false),
        MenuItem::Separator,
        MenuItem::Item { label: "Quit Oyenza Studio".into(), cmd: MenuCmd::Quit, checked: false, key: "q" },
    ];
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
        // Otherwise a look on request, under About (as in most Mac apps).
        UpdateLine::Check => {
            let at = items.iter().position(|i| matches!(i, MenuItem::Item { cmd: MenuCmd::About, .. })).map_or(items.len(), |i| i + 1);
            items.insert(at, item("Check for updates…", MenuCmd::CheckForUpdates, false));
        }
        UpdateLine::None => {}
    }
    items
}

/// Oyenza, optional: sign in with a code from oyenza.app, then what the
/// island may show, and signing out (as in the Windows tray).
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

/// Opt-in lyrics from Apple Music with the person's own token.
fn apple_music_menu() -> MenuItem {
    let on = crate::paths::apple_music_token_path().exists();
    let mut items = vec![item("Use token from clipboard", MenuCmd::AppleMusicToken, false), item("How to get the token", MenuCmd::AppleMusicHelp, false)];
    if on {
        items.push(MenuItem::Separator);
        items.push(item("Remove token", MenuCmd::AppleMusicForget, false));
    }
    MenuItem::Sub { label: if on { "Apple Music lyrics: on".into() } else { "Apple Music lyrics".into() }, items }
}

define_class!(
    /// Receives the menu's clicks; each item's tag indexes the built commands.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniMenuTarget"]
    pub struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(pick:))]
        fn pick(&self, sender: &NSMenuItem) {
            let cmd = CMDS.with(|c| c.borrow().get(sender.tag() as usize).copied());
            if let Some(cmd) = cmd {
                crate::app::on_menu(cmd);
            }
        }
    }
);

thread_local! {
    /// The commands behind the current menu's items (index = item tag).
    static CMDS: RefCell<Vec<MenuCmd>> = const { RefCell::new(Vec::new()) };
    /// The status item, for opening its menu from code (the smoke test).
    static ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
}

/// Open the menu bar item's menu, as a click on it would. Runs the menu's own
/// tracking loop, so call it with the app not borrowed.
pub fn open_menu() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let item = ITEM.with(|i| i.borrow().clone());
    if let Some(button) = item.and_then(|i| i.button(mtm)) {
        unsafe { button.performClick(None) };
    }
}

pub struct MenuBar {
    item: Retained<NSStatusItem>,
    target: Retained<MenuTarget>,
    pub menu: Retained<NSMenu>,
}

impl MenuBar {
    pub fn add(mtm: MainThreadMarker, s: &Settings, pinned: bool, login: Login, oyenza: &OyenzaLink, update: UpdateLine) -> MenuBar {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            match mark_image(mtm) {
                Some(img) => button.setImage(Some(&img)),
                None => button.setTitle(&NSString::from_str("Poni")),
            }
            button.setToolTip(Some(&NSString::from_str(&format!("Oyenza Studio {}", crate::version::VERSION))));
        }
        let target: Retained<MenuTarget> = unsafe { objc2::msg_send![MenuTarget::alloc(mtm), init] };
        let menu = build(mtm, &target, &menu_items(s, pinned, login, crate::services::recorder::installed(), oyenza, update));
        item.setMenu(Some(&menu));
        ITEM.with(|i| *i.borrow_mut() = Some(item.clone()));
        MenuBar { item, target, menu }
    }

    /// Rebuild the menu (ticks follow the settings).
    pub fn refresh(&mut self, mtm: MainThreadMarker, s: &Settings, pinned: bool, login: Login, oyenza: &OyenzaLink, update: UpdateLine) {
        self.menu = build(mtm, &self.target, &menu_items(s, pinned, login, crate::services::recorder::installed(), oyenza, update));
        self.item.setMenu(Some(&self.menu));
    }
}

fn build(mtm: MainThreadMarker, target: &MenuTarget, items: &[MenuItem]) -> Retained<NSMenu> {
    let mut cmds = Vec::new();
    let menu = build_into(mtm, target, items, &mut cmds);
    CMDS.with(|c| *c.borrow_mut() = cmds);
    menu
}

fn build_into(mtm: MainThreadMarker, target: &MenuTarget, items: &[MenuItem], cmds: &mut Vec<MenuCmd>) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    menu.setAutoenablesItems(false);
    for it in items {
        match it {
            MenuItem::Item { label, cmd, checked, key } => {
                let mi = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(label), Some(sel!(pick:)), &NSString::from_str(key))
                };
                let t: &AnyObject = target.as_ref();
                unsafe { mi.setTarget(Some(t)) };
                mi.setTag(cmds.len() as isize);
                mi.setState(if *checked { NSControlStateValueOn } else { NSControlStateValueOff });
                cmds.push(*cmd);
                menu.addItem(&mi);
            }
            MenuItem::Sub { label, items } => {
                let sub = build_into(mtm, target, items, cmds);
                let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(label), None, &NSString::from_str("")) };
                mi.setSubmenu(Some(&sub));
                menu.addItem(&mi);
            }
            MenuItem::Info { label } => {
                let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(label), None, &NSString::from_str("")) };
                mi.setEnabled(false);
                menu.addItem(&mi);
            }
            MenuItem::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
        }
    }
    menu
}

/// Oyen's mark as a one-colour template image (macOS tints it for light and
/// dark menu bars). Drawn from the mark's own outlines with the small cut's
/// eyes, the size the logo uses at 32 px and below.
fn mark_image(mtm: MainThreadMarker) -> Option<Retained<NSImage>> {
    const PT: f64 = 18.0;
    const SCALE: f32 = 2.0;
    let px = (PT as f32 * SCALE) as usize;
    let bmp = crate::gfx::Bitmap::new(px, px, SCALE)?;
    let ctx: &CGContext = bmp.ctx();
    let parse = |d: &str| poni_core::svgpath::parse(d).ok().map(|segs| crate::gfx::cg_path(&segs));
    let (head, fold, eyes) = (parse(poni_core::oyen::mark::HEAD)?, parse(poni_core::oyen::mark::FOLD)?, parse(poni_core::oyen::mark::EYES_SMALL)?);
    // The mark is 104 units wide, centred on y -11.04 in its own units.
    let s = 16.5 / 104.0;
    CGContext::translate_ctm(Some(ctx), PT / 2.0, PT / 2.0 + 11.04 * s);
    CGContext::scale_ctm(Some(ctx), s, s);
    CGContext::set_rgb_fill_color(Some(ctx), 0.0, 0.0, 0.0, 1.0);
    for p in [&head, &fold] {
        CGContext::begin_path(Some(ctx));
        CGContext::add_path(Some(ctx), Some(p));
        CGContext::fill_path(Some(ctx));
    }
    CGContext::set_blend_mode(Some(ctx), CGBlendMode::Clear);
    CGContext::begin_path(Some(ctx));
    CGContext::add_path(Some(ctx), Some(&eyes));
    CGContext::eo_fill_path(Some(ctx));
    let cg = bmp.image()?;
    let img = NSImage::initWithCGImage_size(NSImage::alloc(), &cg, NSSize::new(PT as CGFloat, PT as CGFloat));
    img.setTemplate(true);
    let _ = mtm;
    Some(img)
}
