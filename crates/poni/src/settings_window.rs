//! The Settings window's content: every setting Oyenza Studio has, in a
//! sidebar of sections (General, Appearance, Island, Music and lyrics,
//! Pop-ups, Tabs, Add-ons, Focus, Prayer, AI, Oyenza, About). The island's
//! gear and the menu's Settings… open it, in a window of its own: on Windows
//! a borderless one (`settings_host.rs`), on the Mac a real window with the
//! system's title bar (`mac/settings_host.rs`). Both draw this [`View`].
//!
//! It draws with the island's widgets ([`Ui`], the Settings page's switch
//! and picker) on the same [`AppState`], and changes settings the way the
//! island's pages do: `Command::SaveSettings` after a change, which the app
//! applies as a menu change is. Each section is a list of [`Block`]s, most
//! of them groups of rows ([`Item`]: a label, a dim line under it, and a
//! switch, a picker, a stepper or a button), laid out from the top; a click
//! comes back as an [`Act`] on the row's [`Key`] ([`apply`]).
//!
//! It follows the system's light or dark mode: the window's own surfaces in
//! [`Surfaces`], and everything on them through the Glass look of that mode,
//! which turns the island's white ink dark on the light window.

use crate::commands::Command;
use crate::gfx::{Canvas, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::settings::{draw_switch, picker, picker_width};
use crate::pages::PageKind;
use crate::pill::status;
use crate::state::AppState;
use crate::ui::{wid, wid_n, CursorKind, Hit, Input, MouseButton, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use crate::version::VERSION;
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::settings::{IdleStyle, MonitorChoice, PrayerSound, Theme};

/// The card, in DIPs. Windows' window gives it room for its shadow and is
/// less tall on a short screen (the content scrolls).
pub const CARD_W: f32 = 840.0;
pub const CARD_H: f32 = 580.0;
/// Its corners when the window draws its own (Windows).
const RADIUS: f32 = 14.0;
const SIDEBAR_W: f32 = 220.0;
/// The content column: its left and right padding, and where it starts.
const PAD: f32 = 28.0;
const TITLE_Y: f32 = 22.0;
const TITLE_H: f32 = 30.0;
const CONTENT_Y: f32 = 70.0;
/// Sidebar rows.
const NAV_Y: f32 = 64.0;
const NAV_H: f32 = 32.0;
const NAV_GAP: f32 = 2.0;
const NAV_TILE: f32 = 22.0;
/// Rows: one line, or a label over a dim line.
const ROW_H: f32 = 40.0;
const ROW_H_SUB: f32 = 52.0;
const ROW_PAD: f32 = 14.0;
const GROUP_TITLE_H: f32 = 24.0;
const GROUP_GAP: f32 = 18.0;
const GROUP_R: f32 = 10.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
const PICKER_H: f32 = 26.0;
const STEP_BTN: f32 = 26.0;
const STEP_VALUE_W: f32 = 84.0;
/// The Appearance section's theme cards.
const THEME_CARD_H: f32 = 154.0;
const THEME_PREVIEW_H: f32 = 74.0;
const THEME_GAP: f32 = 12.0;
/// A wheel notch scrolls this far.
const WHEEL_STEP: f32 = 48.0;

/// The window's own surfaces, painted as they are; text and widgets on
/// them go through the look of the same mode.
#[derive(Clone, Copy, Debug)]
pub struct Surfaces {
    pub ground: Color,
    sidebar: Color,
    group: Color,
    group_hover: Color,
    group_press: Color,
}

const DARK: Surfaces = Surfaces {
    ground: Color::hex(0x1C1C1E),
    sidebar: Color::hex(0x242426),
    group: Color::hex(0x2C2C2E),
    group_hover: Color::hex(0x333336),
    group_press: Color::hex(0x3A3A3D),
};
const LIGHT: Surfaces = Surfaces {
    ground: Color::hex(0xF2F2F7),
    sidebar: Color::hex(0xE6E6EB),
    group: Color::hex(0xFFFFFF),
    group_hover: Color::hex(0xF7F7F9),
    group_press: Color::hex(0xEDEDF1),
};

pub fn surfaces(light: bool) -> Surfaces {
    if light {
        LIGHT
    } else {
        DARK
    }
}

/// Lines and edges, in the look's ink.
const RULE: Color = Color::white(0.08);
const EDGE: Color = Color::white(0.10);
/// Behind the theme cards' little screens.
const WALLPAPER: Color = Color::hex(0x3B4A7A);

/// The window's sections, in the sidebar's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    Island,
    Music,
    Popups,
    Tabs,
    Addons,
    Focus,
    Prayer,
    Ai,
    Oyenza,
    About,
}

impl Section {
    pub const ALL: [Section; 12] = [
        Section::General,
        Section::Appearance,
        Section::Island,
        Section::Music,
        Section::Popups,
        Section::Tabs,
        Section::Addons,
        Section::Focus,
        Section::Prayer,
        Section::Ai,
        Section::Oyenza,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Island => "Island",
            Section::Music => "Music and lyrics",
            Section::Popups => "Pop-ups",
            Section::Tabs => "Tabs",
            Section::Addons => "Add-ons",
            Section::Focus => "Focus",
            Section::Prayer => "Prayer times",
            Section::Ai => "AI usage",
            Section::Oyenza => "Oyenza account",
            Section::About => "About",
        }
    }

    /// For the self-test's file names.
    pub fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Appearance => "appearance",
            Section::Island => "island",
            Section::Music => "music",
            Section::Popups => "popups",
            Section::Tabs => "tabs",
            Section::Addons => "addons",
            Section::Focus => "focus",
            Section::Prayer => "prayer",
            Section::Ai => "ai",
            Section::Oyenza => "oyenza",
            Section::About => "about",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::General => Icon::Gear,
            Section::Appearance => Icon::Eye,
            Section::Island => Icon::AppWindow,
            Section::Music => Icon::Music,
            Section::Popups => Icon::Volume,
            Section::Tabs => Icon::App,
            Section::Addons => Icon::Addons,
            Section::Focus => Icon::Timer,
            Section::Prayer => Icon::Moon,
            Section::Ai => Icon::Ai,
            Section::Oyenza => Icon::Oyenza,
            Section::About => Icon::Gear,
        }
    }

    /// The colour of its tile in the sidebar.
    fn tint(self) -> Color {
        match self {
            Section::General => Color::hex(0x8E8E93),
            Section::Appearance => Color::hex(0x5E5CE6),
            Section::Island => palette::BLUE,
            Section::Music => palette::PINK,
            Section::Popups => palette::CYAN,
            Section::Tabs => palette::GREEN,
            Section::Addons => Color::hex(0x0A84FF),
            Section::Focus => palette::ORANGE,
            Section::Prayer => Color::hex(0x30B0A0),
            Section::Ai => palette::PURPLE,
            Section::Oyenza => palette::ORANGE,
            Section::About => Color::hex(0x8E8E93),
        }
    }

    /// Prayer times has a section only while its add-on is on.
    pub fn shown(self, st: &AppState) -> bool {
        match self {
            Section::Prayer => st.prayer_on(),
            _ => true,
        }
    }
}

/// The sections the sidebar lists now.
pub fn sections(st: &AppState) -> Vec<Section> {
    Section::ALL.into_iter().filter(|s| s.shown(st)).collect()
}

// ---------------------------------------------------------------- rows

