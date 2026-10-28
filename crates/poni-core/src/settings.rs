//! User settings, stored as JSON next to the data store.

use crate::ielts::Spice;
use crate::pomodoro::PomodoroConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleStyle {
    /// Small pill with a clock and battery ring.
    Clock,
    /// Small blank pill (like a MacBook notch).
    Minimal,
    /// A friendly pair of blinking eyes.
    Eyes,
    /// Nothing visible until the cursor reaches the top edge.
    Hidden,
    /// A wider pill like a status bar: the date and time, the volume and
    /// the battery (the Pill theme; the other themes show the clock).
    Status,
}

/// How the island looks. Pages and live activities work the same in every theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// The plain black island.
    #[default]
    Classic,
    /// Oyen, the Oyenza cat, lives in the island: he peeks over its edge
    /// while it rests, bobs to the music and takes Oyen's orange as accent.
    Oyen,
    /// Liquid glass: the island bends, frosts and tints what's under it.
    Glass,
    /// Pill: the island floats a little below the top edge as a black
    /// capsule, with its own layouts: the time and battery at rest, the song
    /// while music plays, a rounded card when it opens.
    Pill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorChoice {
    Primary,
    /// The monitor under the cursor when Poni starts.
    Cursor,
}

/// Bumped when defaults change in a way existing files should pick up.
pub const SETTINGS_VERSION: u32 = 4;

fn version_zero() -> u32 {
    0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Missing in files written before versioning (treated as 0).
    #[serde(default = "version_zero")]
    pub settings_version: u32,
    pub theme: Theme,
    pub idle_style: IdleStyle,
    pub use_24h: bool,
    /// Delay before hover expands the island.
    pub hover_delay_ms: u32,
    /// Grace period after the cursor leaves before collapsing (0 = the
    /// island starts closing on the next frame).
    pub collapse_delay_ms: u32,
    /// Width of the expanded island in DIPs (bigger = roomier pages).
    pub expanded_width: f32,
    pub hide_in_fullscreen: bool,
    /// Step aside over maximized windows: the resting island turned into the
    /// Hidden look's invisible strip while an app window reached the top of
    /// the screen under it. 0.2.4 and 0.2.5 did it by default and it read as
    /// the island vanishing, so it's gone from the menu and Settings and
    /// always off (`sanitized`); the Hidden idle look is the way to keep the
    /// top edge clear. The key stays so files from those versions still load.
    pub step_aside: bool,
    pub monitor: MonitorChoice,
    pub show_media_activity: bool,
    pub lyrics: bool,
    /// Romanized "pronunciation" line under Japanese, Korean and Chinese lyrics.
    pub lyrics_pronunciation: bool,
    /// Shift synced lyrics: positive shows each line earlier (ms).
    pub lyrics_offset_ms: i32,
    pub hud_volume: bool,
    pub hud_power: bool,
    pub hud_bluetooth: bool,
    pub sounds: bool,
    /// Page shown first when expanding ("media", "today", "health", "shelf", "tasks", "focus", "ai", "oyenza").
    pub start_page: String,
    /// Remember the last page instead of `start_page`.
    pub remember_page: bool,
    /// Tabs taken off the header's row in Settings, by page key: Music,
    /// Today, Shelf, Tasks and Focus can go. Keys this version doesn't know
    /// are kept, so a newer Studio's choices survive a save.
    pub hidden_tabs: Vec<String>,
    pub pomodoro: PomodoroConfig,
    /// Oyenza (once signed in): show sums and senders on the island straight
    /// away. Off: they wait for a click on Show, so a shared screen shows none.
    pub oyenza_show_amounts: bool,
    /// Keep the System page's charts going while it's closed (a light reading
    /// of the kernel counters once a second), so they're full when it opens.
    /// Only while the System add-on is on ([`Settings::system_history_on`]).
    pub system_history: bool,
    /// Add-ons switched on, by id (`poni_core::addons`). Ids this version
    /// doesn't know are kept, so a newer Poni's choices survive a save.
    pub addons: BTreeMap<String, bool>,
    /// The Prayer times add-on's choices.
    pub prayer: PrayerSettings,
    /// Show over games: while a full-screen app is in front, the island
    /// shows its frame rate, the GPU's load and temperature and the CPU's
    /// load. Turning it on also keeps the island on top of full-screen apps.
    pub game_stats: bool,
    /// Show over games, readout only for a few seconds (when the game comes
    /// to the front, when the frame rate drops sharply, on Alt+Shift+F),
    /// then the island hides and Windows sends the game's frames straight to
    /// the screen. Off (the default), the readout stays while the game is in
    /// front. Its own name: the corner test build wrote `game_readout`
    /// ("peek" as its default) into the files of the people who tried it.
    pub game_peek: bool,
    /// The AI tab's choices (its readers are the `studio-usage` crate).
    pub usage: UsageSettings,
    /// The Record page's choices (Oyenza Studio Recorder).
    pub record: RecordSettings,
    /// The Notifications add-on's choices: which apps the island shows, and
    /// whether who it's from and the message show at once.
    pub notifications: crate::notifications::NotificationSettings,
    /// How Oyen talks on the IELTS page (Sopan, Gaul or Ngegas, the default).
    pub ielts_spice: Spice,
}

