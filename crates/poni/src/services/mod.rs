//! Background services. Each one reports to the UI thread through an
//! [`EventSink`]; a failing service is logged and skipped, never fatal.

pub mod applemusic;
pub mod audio;
pub mod backdrop;
pub mod bluetooth;
pub mod clipboard;
pub mod fullscreen;
pub mod http;
pub mod imaging;
pub mod lyrics;
pub mod media;
pub mod oyenza;
pub mod phonedrop;
pub mod power;
pub mod romanize;
#[cfg(test)]
mod romanize_compare;
pub mod secret;
pub mod sound;
pub mod sysmon;
pub mod sysstats;

use crate::commands::{MediaCmd, OyenzaCmd};
use crate::events::EventSink;
use crate::state::MediaSession;
use poni_core::geom::Color;
use poni_core::settings::Settings;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

pub struct Services {
    media: Option<media::MediaService>,
    lyrics: Option<lyrics::LyricsService>,
    _audio: Option<audio::AudioService>,
    _bluetooth: Option<bluetooth::BluetoothService>,
    fullscreen: Option<fullscreen::FullscreenWatcher>,
    clip: Option<clipboard::ClipboardHistory>,
    oyenza: Option<oyenza::OyenzaService>,
}

fn started<T>(name: &str, r: windows::core::Result<T>) -> Option<T> {
    match r {
        Ok(v) => {
            crate::pinfo!("service {name}: started");
            Some(v)
        }
        Err(e) => {
            crate::pwarn!("service {name}: unavailable ({e})");
            None
        }
    }
}

impl Services {
    pub fn start(sink: &EventSink, _island: HWND, helper: HWND, settings: &Settings, activity: oyenza::Activity) -> Self {
        let media = started("media", media::MediaService::start(sink.clone()));
        let lyrics = started("lyrics", lyrics::LyricsService::start(sink.clone()));
        let audio = started("audio", audio::AudioService::start(sink.clone()));
        let bluetooth = started("bluetooth", bluetooth::BluetoothService::start(sink.clone()));
        let fullscreen = started("fullscreen", fullscreen::FullscreenWatcher::start(helper, sink.clone()));
        let clip = started("clipboard history", clipboard::ClipboardHistory::start(sink.clone(), settings.clipboard_history));
        // Idle (no calls) until the person signs in to Oyenza from the tray.
        let oyenza = started("oyenza", oyenza::OyenzaService::start(sink.clone(), activity));
        Self { media, lyrics, _audio: audio, _bluetooth: bluetooth, fullscreen, clip, oyenza }
    }

    pub fn stop(self, _helper: HWND) {
        if let Some(c) = self.clip {
            c.stop();
        }
        if let Some(f) = self.fullscreen {
            f.stop();
        }
    }

    /// Turn clipboard history capture on or off (the setting changed).
    pub fn set_clipboard_history(&self, on: bool) {
        if let Some(c) = &self.clip {
            c.set_enabled(on);
        }
    }

    /// Explorer restarted: AppBar registrations are lost and must be redone.
    pub fn on_taskbar_created(&self) {
        if let Some(f) = &self.fullscreen {
            f.on_taskbar_created();
        }
    }

    pub fn media_cmd(&self, c: MediaCmd) {
        if let Some(m) = &self.media {
            m.send(c);
        }
    }

    pub fn request_lyrics(&self, s: &MediaSession) {
        if let Some(l) = &self.lyrics {
            l.request(s);
        }
    }

    pub fn oyenza(&self, c: OyenzaCmd) {
        if let Some(o) = &self.oyenza {
            o.send(c);
        }
    }

    /// The PC's use or power changed (the Oyenza worker paces itself by it).
    pub fn oyenza_activity(&self, a: oyenza::Activity) {
        if let Some(o) = &self.oyenza {
            o.set_activity(a);
        }
    }

    /// The Apple Music token on disk was saved or removed.
    pub fn apple_music_token_changed(&self) {
        if let Some(l) = &self.lyrics {
            l.apple_token_changed();
        }
    }

    /// Handle the AppBar callback message; `Some(true)` when a full-screen
    /// app covers the island's monitor.
    pub fn on_appbar_message(&self, wp: WPARAM, lp: LPARAM, monitor: isize) -> Option<bool> {
        self.fullscreen.as_ref().and_then(|f| f.on_appbar_message(wp, lp, monitor))
    }
}

/// Windows apps are set to light mode (Settings › Personalization › Colors).
pub fn apps_use_light_theme() -> bool {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    // The system background is white in light mode and black in dark mode.
    UISettings::new().and_then(|s| s.GetColorValue(UIColorType::Background)).map(|c| c.R as u32 + c.G as u32 + c.B as u32 > 382).unwrap_or(true)
}

/// The Windows accent colour as it is: the Glass theme's text and fills sit
/// on light and dark glass alike, so it isn't lightened for black.
pub fn system_accent_base() -> Option<Color> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let s = UISettings::new().ok()?;
    let c = s.GetColorValue(UIColorType::Accent).ok()?;
    Some(Color::rgba(c.R as f32 / 255.0, c.G as f32 / 255.0, c.B as f32 / 255.0, 1.0))
}

/// The Windows accent colour, lightened for use on the black island.
pub fn system_accent() -> Option<Color> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let s = UISettings::new().ok()?;
    let c = s.GetColorValue(UIColorType::AccentLight2).ok()?;
    Some(Color::rgba(c.R as f32 / 255.0, c.G as f32 / 255.0, c.B as f32 / 255.0, 1.0).for_island_accent())
}