/// What a row changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Autostart,
    Sounds,
    Clock24,
    SystemHistory,
    IdleLook,
    HoverDelay,
    CloseDelay,
    Width,
    Display,
    Fullscreen,
    GameStats,
    GamePeek,
    Lyrics,
    Pronunciation,
    MediaActivity,
    LyricsOffset,
    HudVolume,
    HudPower,
    HudBluetooth,
    RememberPage,
    StartPage,
    Tab(PageKind),
    FocusMin,
    ShortBreak,
    LongBreak,
    LongEvery,
    AutoBreaks,
    AutoFocus,
    PrayerSound,
    PrayerCountdown,
    PrayerPause,
    PrayerFollow,
    AiNotices,
    AiNear,
    SettingsFile,
    DataFolder,
    ReleaseNotes,
    LyricsCredit,
}

/// A row's control, at its right end.
#[derive(Clone, Debug, PartialEq)]
pub enum Ctl {
    /// `None` = not known yet (Start with Windows until it's read): dimmed.
    Switch(Option<bool>),
    /// A picker; `None` = none of its choices (a value typed into the file).
    Choice(Vec<&'static str>, Option<usize>),
    /// − and + around the value.
    Stepper(String),
    /// A button with this label.
    Button(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: Key,
    pub label: String,
    pub sub: Option<String>,
    pub ctl: Ctl,
}

fn item(key: Key, label: &str, sub: Option<&str>, ctl: Ctl) -> Item {
    Item { key, label: label.to_string(), sub: sub.map(str::to_string), ctl }
}

fn switch(key: Key, label: &str, sub: Option<&str>, on: bool) -> Item {
    item(key, label, sub, Ctl::Switch(Some(on)))
}

/// A section's content, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// Rows on one card, with a caption above it.
    Group(Option<&'static str>, Vec<Item>),
    /// This copy's version, Check now, and a new version when there is one.
    Version,
    /// The four themes as cards with a small picture of each.
    Themes,
    /// The Oyenza account row (the island's Settings page has the same).
    Account,
    /// The Add-ons page itself.
    Addons,
    /// A dim line of text.
    Note(&'static str),
}

/// What a click on a row's control did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Toggle,
    Pick(usize),
    Step(i32),
    Press,
}

/// How long the pointer rests on the island before it opens (ms).
const HOVER: [(u32, &str); 3] = [(120, "Quick"), (220, "Normal"), (450, "Slow")];
/// How long it stays open after the pointer leaves (ms).
const CLOSE: [(u32, &str); 3] = [(0, "At once"), (400, "Short"), (1000, "Long")];
const DISPLAYS: [(MonitorChoice, &str); 2] = [(MonitorChoice::Primary, "Main display"), (MonitorChoice::Cursor, "Under the pointer")];
const FULLSCREEN: [(bool, &str); 2] = [(true, "Hide"), (false, "Stay on top")];
const SOUNDS: [(PrayerSound, &str); 3] = [(PrayerSound::Adzan, "Adzan"), (PrayerSound::Tone, "Tone"), (PrayerSound::Silent, "Silent")];
const THEMES: [(Theme, &str, &str); 4] = [
    (Theme::Classic, "Classic", "A black island at the top edge"),
    (Theme::Oyen, "Oyen", "Oyen lives in the island"),
    (Theme::Glass, "Glass", "Frosted glass over your screen"),
    (Theme::Pill, "Pill", "Floats just below the top edge"),
];
/// Start with Windows, or at login on a Mac (its menu bar says so).
const AUTOSTART: (&str, &str) = if cfg!(target_os = "macos") {
    ("Start at login", "Opens Oyenza Studio when you log in")
} else {
    ("Start with Windows", "Opens Oyenza Studio when you sign in")
};
const WIDTH_STEP: f32 = 20.0;
const OFFSET_STEP: i32 = 100;

fn chosen<T: PartialEq>(list: &[(T, &str)], v: T) -> Option<usize> {
    list.iter().position(|(x, _)| *x == v)
}

fn names<T>(list: &[(T, &'static str)]) -> Vec<&'static str> {
    list.iter().map(|(_, n)| *n).collect()
}

/// The preset nearest to `ms` (a hand-edited file's value lands on one).
fn nearest(list: &[(u32, &str)], ms: u32) -> Option<usize> {
    list.iter().enumerate().min_by_key(|(_, (v, _))| v.abs_diff(ms)).map(|(i, _)| i)
}

fn idle_name(s: IdleStyle, theme: Theme) -> &'static str {
    match s {
        IdleStyle::Clock => "Clock",
        IdleStyle::Eyes if theme == Theme::Oyen => "Oyen",
        IdleStyle::Eyes => "Eyes",
        IdleStyle::Minimal => "Minimal",
        IdleStyle::Hidden => "Hidden",
        IdleStyle::Status => "Status",
    }
}

/// The pages the island may open on: its tabs, but not the Add-ons "+".
fn start_pages(st: &AppState) -> Vec<PageKind> {
    crate::pages::tabs(st).into_iter().filter(|p| *p != PageKind::Addons).collect()
}

fn minutes(m: u32) -> String {
    format!("{m} min")
}

/// The lyrics' shift as people say it.
fn offset_text(ms: i32) -> String {
    match ms {
        0 => "On time".into(),
        _ => format!("{}{:.1} s", if ms > 0 { "−" } else { "+" }, ms.unsigned_abs() as f32 / 1000.0),
    }
}

/// What `section` shows for `st`.
pub fn blocks(section: Section, st: &AppState) -> Vec<Block> {
    let s = &st.settings;
    match section {
        Section::General => {
            let mut rows = vec![
                item(Key::Autostart, AUTOSTART.0, Some(AUTOSTART.1), Ctl::Switch(st.ui.autostart)),
                switch(Key::Sounds, "Sounds", Some("A soft sound for timers, drops and reminders"), s.sounds),
                switch(Key::Clock24, "24-hour clock", None, s.use_24h),
            ];
            if s.system_on() {
                rows.push(switch(Key::SystemHistory, "Keep System page history", Some("The charts keep the last minute while the page is closed"), s.system_history));
            }
            vec![Block::Version, Block::Group(None, rows)]
        }
        Section::Appearance => {
            let looks = status::looks(s.theme);
            let look_names = looks.iter().map(|l| idle_name(*l, s.theme)).collect();
            let shown = status::shown(s.theme, s.idle_style);
            vec![
                Block::Themes,
                Block::Group(
                    Some("Island at rest"),
                    vec![item(Key::IdleLook, "Idle look", Some("What the closed island shows"), Ctl::Choice(look_names, looks.iter().position(|l| *l == shown)))],
                ),
            ]
        }
        Section::Island => {
            // The Mac's island goes where the notch is; the game readout is Windows'.
            let mut shows = Vec::new();
            if cfg!(windows) {
                shows.push(item(Key::Display, "Display", Some("The screen the island hangs on"), Ctl::Choice(names(&DISPLAYS), chosen(&DISPLAYS, s.monitor))));
            }
            shows.push(item(Key::Fullscreen, "In full-screen apps", Some("Games and videos that fill the screen"), Ctl::Choice(names(&FULLSCREEN), chosen(&FULLSCREEN, s.hide_in_fullscreen))));
            if cfg!(windows) {
                shows.push(switch(Key::GameStats, "Show FPS, GPU and CPU over games", Some("Keeps the island on top of full-screen apps"), s.game_stats));
                shows.push(switch(Key::GamePeek, "Only for a few seconds", Some("Then the island hides and adds no delay to the game"), s.game_peek));
            }
            vec![
                Block::Group(
                    Some("Opening and closing"),
                    vec![
                        item(Key::HoverDelay, "Opens after", Some("How long the pointer rests on the island first"), Ctl::Choice(names(&HOVER), nearest(&HOVER, s.hover_delay_ms))),
                        item(Key::CloseDelay, "Closes", Some("Once the pointer leaves the open island"), Ctl::Choice(names(&CLOSE), nearest(&CLOSE, s.collapse_delay_ms))),
                        item(Key::Width, "Width when open", Some("Wider gives the pages more room"), Ctl::Stepper(format!("{:.0}", s.expanded_width))),
                    ],
                ),
                Block::Group(Some("Where it shows"), shows),
            ]
        }
        Section::Music => vec![
            Block::Group(
                Some("Lyrics"),
                vec![
                    switch(Key::Lyrics, "Synced lyrics", Some("Lyrics move with the song, from LRCLIB"), s.lyrics),
                    switch(Key::Pronunciation, "Lyrics pronunciation", Some("Romaji, pinyin and Korean under the lines"), s.lyrics_pronunciation),
                    item(Key::LyricsOffset, "Timing", Some("Shift every line earlier or later"), Ctl::Stepper(offset_text(s.lyrics_offset_ms))),
                ],
            ),
            Block::Group(Some("Closed island"), vec![switch(Key::MediaActivity, "Show what's playing", Some("The song and its cover on the closed island"), s.show_media_activity)]),
        ],
        Section::Popups => vec![Block::Group(
            Some("The island shows a pop-up for"),
            vec![
                switch(Key::HudVolume, "Volume", Some("When you change the volume or mute"), s.hud_volume),
                switch(Key::HudPower, "Charger and battery", Some("When you plug in, unplug or run low"), s.hud_power),
                switch(Key::HudBluetooth, "Bluetooth", Some("When headphones and other devices connect"), s.hud_bluetooth),
            ],
        )],
        Section::Tabs => {
            let pages = start_pages(st);
            let start = PageKind::from_key(&s.start_page);
            let start_name = if pages.contains(&start) { start.title() } else { pages.first().map_or("Music", |p| p.title()) };
            let mut open = vec![switch(Key::RememberPage, "Open where you left off", Some("The page you last had open"), s.remember_page)];
            if !s.remember_page {
                open.push(item(Key::StartPage, "Opens on", None, Ctl::Stepper(start_name.to_string())));
            }
            let tabs = PageKind::HIDEABLE.iter().map(|p| item(Key::Tab(*p), p.title(), None, Ctl::Switch(Some(!st.tab_hidden(*p))))).collect();
            vec![Block::Group(Some("When the island opens"), open), Block::Group(Some("Tabs in the island"), tabs), Block::Note("AI, Oyenza and the add-ons' pages always have a tab.")]
        }
        Section::Addons => vec![Block::Addons],
        Section::Focus => {
            let p = &s.pomodoro;
            let every = match p.long_every {
                0 => "Never".to_string(),
                1 => "1 session".to_string(),
                n => format!("{n} sessions"),
            };
            vec![
                Block::Group(
                    Some("Lengths"),
                    vec![
                        item(Key::FocusMin, "Focus", None, Ctl::Stepper(minutes(p.focus_min))),
                        item(Key::ShortBreak, "Short break", None, Ctl::Stepper(minutes(p.short_break_min))),
                        item(Key::LongBreak, "Long break", None, Ctl::Stepper(minutes(p.long_break_min))),
                        item(Key::LongEvery, "Long break after", Some("Focus sessions before a long break"), Ctl::Stepper(every)),
                    ],
                ),
                Block::Group(
                    Some("Starting by itself"),
                    vec![
                        switch(Key::AutoBreaks, "Breaks", Some("A break starts when a focus session ends"), p.auto_start_breaks),
                        switch(Key::AutoFocus, "Focus sessions", Some("The next session starts when a break ends"), p.auto_start_focus),
                    ],
                ),
            ]
        }
        Section::Prayer => {
            let p = &s.prayer;
            let countdown = if p.remind_before_min == 0 { "Off".to_string() } else { minutes(p.remind_before_min) };
            vec![
                Block::Group(
                    Some("When it's time"),
                    vec![
                        item(Key::PrayerSound, "Sound", Some("Subuh plays the tone"), Ctl::Choice(names(&SOUNDS), chosen(&SOUNDS, p.sound))),
                        switch(Key::PrayerPause, "Pause music for the adzan", None, p.pause_music),
                        item(Key::PrayerCountdown, "Countdown before each prayer", Some("On the closed island"), Ctl::Stepper(countdown)),
                    ],
                ),
                Block::Group(Some("Place"), vec![switch(Key::PrayerFollow, "Follow my location", Some("Windows' location picks the nearest place"), p.follow_location)]),
                Block::Note("Pick a place by hand on the island's Prayer page."),
            ]
        }
        Section::Ai => {
            let u = &s.usage;
            let mut rows = vec![switch(Key::AiNotices, "Heads-up notices", Some("When a plan's limit gets close, runs out or comes back"), u.notices)];
            if u.notices {
                rows.push(item(Key::AiNear, "Close to the limit at", None, Ctl::Stepper(format!("{} %", st.usage_near_pct()))));
            }
            vec![Block::Group(None, rows), Block::Note("Services, keys and the Oyenza Inference connect are on the AI tab.")]
        }
        Section::Oyenza => vec![Block::Account],
        Section::About => vec![
            Block::Version,
            Block::Group(
                None,
                vec![
                    item(Key::ReleaseNotes, "What's new", Some("This version's release notes"), Ctl::Button("Open")),
                    item(Key::LyricsCredit, "Lyrics", Some("Synced lyrics come from LRCLIB"), Ctl::Button("lrclib.net")),
                ],
            ),
            Block::Group(
                Some("Files"),
                vec![
                    item(Key::SettingsFile, "Settings file", Some("settings.json, the same settings as a file"), Ctl::Button("Open")),
                    item(Key::DataFolder, "Data folder", Some("Tasks, the Shelf and your Focus history"), Ctl::Button("Open")),
                ],
            ),
        ],
    }
}

/// Step a number of minutes: by 5, by 1 below 5.
fn step_min(v: u32, d: i32, lo: u32, hi: u32) -> u32 {
    let by = if (d < 0 && v <= 5) || (d > 0 && v < 5) { 1 } else { 5 };
    (v as i64 + d as i64 * by).clamp(lo as i64, hi as i64) as u32
}

/// Carry out `act` on the row `key`. Settings save through
/// `Command::SaveSettings`, as the island's pages save them.
pub fn apply(st: &mut AppState, key: Key, act: Act) {
    let theme = st.settings.theme;
    let s = &mut st.settings;
    let step = if let Act::Step(d) = act { d } else { 0 };
    match key {
        Key::Autostart => {
            if let Some(on) = st.ui.autostart {
                st.cmd(Command::SetAutostart(!on));
            }
            return;
        }
        Key::Sounds => s.sounds = !s.sounds,
        Key::Clock24 => s.use_24h = !s.use_24h,
        Key::SystemHistory => s.system_history = !s.system_history,
        Key::IdleLook => {
            let looks = status::looks(theme);
            match act {
                Act::Pick(i) if i < looks.len() => s.idle_style = looks[i],
                _ => return,
            }
        }
        Key::HoverDelay => match act {
            Act::Pick(i) if i < HOVER.len() => s.hover_delay_ms = HOVER[i].0,
            _ => return,
        },
        Key::CloseDelay => match act {
            Act::Pick(i) if i < CLOSE.len() => s.collapse_delay_ms = CLOSE[i].0,
            _ => return,
        },
        Key::Width => s.expanded_width = (s.expanded_width + step as f32 * WIDTH_STEP).clamp(480.0, 900.0),
        Key::Display => match act {
            Act::Pick(i) if i < DISPLAYS.len() => s.monitor = DISPLAYS[i].0,
            _ => return,
        },
        Key::Fullscreen => match act {
            Act::Pick(i) if i < FULLSCREEN.len() => s.hide_in_fullscreen = FULLSCREEN[i].0,
            _ => return,
        },
        Key::GameStats => s.game_stats = !s.game_stats,
        Key::GamePeek => s.game_peek = !s.game_peek,
        Key::Lyrics => s.lyrics = !s.lyrics,
        Key::Pronunciation => s.lyrics_pronunciation = !s.lyrics_pronunciation,
        Key::MediaActivity => s.show_media_activity = !s.show_media_activity,
        // + shows the lines later, as people hear a late line.
        Key::LyricsOffset => s.lyrics_offset_ms = (s.lyrics_offset_ms - step * OFFSET_STEP).clamp(-5000, 5000),
        Key::HudVolume => s.hud_volume = !s.hud_volume,
        Key::HudPower => s.hud_power = !s.hud_power,
        Key::HudBluetooth => s.hud_bluetooth = !s.hud_bluetooth,
        Key::RememberPage => s.remember_page = !s.remember_page,
        Key::StartPage => {
            let pages = start_pages(st);
            if pages.is_empty() || step == 0 {
                return;
            }
            let at = pages.iter().position(|p| p.key() == st.settings.start_page).unwrap_or(0) as i32;
            let next = (at + step).rem_euclid(pages.len() as i32) as usize;
            st.settings.start_page = pages[next].key().to_string();
        }
        Key::Tab(p) => {
            let hidden = &mut s.hidden_tabs;
            match hidden.iter().position(|k| k == p.key()) {
                Some(i) => {
                    hidden.remove(i);
                }
                None => hidden.push(p.key().to_string()),
            }
        }
        Key::FocusMin => s.pomodoro.focus_min = step_min(s.pomodoro.focus_min, step, 1, 180),
        Key::ShortBreak => s.pomodoro.short_break_min = (s.pomodoro.short_break_min as i32 + step).clamp(1, 60) as u32,
        Key::LongBreak => s.pomodoro.long_break_min = step_min(s.pomodoro.long_break_min, step, 1, 120),
        Key::LongEvery => s.pomodoro.long_every = (s.pomodoro.long_every as i32 + step).clamp(0, 12) as u32,
        Key::AutoBreaks => s.pomodoro.auto_start_breaks = !s.pomodoro.auto_start_breaks,
        Key::AutoFocus => s.pomodoro.auto_start_focus = !s.pomodoro.auto_start_focus,
        Key::PrayerSound => match act {
            Act::Pick(i) if i < SOUNDS.len() => s.prayer.sound = SOUNDS[i].0,
            _ => return,
        },
        Key::PrayerCountdown => s.prayer.remind_before_min = step_min(s.prayer.remind_before_min, step, 0, 60),
        Key::PrayerPause => s.prayer.pause_music = !s.prayer.pause_music,
        Key::PrayerFollow => s.prayer.follow_location = !s.prayer.follow_location,
        Key::AiNotices => s.usage.notices = !s.usage.notices,
        Key::AiNear => s.usage.near_pct = (s.usage.near_pct as i32 + step * 5).clamp(50, 95) as u8,
        Key::SettingsFile => {
            st.cmd(Command::OpenPath(crate::paths::settings_path().to_string_lossy().into_owned()));
            return;
        }
        Key::DataFolder => {
            st.cmd(Command::OpenPath(crate::paths::data_dir().to_string_lossy().into_owned()));
            return;
        }
        Key::ReleaseNotes => {
            st.cmd(Command::OpenUrl(crate::version::notes_url()));
            return;
        }
        Key::LyricsCredit => {
            st.cmd(Command::OpenUrl("https://lrclib.net".into()));
            return;
        }
    }
    st.cmd(Command::SaveSettings);
}

// ---------------------------------------------------------------- the view

/// Where the content goes in the window that shows it.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// The card: what the window shows of the content (DIPs).
    pub card: Rect,
    /// The window draws its own shadow, edge and close button (Windows' is
    /// borderless); the Mac's window has the system's.
    pub own_chrome: bool,
    /// Room at the top of the sidebar for the Mac's window buttons.
    pub top: f32,
}

/// What a frame of the window asked for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Painted {
    /// A click changed something: draw it again (after the app ran the
    /// commands it pushed).
    pub again: bool,
    /// The close button was clicked.
    pub close: bool,
}

/// The content and its input, whichever window shows it.
pub struct View {
    pub input: Input,
    hits: Vec<Hit>,
    pub section: Section,
    /// How far the content is scrolled (DIPs) and how tall it was last drawn.
    scroll: f32,
    content_h: f32,
    /// Something it shows changed: draw it again.
    pub dirty: bool,
}

impl View {
    pub fn new(section: Section) -> Self {
        View { input: Input::default(), hits: Vec::new(), section, scroll: 0.0, content_h: 0.0, dirty: true }
    }

    pub fn show_section(&mut self, section: Section) {
        if section != self.section {
            self.section = section;
            self.scroll = 0.0;
            self.input.focused = None;
        }
        self.dirty = true;
    }

    /// The hit under `p` in the last frame.
    pub fn hit_at(&self, p: Point) -> Option<Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(p)).copied()
    }