/// What the AI tab shows and whether it gives heads-up notices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageSettings {
    /// Services the person hid, by key (`studio_usage::ProviderId::key`). Keys this
    /// version doesn't know are kept, so a newer Studio's choices survive a save.
    pub hidden: Vec<String>,
    /// Services shown even when nothing of them is on this computer (the
    /// ones that take a typed key), by key.
    pub added: Vec<String>,
    /// Heads-up notices: a limit getting close, running out or coming back.
    /// Off (the default), the tab reads only while it is open.
    pub notices: bool,
    /// The notice for a limit getting close comes once this much of it is used (percent).
    pub near_pct: u8,
}

impl Default for UsageSettings {
    fn default() -> Self {
        Self { hidden: Vec::new(), added: Vec::new(), notices: false, near_pct: 80 }
    }
}

/// What the Record page records next: its last picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordSettings {
    pub target: crate::recorder::Target,
    pub camera: bool,
    pub microphone: bool,
    pub system_audio: bool,
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self { target: crate::recorder::Target::Screen, camera: false, microphone: true, system_audio: false }
    }
}

impl RecordSettings {
    pub fn inputs(&self) -> crate::recorder::Inputs {
        crate::recorder::Inputs { camera: self.camera, microphone: self.microphone, system_audio: self.system_audio }
    }
}

/// What plays when a prayer's time comes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrayerSound {
    /// The adzan recording (Subuh plays the tone: the recording isn't Subuh's adzan).
    Adzan,
    /// A short system tone.
    #[default]
    Tone,
    Silent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrayerSettings {
    /// The place the times are for: its id in the add-on's place list, or
    /// "w:lat,lng" for a city abroad. None until one is found or picked.
    pub place: Option<String>,
    /// That city abroad as its own few lines (`prayer::World::excerpt`): its
    /// name, country, point and clock, so its times need nothing online.
    pub world_place: Option<String>,
    /// Let Windows' location pick the nearest place, and pick again when the
    /// PC moves. Picking a place by hand turns this off.
    pub follow_location: bool,
    pub sound: PrayerSound,
    /// Minutes before each prayer the island counts down (0 = no countdown).
    pub remind_before_min: u32,
    /// Pause music that is playing when the adzan starts (it stays paused).
    pub pause_music: bool,
}

impl Default for PrayerSettings {
    fn default() -> Self {
        Self { place: None, world_place: None, follow_location: true, sound: PrayerSound::Tone, remind_before_min: 10, pause_music: true }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            theme: Theme::Classic,
            idle_style: IdleStyle::Clock,
            use_24h: true,
            hover_delay_ms: 220,
            collapse_delay_ms: 0,
            expanded_width: 620.0,
            hide_in_fullscreen: true,
            step_aside: false,
            monitor: MonitorChoice::Primary,
            show_media_activity: true,
            lyrics: true,
            lyrics_pronunciation: true,
            lyrics_offset_ms: 0,
            hud_volume: true,
            hud_power: true,
            hud_bluetooth: true,
            sounds: true,
            start_page: "media".into(),
            remember_page: true,
            hidden_tabs: Vec::new(),
            pomodoro: PomodoroConfig::default(),
            oyenza_show_amounts: false,
            system_history: true,
            addons: BTreeMap::new(),
            prayer: PrayerSettings::default(),
            game_stats: false,
            game_peek: false,
            usage: UsageSettings::default(),
            record: RecordSettings::default(),
            notifications: crate::notifications::NotificationSettings::default(),
            ielts_spice: Spice::default(),
        }
    }
}

