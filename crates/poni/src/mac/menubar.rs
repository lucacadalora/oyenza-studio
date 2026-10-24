//! The menu bar item (Oyen's mark) and its menu, also shown on a right-click
//! of the island. Slice 1 carries what works on the Mac so far; lyrics,
//! clipboard history and start-at-login join it in later slices.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, sel, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_core_foundation::CGFloat;
use objc2_core_graphics::{CGBlendMode, CGContext};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSize, NSString};
use poni_core::settings::{IdleStyle, Settings, Theme};
use std::cell::RefCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    TogglePinned,
    SetTheme(Theme),
    SetIdleStyle(IdleStyle),
    ToggleSounds,
    OpenSettings,
    ReloadSettings,
    OpenDataFolder,
    About,
    Quit,
}

pub enum MenuItem {
    Item { label: String, cmd: MenuCmd, checked: bool, key: &'static str },
    Sub { label: String, items: Vec<MenuItem> },
    Separator,
}

fn item(label: &str, cmd: MenuCmd, checked: bool) -> MenuItem {
    MenuItem::Item { label: label.to_string(), cmd, checked, key: "" }
}

pub fn menu_items(s: &Settings, pinned: bool) -> Vec<MenuItem> {
    let style = |label: &str, st: IdleStyle| item(label, MenuCmd::SetIdleStyle(st), s.idle_style == st);
    let theme = |label: &str, t: Theme| item(label, MenuCmd::SetTheme(t), s.theme == t);
    // In the Oyen theme the eyes are Oyen's own.
    let eyes = if s.theme == Theme::Oyen { "Oyen" } else { "Eyes" };
    vec![
        item("Keep island open", MenuCmd::TogglePinned, pinned),
        MenuItem::Sub { label: "Theme".into(), items: vec![theme("Classic", Theme::Classic), theme("Oyen", Theme::Oyen)] },
        MenuItem::Sub {
            label: "Idle look".into(),
            items: vec![
                style("Clock", IdleStyle::Clock),
                style(eyes, IdleStyle::Eyes),
                style("Minimal", IdleStyle::Minimal),
                style("Hidden until hover", IdleStyle::Hidden),
            ],
        },
        item("Sounds", MenuCmd::ToggleSounds, s.sounds),
        MenuItem::Separator,
        item("Open settings file", MenuCmd::OpenSettings, false),
        item("Reload settings", MenuCmd::ReloadSettings, false),
        item("Open data folder", MenuCmd::OpenDataFolder, false),
        item(&format!("About Oyen Poni {}", env!("CARGO_PKG_VERSION")), MenuCmd::About, false),
        MenuItem::Separator,
        MenuItem::Item { label: "Quit Oyen Poni".into(), cmd: MenuCmd::Quit, checked: false, key: "q" },
    ]
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
}

pub struct MenuBar {
    item: Retained<NSStatusItem>,
    target: Retained<MenuTarget>,
    pub menu: Retained<NSMenu>,
}

impl MenuBar {
    pub fn add(mtm: MainThreadMarker, s: &Settings, pinned: bool) -> MenuBar {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            match mark_image(mtm) {
                Some(img) => button.setImage(Some(&img)),
                None => button.setTitle(&NSString::from_str("Poni")),
            }
            button.setToolTip(Some(&NSString::from_str("Oyen Poni")));
        }
        let target: Retained<MenuTarget> = unsafe { objc2::msg_send![MenuTarget::alloc(mtm), init] };
        let menu = build(mtm, &target, &menu_items(s, pinned));
        item.setMenu(Some(&menu));
        MenuBar { item, target, menu }
    }

    /// Rebuild the menu (ticks follow the settings).
    pub fn refresh(&mut self, mtm: MainThreadMarker, s: &Settings, pinned: bool) {
        self.menu = build(mtm, &self.target, &menu_items(s, pinned));
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