    /// The pointer's cursor over `p`.
    pub fn cursor_at(&self, p: Point) -> CursorKind {
        self.hit_at(p).map_or(CursorKind::Arrow, |h| h.cursor)
    }

    /// The pointer moved to `p`. True when what's under it changed (or a
    /// press is held): the window is drawn again only then.
    pub fn hover(&mut self, p: Point) -> bool {
        let was = self.input.mouse.and_then(|m| self.hit_at(m)).map(|h| h.id);
        let entered = self.input.mouse.is_none();
        self.input.mouse = Some(p);
        entered || self.input.down || was != self.hit_at(p).map(|h| h.id)
    }

    /// The pointer left the window.
    pub fn leave(&mut self) {
        self.input.mouse = None;
    }

    pub fn press(&mut self, p: Point) {
        self.input.mouse = Some(p);
        self.input.down = true;
        self.input.just_pressed = true;
        self.input.press_pos = Some(p);
        self.input.pressed_id = self.hit_at(p).map(|h| h.id);
    }

    /// The button came up at `p`: a click when it's still on what it went
    /// down on (it shows on the next frame).
    pub fn release(&mut self, p: Point) {
        self.input.mouse = Some(p);
        if let (Some(pid), Some(h)) = (self.input.pressed_id, self.hit_at(p).map(|h| h.id)) {
            if pid == h {
                self.input.clicked = Some((h, MouseButton::Left));
            }
        }
        if self.input.down {
            self.input.released = self.input.pressed_id.map(|id| (id, p));
        }
        self.input.down = false;
        self.input.pressed_id = None;
    }

