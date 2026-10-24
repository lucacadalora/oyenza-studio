//! The Glass theme on the Mac: macOS's own glass under the island, made the
//! way Control Center and the menus are made. (The Windows app copies the
//! screen and bends it itself; on a Mac that copy would need the screen
//! recording permission and its menu bar indicator.)
//!
//! * From macOS 26 it is Liquid Glass (`NSGlassEffectView`): the system bends
//!   and lights what is behind the island. Before that, the dark HUD blur
//!   (`NSVisualEffectView`) stands in.
//! * The glass is the dark kind, as the island always is next to the
//!   camera, and the content on it draws in the dark Glass look: white text
//!   stays readable over any wallpaper without the app reading the screen.
//! * Where the island meets a MacBook's notch it keeps a black brow the
//!   notch's height, fading into the glass under it: the notch is black
//!   hardware, and glass beside it would show it as a hole. Resting in the
//!   menu bar's line the island is all brow, black as in Classic.
//!
//! The glass view sits in the panel under the island view (window.rs) and
//! follows the island's outline each frame, before the island repaints; it
//! is hidden in the other themes. Offscreen renders paint a smoked stand-in.

use crate::gfx::{notch_segments, Canvas};
use crate::notch::Notch;
use crate::window;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions, NSColor, NSGlassEffectView, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode, NSWorkspace,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use poni_core::geom::{Color, Rect};
use std::cell::RefCell;

/// The black brow fades into the glass over this height (DIPs).
const BROW_FADE: f32 = 16.0;
/// The glass is tinted toward black by this much, so white text reads over
/// a bright wallpaper too.
const TINT: f64 = 0.28;
/// What offscreen renders paint for the glass: its dark ground, mostly opaque.
const STAND_IN: Color = Color::rgba(0.110, 0.110, 0.118, 0.82);
/// The resting pill's glass sits on the menu bar, often near white, where
/// the tint alone leaves its content pale: a smoke deepens small glass, and
/// fades out as the island opens (from SMOKE_FULL_H to SMOKE_NONE_H tall).
const SMALL_SMOKE: f32 = 0.55;
const SMOKE_FULL_H: f32 = 40.0;
const SMOKE_NONE_H: f32 = 72.0;

enum Material {
    /// Liquid Glass, macOS 26 and later.
    Liquid(Retained<NSGlassEffectView>),
    Blur(Retained<NSVisualEffectView>),
}

struct GlassView {
    material: Material,
    /// What was last set: frame (the root view's points, y up) and corner.
    frame: NSRect,
    radius: f64,
    shown: bool,
}

impl GlassView {
    fn view(&self) -> &NSView {
        match &self.material {
            Material::Liquid(v) => v,
            Material::Blur(v) => v,
        }
    }

    fn name(&self) -> &'static str {
        match self.material {
            Material::Liquid(_) => "Liquid Glass (NSGlassEffectView)",
            Material::Blur(_) => "HUD blur (NSVisualEffectView)",
        }
    }
}

thread_local! {
    static GLASS: RefCell<Option<GlassView>> = const { RefCell::new(None) };
}

/// Height of the black brow: the notch's (none without one).
pub fn brow(notch: Option<Notch>) -> f32 {
    notch.map_or(0.0, |n| n.h)
}

/// Where the glass goes for the island's `body` and corner `radius`: nowhere
/// while the island hides or is all brow.
pub fn shape(body: Rect, radius: f32, notch: Option<Notch>, hidden: bool) -> Option<(Rect, f32)> {
    (!hidden && body.h > brow(notch) + 1.0).then_some((body, radius))
}

/// The island's body in the Glass theme, under its content: the black brow
/// where it meets a notch, fading into the glass (the view under the window,
/// or with `stand_in` a smoked fill for offscreen renders). Hidden or all
/// brow, the island is simply black.
pub fn draw_body(c: &mut Canvas, body: Rect, radius: f32, shoulder: f32, notch: Option<Notch>, hidden: bool, stand_in: bool) {
    let brow = brow(notch);
    let black = Color::black(1.0);
    c.unthemed(|c| {
        if shape(body, radius, notch, hidden).is_none() {
            c.notch(body, radius, shoulder, black, None);
            return;
        }
        // The glass view has no shoulders: a plain rounded body.
        if stand_in {
            c.notch(body, radius, 0.0, STAND_IN, None);
        } else {
            let smoke = SMALL_SMOKE * ((SMOKE_NONE_H - body.h) / (SMOKE_NONE_H - SMOKE_FULL_H)).clamp(0.0, 1.0);
            if smoke > 0.0 {
                c.notch(body, radius, 0.0, Color::black(smoke), None);
            }
        }
        if brow > 0.0 {
            let top = body.y + brow;
            c.fill_path_fade(&notch_segments(body, radius, shoulder), black, top, top + BROW_FADE, 1.0, 0.0);
        }
    });
}