impl Settings {
    /// Show over games needs the island on top of full-screen apps, so the
    /// two follow each other: turning the readout on picks Stay on top, and
    /// picking Hide afterwards turns the readout off. `before` is the state
    /// the change started from.
    pub fn pair_game_stats(&mut self, before: &Settings) {
        if !(self.game_stats && self.hide_in_fullscreen) {
            return;
        }
        if before.game_stats && !before.hide_in_fullscreen {
            self.game_stats = false;
        } else {
            self.hide_in_fullscreen = false;
        }
    }

    /// Is the add-on `id` switched on?
    pub fn addon_on(&self, id: &str) -> bool {
        self.addons.get(id).copied().unwrap_or(false)
    }

    pub fn set_addon(&mut self, id: &str, on: bool) {
        self.addons.insert(id.to_string(), on);
    }

    /// The System add-on is on: the System tab shows.
    pub fn system_on(&self) -> bool {
        self.addon_on(crate::addons::SYSTEM.id)
    }

    /// The System page's readings go on while it's closed: its history
    /// switch, and only while System itself is on.
    pub fn system_history_on(&self) -> bool {
        self.system_on() && self.system_history
    }

    /// Has the person never switched the add-on `id` on or off? An add-on
    /// that switches itself on when first needed (Japanese readings) does so
    /// only then: one switched off by hand stays off.
    pub fn addon_unset(&self, id: &str) -> bool {
        !self.addons.contains_key(id)
    }

    /// Screen recorder: on once Oyenza Studio Recorder is `installed`, unless
    /// the person switched it off (installing the recorder is the switch).
    pub fn recorder_on(&self, installed: bool) -> bool {
        installed && self.addons.get(crate::addons::SCREEN_RECORDER.id).copied().unwrap_or(true)
    }