    pub fn wheel(&mut self, p: Point, notches: f32) {
        self.input.mouse = Some(p);
        self.input.wheel += notches;
    }

    /// Draw the window on `c` for `st`, light or dark.
    pub fn draw(&mut self, c: &mut Canvas, st: &mut AppState, now: f64, frame: Frame, light: bool) -> Painted {
        self.dirty = false;
        // A section that went away (Prayer times switched off) is General.
        if !self.section.shown(st) {
            self.section = Section::General;
            self.scroll = 0.0;
        }
        let mut out = Painted::default();
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();
        {
            let mut ui = Ui::new(c, &mut self.input, &mut hits, now);
            match draw(&mut ui, st, self.section, frame, light, &mut self.scroll, &mut self.content_h) {
                Some(Nav::Section(s)) => {
                    self.section = s;
                    self.scroll = 0.0;
                    out.again = true;
                }
                Some(Nav::Close) => out.close = true,
                None => {}
            }
            out.again |= ui.repaint_requested();
        }
        self.hits = hits;
        self.input.end_frame();
        out
    }
}

// ---------------------------------------------------------------- drawing

/// Where a click in the window's frame goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav {
    Section(Section),
    Close,
}

/// The whole window: the card, the sidebar and the section. Returns the
/// section picked in the sidebar, or the close button.
fn draw(ui: &mut Ui, st: &mut AppState, section: Section, frame: Frame, light: bool, scroll: &mut f32, content_h: &mut f32) -> Option<Nav> {
    let card = frame.card;
    let sf = surfaces(light);
    // Windows' borderless window: its own shadow and rounded corners.
    let radius = if frame.own_chrome { RADIUS } else { 0.0 };
    ui.c.unthemed(|c| {
        if frame.own_chrome {
            let shadow = Color::black(if light { 0.30 } else { 0.55 });
            c.rrect_shadow(card.offset(0.0, 4.0), RADIUS, shadow, &crate::pill::shape::SHADOW_RAMP);
        }
        c.fill_rrect(card, radius, sf.ground);
    });
    // The sidebar: the card's left part in its own shade.
    let side = Rect::new(card.x, card.y, SIDEBAR_W, card.h);
    ui.c.clip(side);
    ui.c.unthemed(|c| c.fill_rrect(Rect::new(side.x, side.y, side.w + RADIUS, side.h), radius, sf.sidebar));
    ui.c.unclip();
    let px = 1.0 / ui.c.scale().max(0.5);
    ui.c.fill_rect(Rect::new(side.right() - px, side.y, px, side.h), RULE);
    if frame.own_chrome {
        ui.c.stroke_rrect(card.inset(px / 2.0, px / 2.0), RADIUS, EDGE, px);
    }

    let picked = draw_sidebar(ui, st, side, section, frame.top);
    let content = Rect::new(side.right() + PAD, card.y, card.right() - side.right() - 2.0 * PAD, card.h);
    ui.c.text(section.title(), Rect::new(content.x, card.y + TITLE_Y, content.w - 40.0, TITLE_H), &TextStyle::new(22.0).weight(Weight::Bold));
    let close = frame.own_chrome && draw_close(ui, card);

    let view = Rect::new(content.x - PAD / 2.0, card.y + CONTENT_Y, content.w + PAD, card.bottom() - CONTENT_Y - card.y - 1.0);
    let wheel = ui.wheel_in(view);
    let max_scroll = (*content_h - view.h + 16.0).max(0.0);
    *scroll = (*scroll - wheel * WHEEL_STEP).clamp(0.0, max_scroll);
    ui.c.clip(view);
    let mut y = view.y - *scroll;
    let x = content.x;
    for (bi, b) in blocks(section, st).into_iter().enumerate() {
        y = draw_block(ui, st, &b, bi, Rect::new(x, y, content.w, 0.0), view, &sf);
    }
    ui.c.unclip();
    *content_h = y + *scroll - view.y;
    // The edge the content scrolls under.
    if *scroll > 0.5 {
        ui.c.fill_rect(Rect::new(view.x, view.y, view.w, px), RULE);
    }
    if close {
        return Some(Nav::Close);
    }
    picked.filter(|p| *p != section).map(Nav::Section)
}

