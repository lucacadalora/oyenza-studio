//! Background services. Each one reports to the UI thread through an
//! [`EventSink`]; a failing service is logged and skipped, never fatal.

pub mod addons;
pub mod adzan;
pub mod applemusic;
pub mod applemusic_ui;
pub mod audio;
pub mod backdrop;
pub mod bluetooth;
pub mod clipboard;
pub mod frames;
pub mod fullscreen;
pub mod cities;
pub mod http;
pub mod imaging;
pub mod location;
pub mod lyrics;
pub mod media;
pub mod oyenza;
pub mod pairdrop;
pub mod phonedrop;
pub mod power;
pub mod psu;
pub mod recorder;
pub mod romanize;
#[cfg(test)]
mod romanize_compare;
pub mod secret;
pub mod sound;
pub mod stocks;
pub mod sysmon;
pub mod sysstats;
pub mod update;

use crate::commands::{MediaCmd, OyenzaCmd};
use crate::events::EventSink;
use crate::state::MediaSession;
use poni_core::geom::Color;
use poni_core::settings::Settings;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};

pub struct Services {
    media: Option<media::MediaService>,
    lyrics: Option<lyrics::LyricsService>,
    _audio: Option<audio::AudioService>,
    _bluetooth: Option<bluetooth::BluetoothService>,
    fullscreen: Option<fullscreen::FullscreenWatcher>,
    oyenza: Option<oyenza::OyenzaService>,
    pairdrop: Option<pairdrop::PairDropService>,
    addons: Option<addons::AddonService>,
    /// Started the first time the Stocks page opens ([`Services::stocks_watch`]).
    stocks: std::cell::OnceCell<Option<stocks::StocksService>>,
    sink: EventSink,
}

fn started<T, E: std::fmt::Display>(name: &str, r: Result<T, E>) -> Option<T> {
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
        // Idle (no calls) until the person signs in to Oyenza from the tray.
        let oyenza = started("oyenza", oyenza::OyenzaService::start(sink.clone(), activity));
        // Asks nothing until a phone is paired.
        let pairdrop = started("pair drop", pairdrop::PairDropService::start(sink.clone()));
        // Asleep unless an add-on is on; then it only checks the files it has.
        let on = poni_core::addons::ALL.iter().filter(|a| settings.addon_on(a.id)).map(|a| a.id).collect();
        let addons = started("add-ons", addons::AddonService::start(sink.clone(), on));
        Self { media, lyrics, _audio: audio, _bluetooth: bluetooth, fullscreen, oyenza, pairdrop, addons, stocks: std::cell::OnceCell::new(), sink: sink.clone() }
    }

    pub fn stop(self, _helper: HWND) {
        if let Some(f) = self.fullscreen {
            f.stop();
        }
    }

    /// Run a one-off download on the add-on thread (the update file).
    /// False when that thread couldn't start.
    pub fn run_job(&self, job: addons::Job) -> bool {
        self.addons.as_ref().map(|a| a.run(job)).is_some()
    }

    /// What the Stocks page wants (`pages::stocks::watch`). Its thread starts
    /// the first time the page opens and sleeps whenever it is closed.
    pub fn stocks_watch(&self, w: stocks::Watch) {
        if !w.open && self.stocks.get().is_none() {
            return;
        }
        if let Some(s) = self.stocks.get_or_init(|| started("stocks", stocks::StocksService::start(self.sink.clone()))) {
            s.set_watch(w);
        }
    }

    /// An add-on was switched on (download its files) or off (delete them).
    pub fn set_addon(&self, id: &'static str, on: bool) {
        if let Some(a) = &self.addons {
            a.set(id, on);
        }
    }

    /// Step aside on (the band along the top edge of the island's monitor,
    /// screen pixels) or off: the full-screen watcher reports what reaches it.
    pub fn watch_top(&self, band: Option<RECT>) {
        if let Some(f) = &self.fullscreen {
            f.watch_top(band);
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

    /// Show a pairing QR code: the link, or a sentence for the panel.
    pub fn pair_begin(&self) -> Result<String, String> {
        match &self.pairdrop {
            Some(p) => p.begin_pairing(),
            None => Err("Pairing couldn't start on this PC.".into()),
        }
    }

    pub fn pair_cancel(&self) {
        if let Some(p) = &self.pairdrop {
            p.cancel_pairing();
        }
    }

    pub fn pair_remove(&self, room: &str) {
        if let Some(p) = &self.pairdrop {
            p.remove(room);
        }
    }

    /// The Shelf's files, which connected paired phones may save.
    pub fn pair_shelf(&self, paths: Vec<std::path::PathBuf>) {
        if let Some(p) = &self.pairdrop {
            p.set_shelf(paths);
        }
    }

    pub fn pairs(&self) -> Vec<poni_core::pairdrop::Pair> {
        self.pairdrop.as_ref().map(|p| p.pairs()).unwrap_or_default()
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