/// Show macOS's glass under the island at `shape` (the island's body in
/// DIPs, y down from the window's top, and its corner radius), or hide it.
/// Called before the island repaints, never from inside its drawing.
pub fn show(shape: Option<(Rect, f32)>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    GLASS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some((body, radius)) = shape else {
            if let Some(g) = slot.as_mut().filter(|g| g.shown) {
                g.view().setHidden(true);
                g.shown = false;
            }
            return;
        };
        if slot.is_none() {
            *slot = make(mtm);
        }
        let Some(g) = slot.as_mut() else { return };
        let Some(win_h) = window::with_window(|w| w.frame.size.height) else { return };
        // The top corners reach above the window, where nothing shows: the
        // glass meets the top of the screen square, its bottom rounded.
        let r = radius as f64;
        let frame = NSRect::new(NSPoint::new(body.x as f64, win_h - body.bottom() as f64), NSSize::new(body.w as f64, body.h as f64 + r));
        if frame != g.frame {
            g.view().setFrame(frame);
            g.frame = frame;
        }
        if (r - g.radius).abs() > 0.01 {
            set_radius(g, r);
            g.radius = r;
        }
        if !g.shown {
            g.view().setHidden(false);
            g.shown = true;
        }
    });
}

/// Which glass this Mac draws, once it has shown (for the log and the smoke test).
pub fn kind() -> Option<&'static str> {
    GLASS.with(|cell| cell.borrow().as_ref().map(GlassView::name))
}

/// Is the glass showing under the island right now?
pub fn shown() -> bool {
    GLASS.with(|cell| cell.borrow().as_ref().is_some_and(|g| g.shown))
}

fn make(mtm: MainThreadMarker) -> Option<GlassView> {
    let (root, island) = window::with_window(|w| (w.root.clone(), w.view.clone()))?;
    let zero = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
    let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua });
    // Liquid Glass is a macOS 26 class: look it up before touching it.
    let material = if AnyClass::get(c"NSGlassEffectView").is_some() {
        let v = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), zero);
        // The regular glass is the default style. A selector a release lacks
        // would end the app (an Objective-C exception), so ask first.
        if v.respondsToSelector(sel!(setTintColor:)) {
            v.setTintColor(Some(&NSColor::colorWithWhite_alpha(0.0, TINT)));
        }
        v.setAppearance(dark.as_deref());
        Material::Liquid(v)
    } else {
        let v = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), zero);
        v.setMaterial(NSVisualEffectMaterial::HUDWindow);
        v.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        v.setState(NSVisualEffectState::Active);
        v.setAppearance(dark.as_deref());
        v.setWantsLayer(true);
        Material::Blur(v)
    };
    let g = GlassView { material, frame: zero, radius: -1.0, shown: false };
    g.view().setHidden(true);
    // Pinned to the window's top when the window grows (the island hangs from it).
    g.view().setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    root.addSubview_positioned_relativeTo(g.view(), NSWindowOrderingMode::Below, Some(&**island));
    // Under Reduce Transparency macOS draws glass and blur flat: say so in the log.
    let flat = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency();
    pinfo!("glass: {}{}", g.name(), if flat { ", drawn flat (Reduce Transparency is on)" } else { "" });
    Some(g)
}

fn set_radius(g: &GlassView, r: f64) {
    match &g.material {
        Material::Liquid(v) => {
            if v.respondsToSelector(sel!(setCornerRadius:)) {
                v.setCornerRadius(r);
            }
        }
        Material::Blur(v) => {
            if let Some(layer) = v.layer() {
                layer.setCornerRadius(r);
                layer.setMasksToBounds(true);
            }
        }
    }
}