    /// Clamp values a user may have hand-edited into nonsense.
    pub fn sanitized(mut self) -> Self {
        // Only untouched old defaults move.
        // v2: hover opens after a 220 ms dwell (140 ms caught pointers on
        // their way to browser tabs).
        if self.settings_version < 2 && self.hover_delay_ms == 140 {
            self.hover_delay_ms = 220;
        }
        // v3: the island closes as soon as the pointer leaves it (the old
        // 380 ms grace read as lag; coming back while it closes reopens it).
        if self.settings_version < 3 && self.collapse_delay_ms == 380 {
            self.collapse_delay_ms = 0;
        }
        // v4: Clipboard history became an add-on, off by default (it left the
        // app after 0.2.8, and its old `clipboard_history` key is dropped on
        // the next save). A Clipboard start page opens Music.
        if self.settings_version < 4 && self.start_page == "clipboard" {
            self.start_page = "media".into();
        }
        self.settings_version = self.settings_version.max(SETTINGS_VERSION);
        // Step aside is no longer offered: the island stays over maximized windows.
        self.step_aside = false;
        // Show over games needs the island on top of full-screen apps.
        if self.game_stats {
            self.hide_in_fullscreen = false;
        }
        self.hover_delay_ms = self.hover_delay_ms.min(2000);
        self.lyrics_offset_ms = self.lyrics_offset_ms.clamp(-5000, 5000);
        self.collapse_delay_ms = self.collapse_delay_ms.min(5000);
        if !self.expanded_width.is_finite() {
            self.expanded_width = 620.0;
        }
        self.expanded_width = self.expanded_width.clamp(480.0, 900.0);
        let p = &mut self.pomodoro;
        p.focus_min = p.focus_min.clamp(1, 180);
        p.short_break_min = p.short_break_min.clamp(1, 60);
        p.long_break_min = p.long_break_min.clamp(1, 120);
        p.long_every = p.long_every.min(12);
        self.prayer.remind_before_min = self.prayer.remind_before_min.min(60);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        // An old file's "water" block (the page Health replaced) is ignored.
        let s: Settings = serde_json::from_str(r#"{"idle_style":"eyes","water":{"goal_ml":1500}}"#).unwrap();
        assert_eq!(s.idle_style, IdleStyle::Eyes);
        assert_eq!(s.pomodoro.focus_min, 25);
    }

    #[test]
    fn addons_are_off_until_switched_on_and_unknown_ones_survive() {
        let mut s = Settings::default();
        assert!(!s.addon_on("sholat"));
        s.set_addon("sholat", true);
        assert!(s.addon_on("sholat"));
        // A newer Poni's add-on stays in the file when this one saves it.
        let s: Settings = serde_json::from_str(r#"{"addons":{"sholat":true,"future":true}}"#).unwrap();
        assert!(s.addon_on("sholat"));
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.addon_on("future"));
        // Files from before add-ons have none on.
        let old: Settings = serde_json::from_str(r#"{"settings_version":3}"#).unwrap();
        assert!(old.addons.is_empty());
    }

    #[test]
    fn an_addon_switched_off_by_hand_is_no_longer_unset() {
        let mut s = Settings::default();
        assert!(s.addon_unset("ja"));
        s.set_addon("ja", true);
        assert!(!s.addon_unset("ja"));
        s.set_addon("ja", false);
        assert!(!s.addon_unset("ja"), "off by hand: the person's word, kept");
        assert!(!s.addon_on("ja"));
        // It survives a save and a load.
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!back.addon_unset("ja"));
    }

    #[test]
    fn prayer_defaults_follow_location_with_a_tone_ten_minutes_before() {
        let p = Settings::default().prayer;
        assert_eq!((p.place, p.follow_location, p.sound, p.remind_before_min, p.pause_music), (None, true, PrayerSound::Tone, 10, true));
        let s: Settings = serde_json::from_str(r#"{"prayer":{"place":"jakarta","sound":"adzan","remind_before_min":600}}"#).unwrap();
        let s = s.sanitized();
        assert_eq!((s.prayer.place.as_deref(), s.prayer.sound, s.prayer.remind_before_min), (Some("jakarta"), PrayerSound::Adzan, 60));
        assert!(s.prayer.follow_location);
    }

    #[test]
    fn theme_defaults_to_classic() {
        // Files written before themes existed keep the black island.
        let s: Settings = serde_json::from_str(r#"{"idle_style":"eyes"}"#).unwrap();
        assert_eq!(s.theme, Theme::Classic);
        let s: Settings = serde_json::from_str(r#"{"theme":"oyen"}"#).unwrap();
        assert_eq!(s.theme, Theme::Oyen);
        let s: Settings = serde_json::from_str(r#"{"theme":"glass"}"#).unwrap();
        assert_eq!(s.theme, Theme::Glass);
        let s: Settings = serde_json::from_str(r#"{"theme":"pill"}"#).unwrap();
        assert_eq!(s.theme, Theme::Pill);
    }

    #[test]
    fn sanitize_clamps() {
        let s = Settings { expanded_width: 5000.0, hover_delay_ms: 99999, ..Default::default() }.sanitized();
        assert_eq!(s.expanded_width, 900.0);
        assert_eq!(s.hover_delay_ms, 2000);
    }

    #[test]
    fn migrates_old_hover_default_once() {
        let s: Settings = serde_json::from_str(r#"{"hover_delay_ms":140}"#).unwrap();
        assert_eq!(s.settings_version, 0);
        let s = s.sanitized();
        assert_eq!((s.hover_delay_ms, s.settings_version), (220, SETTINGS_VERSION));
        // A user's own choice is kept, and a current file isn't touched.
        let s: Settings = serde_json::from_str(r#"{"hover_delay_ms":90}"#).unwrap();
        assert_eq!(s.sanitized().hover_delay_ms, 90);
        let s: Settings = serde_json::from_str(r#"{"settings_version":2,"hover_delay_ms":140}"#).unwrap();
        assert_eq!(s.sanitized().hover_delay_ms, 140);
    }

    #[test]
    fn files_with_clipboard_history_still_load_and_drop_it() {
        // Files from 0.2.8 and before hold the key: it's ignored, and gone after a save.
        let s = serde_json::from_str::<Settings>(r#"{"settings_version":4,"clipboard_history":true,"theme":"oyen"}"#).unwrap().sanitized();
        assert_eq!(s.theme, Theme::Oyen);
        assert!(!serde_json::to_string(&s).unwrap().contains("clipboard"));
        // A Clipboard start page from before 0.2.7 opens Music.
        let s = serde_json::from_str::<Settings>(r#"{"settings_version":3,"start_page":"clipboard"}"#).unwrap().sanitized();
        assert_eq!((s.start_page.as_str(), s.settings_version), ("media", SETTINGS_VERSION));
    }

    #[test]
    fn system_history_counts_only_while_system_is_on() {
        // Off by default, so nothing reads the counters at rest; its history
        // switch keeps its default for when it is switched on.
        let mut s = Settings::default();
        assert!(!s.system_on() && s.system_history && !s.system_history_on());
        s.set_addon(crate::addons::SYSTEM.id, true);
        assert!(s.system_on() && s.system_history_on());
        s.system_history = false;
        assert!(!s.system_history_on());
        // Files from before have no System switch: off.
        let old: Settings = serde_json::from_str(r#"{"settings_version":4,"system_history":true}"#).unwrap();
        assert!(!old.sanitized().system_on());
    }

    #[test]
    fn migrates_old_collapse_grace_once() {
        // Files from 0.1.7 and earlier hold the old 380 ms grace.
        let s = serde_json::from_str::<Settings>(r#"{"settings_version":2,"collapse_delay_ms":380}"#).unwrap().sanitized();
        assert_eq!((s.collapse_delay_ms, s.settings_version), (0, SETTINGS_VERSION));
        let s = serde_json::from_str::<Settings>(r#"{"collapse_delay_ms":380,"hover_delay_ms":140}"#).unwrap().sanitized();
        assert_eq!((s.collapse_delay_ms, s.hover_delay_ms), (0, 220));
        // A grace the user chose is kept, a current file isn't touched, and 0 stays 0.
        let s: Settings = serde_json::from_str(r#"{"settings_version":2,"collapse_delay_ms":600}"#).unwrap();
        assert_eq!(s.sanitized().collapse_delay_ms, 600);
        let s: Settings = serde_json::from_str(r#"{"settings_version":3,"collapse_delay_ms":380}"#).unwrap();
        assert_eq!(s.sanitized().collapse_delay_ms, 380);
        assert_eq!(Settings::default().sanitized().collapse_delay_ms, 0);
        // A file from a newer Poni keeps its version.
        let s: Settings = serde_json::from_str(r#"{"settings_version":9}"#).unwrap();
        assert_eq!(s.sanitized().settings_version, 9);
    }

    #[test]
    fn game_stats_and_full_screen_choice_follow_each_other() {
        // The default hides the island over full-screen apps; turning the
        // readout on keeps it on top instead.
        let before = Settings::default();
        let mut s = Settings { game_stats: true, ..before.clone() };
        s.pair_game_stats(&before);
        assert!(s.game_stats && !s.hide_in_fullscreen);
        // Picking Hide afterwards turns the readout off.
        let before = s.clone();
        let mut s = Settings { hide_in_fullscreen: true, ..before.clone() };
        s.pair_game_stats(&before);
        assert!(!s.game_stats && s.hide_in_fullscreen);
        // Stay on top alone, and the readout off, change nothing.
        let before = Settings { hide_in_fullscreen: false, ..Settings::default() };
        let mut s = before.clone();
        s.pair_game_stats(&before);
        assert_eq!(s, before);
        // A hand-edited file with both on keeps the readout, when changed
        // and when loaded.
        let mut s = Settings { game_stats: true, ..Settings::default() };
        s.pair_game_stats(&Settings::default());
        assert!(s.game_stats && !s.hide_in_fullscreen);
        let s: Settings = serde_json::from_str(r#"{"game_stats":true,"hide_in_fullscreen":true}"#).unwrap();
        let s = s.sanitized();
        assert!(s.game_stats && !s.hide_in_fullscreen);
    }

    #[test]
    fn the_readout_stays_unless_peeks_are_picked() {
        // Files from 0.2.0 and before, and from the corner test build (its
        // "peek" was its default, not a choice), get the readout that stays.
        for old in [r#"{"game_stats":true}"#, r#"{"game_stats":true,"game_readout":"peek"}"#, r#"{"game_readout":"corner"}"#] {
            let s: Settings = serde_json::from_str(old).unwrap();
            assert!(!s.game_peek, "{old}");
        }
        let s: Settings = serde_json::from_str(r#"{"game_stats":true,"game_peek":true}"#).unwrap();
        assert!(s.game_peek);
    }

    #[test]
    fn the_island_never_steps_aside() {
        assert!(!Settings::default().step_aside);
        // 0.2.4 and 0.2.5 saved Step aside, their default: the island stays now.
        let old: Settings = serde_json::from_str(r#"{"settings_version":3,"step_aside":true}"#).unwrap();
        assert!(!old.sanitized().step_aside);
    }

    #[test]
    fn the_ai_tab_hides_nothing_and_keeps_notices_off_until_asked() {
        let u = Settings::default().usage;
        assert_eq!(u, UsageSettings { hidden: vec![], added: vec![], notices: false, near_pct: 80 });
        // Files from before the AI tab, and a hand-written half, fill the rest.
        let old: Settings = serde_json::from_str(r#"{"settings_version":4}"#).unwrap();
        assert_eq!(old.usage, u);
        let s: Settings = serde_json::from_str(r#"{"usage":{"notices":true,"hidden":["codex","future"]}}"#).unwrap();
        assert!(s.usage.notices && s.usage.added.is_empty());
        assert_eq!(s.usage.near_pct, 80);
        // A newer Studio's service stays hidden when this one saves the file.
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.usage.hidden, ["codex", "future"]);
    }

    #[test]
    fn every_tab_shows_until_hidden() {
        assert!(Settings::default().hidden_tabs.is_empty());
        // Files from before, and a newer Studio's tab, which stays hidden
        // when this one saves the file.
        let old: Settings = serde_json::from_str(r#"{"settings_version":4}"#).unwrap();
        assert!(old.hidden_tabs.is_empty());
        let s: Settings = serde_json::from_str(r#"{"hidden_tabs":["today","future"]}"#).unwrap();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.hidden_tabs, ["today", "future"]);
    }

    #[test]
    fn ielts_talks_ngegas_until_told_otherwise() {
        assert_eq!(Settings::default().ielts_spice, Spice::Ngegas);
        let s: Settings = serde_json::from_str(r#"{"ielts_spice":"gaul"}"#).unwrap();
        assert_eq!(s.ielts_spice, Spice::Gaul);
        // A spice from a newer version is the default.
        let s: Settings = serde_json::from_str(r#"{"ielts_spice":"pedas"}"#).unwrap();
        assert_eq!(s.ielts_spice, Spice::Ngegas);
        assert!(serde_json::to_string(&Settings::default()).unwrap().contains(r#""ielts_spice":"ngegas""#));
    }

    #[test]
    fn roundtrip() {
        let s = Settings::default();
        let j = serde_json::to_string_pretty(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn record_choices_default_and_round_trip() {
        // Files from before the Record page get its defaults: this screen, the microphone.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.record, RecordSettings::default());
        assert_eq!(s.record.inputs(), crate::recorder::Inputs { camera: false, microphone: true, system_audio: false });
        let mut s = Settings::default();
        s.record = RecordSettings { target: crate::recorder::Target::Area, camera: true, microphone: false, system_audio: true };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains(r#""record":{"target":"area","camera":true,"microphone":false,"system_audio":true}"#), "{j}");
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(back.record, s.record);
    }
}
