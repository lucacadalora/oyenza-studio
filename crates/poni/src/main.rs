//! Poni — a living notch (Dynamic Island) for Windows 10/11, and for macOS.
//!
//! The Mac app lives in `src/mac/`: its own window, renderer (Core Graphics +
//! Core Text), main loop and menu bar item, around the same state, pages and
//! activities as the Windows app.
//!
//! Architecture (see ARCHITECTURE.md):
//! - One UI thread owns a layered top-most window, the Direct2D renderer and
//!   [`state::AppState`]. Rendering only happens while something moves.
//! - Services (media, lyrics, audio, power, Bluetooth, …) run on
//!   their own threads/callbacks and talk to the UI thread through
//!   [`events::EventSink`] (an mpsc channel + a posted window message).
//! - Pages draw with an immediate-mode [`ui::Ui`] and request side effects by
//!   pushing [`commands::Command`]s, which the app executes after each frame.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// The Mac app is being built in slices: shared code its later slices (music,
// Shelf, clipboard, HUDs) will call sits unused there until then.
#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(any(windows, target_os = "macos"))]
#[macro_use]
mod plog;
#[cfg(any(windows, target_os = "macos"))]
mod activities;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod clock;
#[cfg(any(windows, target_os = "macos"))]
mod commands;
#[cfg(windows)]
mod dragdrop;
#[cfg(windows)]
mod events;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod glass;
#[cfg(any(windows, target_os = "macos"))]
mod icons;
#[cfg(any(windows, target_os = "macos"))]
mod island;
#[cfg(any(windows, target_os = "macos"))]
mod oyen;
#[cfg(any(windows, target_os = "macos"))]
mod pages;
#[cfg(any(windows, target_os = "macos"))]
mod pill;
#[cfg(windows)]
mod paths;
#[cfg(any(windows, target_os = "macos"))]
mod png;
#[cfg(windows)]
mod selftest;
#[cfg(windows)]
mod services;
#[cfg(windows)]
mod shell;
#[cfg(any(windows, target_os = "macos"))]
mod state;
#[cfg(windows)]
mod tray;
#[cfg(any(windows, target_os = "macos"))]
mod ui;
#[cfg(any(windows, target_os = "macos"))]
mod version;
#[cfg(any(windows, target_os = "macos"))]
mod updates;
#[cfg(windows)]
mod window;

// The Mac app's own modules (same names as their Windows twins, so the
// shared modules above find them at the same paths).
#[cfg(target_os = "macos")]
#[path = "mac/app.rs"]
mod app;
#[cfg(target_os = "macos")]
#[path = "mac/clock.rs"]
mod clock;
#[cfg(target_os = "macos")]
#[path = "mac/dragdrop.rs"]
mod dragdrop;
#[cfg(target_os = "macos")]
#[path = "mac/events.rs"]
mod events;
#[cfg(target_os = "macos")]
#[path = "mac/gfx.rs"]
mod gfx;
#[cfg(target_os = "macos")]
#[path = "mac/glass.rs"]
mod glass;
#[cfg(target_os = "macos")]
#[path = "mac/menubar.rs"]
mod menubar;
#[cfg(target_os = "macos")]
#[path = "mac/notch.rs"]
mod notch;
#[cfg(target_os = "macos")]
#[path = "mac/paths.rs"]
mod paths;
#[cfg(target_os = "macos")]
#[path = "mac/render.rs"]
mod render;
#[cfg(target_os = "macos")]
#[path = "mac/services.rs"]
mod services;
#[cfg(target_os = "macos")]
#[path = "mac/window.rs"]
mod window;

#[cfg(any(windows, target_os = "macos"))]
fn main() {
    std::process::exit(app::main_entry());
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("Poni runs on Windows and macOS. Build it with `cargo wbuild` (Windows, cross) or on a Mac.");
}