/// The close button at the card's top right (Windows). True when clicked.
fn draw_close(ui: &mut Ui, card: Rect) -> bool {
    let r = Rect::new(card.right() - 14.0 - 28.0, card.y + 14.0, 28.0, 28.0);
    let resp = ui.button(wid("settingswin.close"), r);
    if resp.hovered || resp.pressed {
        ui.c.fill_rrect(r, 8.0, palette::RED.with_alpha(if resp.pressed { 0.8 } else { 0.6 }));
    }
    let color = if resp.hovered { palette::ON_ACCENT } else { palette::TEXT_DIM };
    ui.c.icon(Icon::Close, Rect::from_center(r.center(), 14.0, 14.0), color);
    resp.clicked
}

/// The sidebar: Oyen's tile and the window's name, then a row per section
/// (About at the bottom). `top`: room for the Mac's window buttons.
fn draw_sidebar(ui: &mut Ui, st: &mut AppState, side: Rect, section: Section, top: f32) -> Option<Section> {
    let icon_c = Point::new(side.x + 20.0 + 12.0, side.y + top + 32.0);
    crate::oyen::app_icon(ui.c, icon_c, 24.0);
    ui.c.text("Settings", Rect::new(icon_c.x + 20.0, side.y + top + 20.0, side.w - 60.0, 24.0), &TextStyle::headline());
    let mut picked = None;
    let list = sections(st);
    let about_at = side.bottom() - 12.0 - NAV_H;
    let mut y = side.y + top + NAV_Y;
    for (i, s) in list.iter().enumerate() {
        let row_y = if *s == Section::About { about_at } else { y };
        let r = Rect::new(side.x + 10.0, row_y, side.w - 20.0, NAV_H);
        y += NAV_H + NAV_GAP;
        let resp = ui.button(wid_n("settingswin.nav", i as u64), r);
        if resp.clicked {
            picked = Some(*s);
        }
        let on = *s == section;
        if on || resp.hovered || resp.pressed {
            let a = if on { 0.12 } else if resp.pressed { 0.09 } else { 0.05 };
            ui.c.fill_rrect(r, 8.0, Color::white(a));
        }
        let tile = Rect::new(r.x + 8.0, r.center().y - NAV_TILE / 2.0, NAV_TILE, NAV_TILE);
        if *s == Section::About {
            crate::oyen::app_icon(ui.c, tile.center(), NAV_TILE);
        } else {
            ui.c.fill_rrect(tile, 6.0, s.tint());
            ui.c.icon(s.icon(), Rect::from_center(tile.center(), 14.0, 14.0), palette::ON_ACCENT);
        }
        let tx = tile.right() + 10.0;
        let style = TextStyle::body().weight(if on { Weight::Semibold } else { Weight::Regular }).color(if on { palette::TEXT } else { Color::white(0.82) });
        ui.c.text(s.title(), Rect::new(tx, r.y, (r.right() - tx - 6.0).max(0.0), r.h), &style);
    }
    picked
}

