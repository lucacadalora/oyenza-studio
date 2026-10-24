//! User settings, stored as JSON next to the data store.

use crate::pomodoro::PomodoroConfig;
use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorChoice {
    Primary,
    /// The monitor under the cursor when Poni starts.
    Cursor,
}

/// Bumped when defaults change in a way existing files should pick up.
pub const SETTINGS_VERSION: u32 = 3;

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
    pub clipboard_history: bool,
    pub sounds: bool,
    /// Page shown first when expanding ("media", "focus", "health", "shelf", "tasks", "clipboard", "today").
    pub start_page: String,
    /// Remember the last page instead of `start_page`.
    pub remember_page: bool,
    pub pomodoro: PomodoroConfig,
    /// Oyenza (once signed in): show sums and senders on the island straight
    /// away. Off: they wait for a click on Show, so a shared screen shows none.
    pub oyenza_show_amounts: bool,
    /// Keep the System page's charts going while it's closed (a light reading
    /// of the kernel counters once a second), so they're full when it opens.
    pub system_history: bool,
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
            monitor: MonitorChoice::Primary,
            show_media_activity: true,
            lyrics: true,
            lyrics_pronunciation: true,
            lyrics_offset_ms: 0,
            hud_volume: true,
            hud_power: true,
            hud_bluetooth: true,
            clipboard_history: true,
            sounds: true,
            start_page: "media".into(),
            remember_page: true,
            pomodoro: PomodoroConfig::default(),
            oyenza_show_amounts: false,
            system_history: true,
        }
    }
}

impl Settings {
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
        self.settings_version = self.settings_version.max(SETTINGS_VERSION);
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
    fn theme_defaults_to_classic() {
        // Files written before themes existed keep the black island.
        let s: Settings = serde_json::from_str(r#"{"idle_style":"eyes"}"#).unwrap();
        assert_eq!(s.theme, Theme::Classic);
        let s: Settings = serde_json::from_str(r#"{"theme":"oyen"}"#).unwrap();
        assert_eq!(s.theme, Theme::Oyen);
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
    fn roundtrip() {
        let s = Settings::default();
        let j = serde_json::to_string_pretty(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}