/// Where `r` meets the scrolled view (None = not on screen): widgets take
/// the pointer only there.
fn visible(r: Rect, view: Rect) -> Option<Rect> {
    let x0 = r.x.max(view.x);
    let y0 = r.y.max(view.y);
    let x1 = r.right().min(view.right());
    let y1 = r.bottom().min(view.bottom());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn row_h(it: &Item) -> f32 {
    if it.sub.is_some() {
        ROW_H_SUB
    } else {
        ROW_H
    }
}

/// How tall a block is drawn.
fn block_h(st: &AppState, b: &Block) -> f32 {
    match b {
        Block::Group(title, items) => title.map_or(0.0, |_| GROUP_TITLE_H) + items.iter().map(row_h).sum::<f32>(),
        Block::Version => {
            if st.update.offer.is_some() {
                112.0
            } else {
                72.0
            }
        }
        Block::Themes => GROUP_TITLE_H + THEME_CARD_H,
        Block::Account => 72.0,
        Block::Addons => crate::pages::addons::height(st),
        Block::Note(_) => 18.0,
    }
}

/// A group's card.
fn fill_group(ui: &mut Ui, r: Rect, color: Color) {
    ui.c.unthemed(|c| c.fill_rrect(r, GROUP_R, color));
}

/// Draw block `b` (the `bi`-th) from `at`'s top left, `at.w` wide; returns
/// where the next one starts.
fn draw_block(ui: &mut Ui, st: &mut AppState, b: &Block, bi: usize, at: Rect, view: Rect, sf: &Surfaces) -> f32 {
    let h = block_h(st, b);
    let r = Rect::new(at.x, at.y, at.w, h);
    let next = r.bottom() + GROUP_GAP;
    if visible(r.inset(-4.0, -4.0), view).is_none() {
        return next;
    }
    match b {
        Block::Group(title, items) => {
            let mut y = r.y;
            if let Some(t) = title {
                ui.c.text(t, Rect::new(r.x + 2.0, y, r.w, GROUP_TITLE_H - 6.0), &TextStyle::footnote().weight(Weight::Semibold).color(palette::TEXT_DIM));
                y += GROUP_TITLE_H;
            }
            let card = Rect::new(r.x, y, r.w, r.bottom() - y);
            fill_group(ui, card, sf.group);
            let accent = st.accent;
            let px = 1.0 / ui.c.scale().max(0.5);
            for (i, it) in items.iter().enumerate() {
                let rh = row_h(it);
                let row = Rect::new(card.x, y, card.w, rh);
                if i > 0 {
                    ui.c.fill_rect(Rect::new(row.x + ROW_PAD, row.y, row.w - ROW_PAD, px), RULE);
                }
                if let Some(act) = draw_row(ui, it, (bi, i), row, view, accent, i == 0, i + 1 == items.len()) {
                    apply(st, it.key, act);
                }
                y += rh;
            }
        }
        Block::Version => draw_version(ui, st, r, view, sf),
        Block::Themes => draw_themes(ui, st, r, view, sf),
        Block::Account => {
            fill_group(ui, r, sf.group);
            if visible(r, view).is_some_and(|v| v.h >= r.h - 0.5) {
                crate::pages::settings::draw_account(ui, st, r.inset(ROW_PAD, 14.0));
            }
        }
        Block::Addons => crate::pages::addons::draw(ui, st, r),
        Block::Note(t) => ui.c.text(t, Rect::new(r.x + 2.0, r.y, r.w, r.h), &TextStyle::footnote().color(palette::TEXT_FAINT)),
    }
    next
}

/// One row: its label (and dim line), and its control. Returns what a
/// click did.
#[allow(clippy::too_many_arguments)]
fn draw_row(ui: &mut Ui, it: &Item, at: (usize, usize), row: Rect, view: Rect, accent: Color, first: bool, last: bool) -> Option<Act> {
    let id = |what: &str| wid_n(&format!("settingswin.{what}.{}", at.0), at.1 as u64);
    let shown = visible(row, view);
    let inner = row.inset(ROW_PAD, 0.0);
    let mut act = None;
    // The control's room at the right end.
    let ctl_w = match &it.ctl {
        Ctl::Switch(_) => SWITCH_W,
        Ctl::Choice(names, _) => picker_width(ui, names),
        Ctl::Stepper(_) => 2.0 * STEP_BTN + STEP_VALUE_W,
        Ctl::Button(label) => ui.pill_width(label, true, true),
    };
    let ctl_x = inner.right() - ctl_w;
    match &it.ctl {
        Ctl::Switch(on) => {
            // The whole row toggles, as on the island's Settings page.
            if let (Some(_), Some(hit)) = (on, shown) {
                let resp = ui.button(id("row"), hit);
                if resp.hovered || resp.pressed {
                    let wash = Color::white(if resp.pressed { 0.07 } else { 0.04 });
                    fill_row(ui, row, wash, first, last);
                }
                if resp.clicked {
                    act = Some(Act::Toggle);
                }
            }
            let sw = Rect::new(ctl_x, row.center().y - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
            let alpha = if on.is_some() { 1.0 } else { 0.5 };
            ui.c.with_alpha(alpha, |c| draw_switch(c, sw, *on == Some(true), accent));
        }
        Ctl::Choice(names, sel) => {
            let y = row.center().y - PICKER_H / 2.0;
            if shown.is_some_and(|v| v.h >= row.h - 0.5) {
                if let Some(i) = picker(ui, &format!("settingswin.pick.{}.{}", at.0, at.1), ctl_x, y, PICKER_H, names, sel.unwrap_or(usize::MAX)) {
                    act = Some(Act::Pick(i));
                }
            } else {
                // Partly scrolled away: drawn, not clickable.
                let mut none = Input::default();
                let mut no_hits = Vec::new();
                let mut ghost = Ui::new(&mut *ui.c, &mut none, &mut no_hits, 0.0);
                let _ = picker(&mut ghost, "settingswin.ghost", ctl_x, y, PICKER_H, names, sel.unwrap_or(usize::MAX));
            }
        }
        Ctl::Stepper(value) => {
            let cy = row.center().y;
            let minus = Rect::new(ctl_x, cy - STEP_BTN / 2.0, STEP_BTN, STEP_BTN);
            let plus = Rect::new(inner.right() - STEP_BTN, cy - STEP_BTN / 2.0, STEP_BTN, STEP_BTN);
            for (r, icon, d, what) in [(minus, Icon::Minus, -1, "minus"), (plus, Icon::Plus, 1, "plus")] {
                let resp = match visible(r, view) {
                    Some(hit) => ui.button(id(what), hit),
                    None => Default::default(),
                };
                let fill = if resp.pressed {
                    palette::SURFACE_PRESSED
                } else if resp.hovered {
                    palette::SURFACE_HOVER
                } else {
                    palette::SURFACE
                };
                ui.c.fill_circle(r.center(), STEP_BTN / 2.0, fill);
                ui.c.icon(icon, Rect::from_center(r.center(), 12.0, 12.0), palette::TEXT);
                if resp.clicked {
                    act = Some(Act::Step(d));
                }
            }
            let vr = Rect::new(minus.right(), row.y, plus.x - minus.right(), row.h);
            ui.c.text(value, vr, &TextStyle::body().weight(Weight::Semibold).tabular().center());
        }
        Ctl::Button(label) => {
            let r = Rect::new(ctl_x, row.center().y - PILL_SMALL / 2.0, ctl_w, PILL_SMALL);
            let clicked = match visible(r, view).filter(|v| v.h >= r.h - 0.5) {
                Some(_) => ui.pill(id("button"), r, label, Some(Icon::Open), PillStyle::Utility).clicked,
                None => {
                    let mut none = Input::default();
                    let mut no_hits = Vec::new();
                    let mut ghost = Ui::new(&mut *ui.c, &mut none, &mut no_hits, 0.0);
                    ghost.pill(0, r, label, Some(Icon::Open), PillStyle::Utility);
                    false
                }
            };
            if clicked {
                act = Some(Act::Press);
            }
        }
    }
    // The label, and the dim line under it.
    let tw = (ctl_x - 16.0 - inner.x).max(0.0);
    match &it.sub {
        Some(sub) => {
            let cy = row.center().y;
            ui.c.text(&it.label, Rect::new(inner.x, cy - 19.0, tw, 19.0), &TextStyle::body());
            ui.c.text(sub, Rect::new(inner.x, cy + 1.0, tw, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
        }
        None => ui.c.text(&it.label, Rect::new(inner.x, row.y, tw, row.h), &TextStyle::body()),
    }
    act
}

/// A row's hover wash, rounded where the row meets the card's corners.
fn fill_row(ui: &mut Ui, row: Rect, c: Color, first: bool, last: bool) {
    if !(first || last) {
        ui.c.fill_rect(row, c);
        return;
    }
    ui.c.clip(row);
    let top = if first { row.y } else { row.y - GROUP_R };
    let bottom = if last { row.bottom() } else { row.bottom() + GROUP_R };
    ui.c.fill_rrect(Rect::new(row.x, top, row.w, bottom - top), GROUP_R, c);
    ui.c.unclip();
}

/// The version card: Oyen's tile, "Oyenza Studio" and this version, and
/// Check now; with a later version out, what it brings and how to get it
/// (the island's Update sheet does the same).
fn draw_version(ui: &mut Ui, st: &mut AppState, r: Rect, view: Rect, sf: &Surfaces) {
    fill_group(ui, r, sf.group);
    let head = Rect::new(r.x + ROW_PAD, r.y, r.w - 2.0 * ROW_PAD, 72.0);
    let cy = head.center().y;
    crate::oyen::app_icon(ui.c, Point::new(head.x + 20.0, cy), 40.0);
    let tx = head.x + 54.0;
    let clickable = visible(r, view).is_some_and(|v| v.h >= r.h - 0.5);
    let check = clickable.then(|| crate::pages::settings::check_slot(ui, st, head)).flatten();
    let right = check.map_or(head.right(), |b| b.x - 12.0);
    ui.c.text("Oyenza Studio", Rect::new(tx, cy - 20.0, (right - tx).max(0.0), 22.0), &TextStyle::headline());
    ui.c.text(&format!("Version {VERSION}"), Rect::new(tx, cy + 2.0, (right - tx).max(0.0), 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    if let Some(b) = check {
        crate::pages::settings::draw_check(ui, st, b);
    }
    let Some(rel) = st.update.offer.clone() else { return };
    let px = 1.0 / ui.c.scale().max(0.5);
    let row = Rect::new(head.x, head.bottom(), head.w, r.bottom() - head.bottom());
    ui.c.fill_rect(Rect::new(row.x, row.y, row.w, px), RULE);
    let accent = st.accent;
    // Windows with the Setup in: now or next start; otherwise its page (or the Mac's DMG).
    let mut buttons: Vec<(&str, Command, PillStyle)> = Vec::new();
    if st.update.ready {
        buttons.push(("Install now", Command::Update, PillStyle::Primary(accent)));
        let later = if st.update.next_start { "Set for next start" } else { "On next start" };
        buttons.push((later, Command::InstallOnNextStart, PillStyle::Utility));
    } else {
        buttons.push(("Download", Command::Update, PillStyle::Primary(accent)));
    }
    buttons.push(("What's new", Command::OpenUrl(rel.notes.clone()), PillStyle::Utility));
    let mut x = row.right();
    let by = row.center().y - PILL_SMALL / 2.0;
    let mut picked = None;
    for (i, (label, cmd, style)) in buttons.into_iter().enumerate().rev() {
        let w = ui.pill_width(label, false, true);
        x -= w;
        let b = Rect::new(x, by, w, PILL_SMALL);
        x -= PILL_GAP;
        let clicked = if clickable {
            ui.pill(wid_n("settingswin.update", i as u64), b, label, None, style).clicked
        } else {
            let mut none = Input::default();
            let mut no_hits = Vec::new();
            Ui::new(&mut *ui.c, &mut none, &mut no_hits, 0.0).pill(0, b, label, None, style);
            false
        };
        if clicked {
            picked = Some(cmd);
        }
    }
    let tw = (x - 12.0 - row.x).max(0.0);
    let cy = row.center().y;
    crate::activities::text_with_dim(ui, "Update available", &rel.version, Rect::new(row.x, cy - 19.0, tw, 19.0), TextStyle::body().weight(Weight::Semibold));
    ui.c.text(&rel.headline, Rect::new(row.x, cy + 1.0, tw, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    if let Some(c) = picked {
        st.cmd(c);
    }
}

/// The four themes, a card each: a small screen with the island on it as
/// the theme draws it, the name and a line.
fn draw_themes(ui: &mut Ui, st: &mut AppState, r: Rect, view: Rect, sf: &Surfaces) {
    ui.c.text("Theme", Rect::new(r.x + 2.0, r.y, r.w, GROUP_TITLE_H - 6.0), &TextStyle::footnote().weight(Weight::Semibold).color(palette::TEXT_DIM));
    let n = THEMES.len() as f32;
    let w = ((r.w - (n - 1.0) * THEME_GAP) / n).floor();
    let accent = st.accent;
    let mut picked = None;
    for (i, (theme, name, line)) in THEMES.iter().enumerate() {
        let c = Rect::new(r.x + i as f32 * (w + THEME_GAP), r.y + GROUP_TITLE_H, w, THEME_CARD_H);
        let resp = match visible(c, view) {
            Some(hit) => ui.button(wid_n("settingswin.theme", i as u64), hit),
            None => Default::default(),
        };
        let on = st.settings.theme == *theme;
        let fill = if resp.pressed {
            sf.group_press
        } else if resp.hovered {
            sf.group_hover
        } else {
            sf.group
        };
        fill_group(ui, c, fill);
        if on {
            ui.c.stroke_rrect(c.inset(1.0, 1.0), GROUP_R - 1.0, accent, 2.0);
        }
        let screen = Rect::new(c.x + 10.0, c.y + 10.0, c.w - 20.0, THEME_PREVIEW_H);
        ui.c.unthemed(|cv| draw_theme_preview(cv, *theme, screen));
        let tx = c.x + 12.0;
        let ty = screen.bottom() + 10.0;
        let tw = c.w - 24.0 - if on { 20.0 } else { 0.0 };
        ui.c.text(name, Rect::new(tx, ty, tw, 19.0), &TextStyle::body().weight(Weight::Semibold));
        ui.c.text(line, Rect::new(tx, ty + 20.0, c.w - 24.0, 34.0), &TextStyle::footnote().color(palette::TEXT_DIM).wrap().top());
        if on {
            let dot = Point::new(c.right() - 20.0, ty + 9.5);
            ui.c.fill_circle(dot, 8.0, accent);
            ui.c.icon(Icon::Check, Rect::from_center(dot, 10.0, 10.0), palette::ON_ACCENT);
        }
        if resp.clicked {
            picked = Some(*theme);
        }
    }
    if let Some(t) = picked.filter(|t| *t != st.settings.theme) {
        st.settings.theme = t;
        st.cmd(Command::SaveSettings);
    }
}

/// A theme's island on a small screen: hanging from its top edge, glass,
/// with Oyen's eyes, or floating below it. Drawn as it is, in either mode.
fn draw_theme_preview(c: &mut Canvas, theme: Theme, screen: Rect) {
    c.clip(screen);
    c.fill_rrect(screen, 7.0, WALLPAPER);
    c.fill_rrect(Rect::new(screen.x, screen.y + screen.h * 0.55, screen.w, screen.h), 7.0, Color::hex(0x56679C));
    let cx = screen.center().x;
    let clock = TextStyle::caption().weight(Weight::Semibold).tabular().center();
    match theme {
        Theme::Classic | Theme::Oyen | Theme::Glass => {
            let body = Rect::new(cx - 34.0, screen.y, 68.0, 20.0);
            let (fill, border) = match theme {
                Theme::Glass => (Color::white(0.30), Some(Color::white(0.45))),
                _ => (palette::ISLAND, None),
            };
            c.notch(body, 9.0, 5.0, fill, border);
            if theme == Theme::Oyen {
                // Oyen's eyes peek over the island's edge.
                for dx in [-6.0, 6.0] {
                    c.fill_rrect(Rect::from_center(Point::new(cx + dx, body.center().y), 5.0, 8.0), 2.5, palette::ORANGE);
                }
            } else {
                c.text("10:08", body, &clock.color(palette::TEXT));
            }
        }
        Theme::Pill => {
            let body = Rect::new(cx - 36.0, screen.y + 7.0, 72.0, 20.0);
            c.fill_rrect(body.offset(0.0, 2.0), 10.0, Color::black(0.25));
            c.fill_rrect(body, 10.0, palette::ISLAND);
            c.text("10:08", body, &clock.color(palette::TEXT));
        }
    }
    c.unclip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state() -> AppState {
        AppState::new(Settings::default(), Store::default())
    }

    fn rows(section: Section, st: &AppState) -> Vec<Item> {
        blocks(section, st)
            .into_iter()
            .flat_map(|b| match b {
                Block::Group(_, items) => items,
                _ => Vec::new(),
            })
            .collect()
    }

    fn row(section: Section, st: &AppState, key: Key) -> Option<Item> {
        rows(section, st).into_iter().find(|i| i.key == key)
    }

    #[test]
    fn every_setting_the_window_has_saves_through_the_app() {
        let mut st = state();
        for s in Section::ALL {
            for it in rows(s, &st) {
                st.cmds.clear();
                let act = match it.ctl {
                    Ctl::Switch(_) => Act::Toggle,
                    Ctl::Choice(..) => Act::Pick(0),
                    Ctl::Stepper(_) => Act::Step(1),
                    Ctl::Button(_) => Act::Press,
                };
                apply(&mut st, it.key, act);
                let saved = st.cmds.iter().any(|c| matches!(c, Command::SaveSettings));
                let opens = st.cmds.iter().any(|c| matches!(c, Command::OpenPath(_) | Command::OpenUrl(_)));
                match it.key {
                    // Start with Windows waits for its reading; the buttons open things.
                    Key::Autostart => assert!(st.cmds.is_empty()),
                    Key::SettingsFile | Key::DataFolder | Key::ReleaseNotes | Key::LyricsCredit => assert!(opens, "{:?}", it.key),
                    _ => assert!(saved, "{:?} didn't save", it.key),
                }
            }
        }
    }

    #[test]
    fn steppers_stay_in_range() {
        let mut st = state();
        for _ in 0..100 {
            apply(&mut st, Key::Width, Act::Step(1));
            apply(&mut st, Key::FocusMin, Act::Step(1));
            apply(&mut st, Key::LongEvery, Act::Step(1));
            apply(&mut st, Key::AiNear, Act::Step(1));
            apply(&mut st, Key::LyricsOffset, Act::Step(1));
        }
        let s = &st.settings;
        assert_eq!((s.expanded_width, s.pomodoro.focus_min, s.pomodoro.long_every, s.usage.near_pct, s.lyrics_offset_ms), (900.0, 180, 12, 95, -5000));
        for _ in 0..100 {
            apply(&mut st, Key::Width, Act::Step(-1));
            apply(&mut st, Key::FocusMin, Act::Step(-1));
            apply(&mut st, Key::PrayerCountdown, Act::Step(-1));
        }
        let s = &st.settings;
        assert_eq!((s.expanded_width, s.pomodoro.focus_min, s.prayer.remind_before_min), (480.0, 1, 0));
        // Minutes go by five, and by one below five.
        assert_eq!((step_min(25, -1, 1, 180), step_min(5, -1, 1, 180), step_min(3, 1, 1, 180), step_min(5, 1, 1, 180)), (20, 4, 4, 10));
    }

    #[test]
    fn delays_show_the_nearest_choice() {
        let mut st = state();
        let pick = |st: &AppState, k| match row(Section::Island, st, k).map(|i| i.ctl) {
            Some(Ctl::Choice(_, sel)) => sel,
            _ => None,
        };
        assert_eq!((pick(&st, Key::HoverDelay), pick(&st, Key::CloseDelay)), (Some(1), Some(0)));
        st.settings.hover_delay_ms = 2000;
        assert_eq!(pick(&st, Key::HoverDelay), Some(2));
        apply(&mut st, Key::HoverDelay, Act::Pick(0));
        assert_eq!(st.settings.hover_delay_ms, 120);
        // A pick past the list changes nothing.
        st.cmds.clear();
        apply(&mut st, Key::CloseDelay, Act::Pick(9));
        assert!(st.cmds.is_empty());
    }

    #[test]
    fn tabs_flip_and_the_start_page_cycles_through_the_tabs() {
        let mut st = state();
        apply(&mut st, Key::Tab(PageKind::Shelf), Act::Toggle);
        assert!(st.tab_hidden(PageKind::Shelf));
        apply(&mut st, Key::Tab(PageKind::Shelf), Act::Toggle);
        assert!(!st.tab_hidden(PageKind::Shelf));
        // Opens on shows only while the island doesn't open where it was left.
        assert!(row(Section::Tabs, &st, Key::StartPage).is_none());
        apply(&mut st, Key::RememberPage, Act::Toggle);
        assert!(row(Section::Tabs, &st, Key::StartPage).is_some());
        let pages = start_pages(&st);
        apply(&mut st, Key::StartPage, Act::Step(1));
        assert_eq!(st.settings.start_page, pages[1].key());
        apply(&mut st, Key::StartPage, Act::Step(-1));
        apply(&mut st, Key::StartPage, Act::Step(-1));
        assert_eq!(st.settings.start_page, pages[pages.len() - 1].key());
        assert!(!pages.contains(&PageKind::Addons) && !pages.contains(&PageKind::Settings));
    }

    #[test]
    fn prayer_has_a_section_only_while_its_add_on_is_on() {
        let st = state();
        assert!(!sections(&st).contains(&Section::Prayer));
        assert_eq!(sections(&st).first(), Some(&Section::General));
        assert_eq!(sections(&st).last(), Some(&Section::About));
    }

    #[test]
    fn idle_looks_follow_the_theme() {
        let mut st = state();
        let looks = |st: &AppState| match row(Section::Appearance, st, Key::IdleLook).map(|i| i.ctl) {
            Some(Ctl::Choice(names, _)) => names,
            _ => Vec::new(),
        };
        assert!(!looks(&st).contains(&"Status"));
        st.settings.theme = Theme::Pill;
        assert!(looks(&st).contains(&"Status"));
        st.settings.theme = Theme::Oyen;
        assert!(looks(&st).contains(&"Oyen"));
    }

    #[test]
    fn lyrics_timing_reads_as_earlier_or_later() {
        assert_eq!(offset_text(0), "On time");
        assert_eq!(offset_text(300), "−0.3 s");
        assert_eq!(offset_text(-1200), "+1.2 s");
    }
}
