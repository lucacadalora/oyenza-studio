//! Notifications: other apps' notifications in the island (the Windows app's
//! Notifications add-on; Windows' side is `services::notifications`).
//!
//! Windows tells the app about each new notification: the app that sent it
//! (its AppUserModelID and name) and its lines of text. This module decides
//! which apps the island shows (the person's picks on the Notifications
//! page; WhatsApp, Telegram and Phone Link are on until they say otherwise)
//! and what the notice says: who it's from, then the message, either of which
//! can wait for a click on Show, as a sum does in a Money in notice. Nothing
//! is kept: the words live in the notice for its few seconds.

use crate::geom::Color;
use serde::{Deserialize, Serialize};

/// The longest line a notice keeps, in characters: a long message is cut.
pub const LINE_CHARS: usize = 160;
/// Apps the page remembers; past this the oldest one switched off goes.
pub const MAX_APPS: usize = 24;
/// How long a notice shows, in seconds (as long as a Money in notice).
pub const NOTICE_S: f64 = 8.0;
/// What the subtitle says while the message waits for Show.
pub const MESSAGE_HIDDEN: &str = "Message hidden";
/// ...and while who it's from waits too.
pub const BOTH_HIDDEN: &str = "Sender and message hidden";

/// The chat apps' own colours, for the notice's disc.
pub const WHATSAPP: Color = Color::hex(0x25D366);
pub const TELEGRAM: Color = Color::hex(0x2AABEE);
pub const PHONE_LINK: Color = Color::hex(0x0078D4);

/// One notification, as Windows describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The app's AppUserModelID ("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App").
    pub app_id: String,
    /// The app's name as Windows shows it ("WhatsApp").
    pub app: String,
    /// The notification's lines, in order; the first is usually who it's from.
    pub lines: Vec<String>,
}

/// An app on the Notifications page, and whether the island shows its notifications.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppChoice {
    pub id: String,
    pub name: String,
    pub on: bool,
}

/// The Notifications add-on's choices (`Settings::notifications`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    /// Show who a message is from. Off: the app's name, and Show for the rest.
    pub who: bool,
    /// Show the message itself. Off: it waits for Show.
    pub text: bool,
    /// The apps that sent a notification since the add-on came on, in the
    /// order they first did.
    pub apps: Vec<AppChoice>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { who: true, text: true, apps: Vec::new() }
    }
}

impl NotificationSettings {
    /// Whether the island shows `app_id`'s notifications. An app it hasn't
    /// seen joins the list (on for the chat apps, off for the rest); one it
    /// has keeps its switch and takes a new name. Returns (shows, list changed).
    pub fn remember(&mut self, app_id: &str, app: &str) -> (bool, bool) {
        let name = clean(app);
        if let Some(a) = self.apps.iter_mut().find(|a| a.id == app_id) {
            let renamed = !name.is_empty() && a.name != name;
            if renamed {
                a.name = name;
            }
            return (a.on, renamed);
        }
        let on = starts_on(app_id, app);
        let name = if name.is_empty() { app_id.to_string() } else { name };
        self.apps.push(AppChoice { id: app_id.to_string(), name, on });
        if self.apps.len() > MAX_APPS {
            let i = self.apps.iter().position(|a| !a.on).unwrap_or(0);
            self.apps.remove(i);
        }
        (on, true)
    }

    /// Switch one app on or off (the page's switches).
    pub fn set(&mut self, app_id: &str, on: bool) {
        if let Some(a) = self.apps.iter_mut().find(|a| a.id == app_id) {
            a.on = on;
        }
    }
}

/// The chat apps the island shows from the start, by a word in their id or name.
fn chat(app_id: &str, app: &str) -> Option<Color> {
    let id = app_id.to_ascii_lowercase();
    let name = app.to_lowercase();
    if id.contains("whatsapp") || name.contains("whatsapp") {
        Some(WHATSAPP)
    } else if id.contains("telegram") || name.contains("telegram") {
        Some(TELEGRAM)
    } else if id.contains("yourphone") || name.contains("phone link") {
        Some(PHONE_LINK)
    } else {
        None
    }
}

/// On the list from its first notification: WhatsApp, Telegram, Phone Link.
pub fn starts_on(app_id: &str, app: &str) -> bool {
    chat(app_id, app).is_some()
}

/// The notice's colour for a chat app (None: the island's accent).
pub fn color(app_id: &str, app: &str) -> Option<Color> {
    chat(app_id, app)
}

/// What a notice says for one notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    /// Who it's from, or the app's name.
    pub title: String,
    /// The app's name after who it's from (empty when the title is the app).
    pub title_dim: String,
    /// The message, or what waits for Show.
    pub subtitle: String,
    /// What Show puts in the subtitle's place (None: nothing waits).
    pub reveal: Option<String>,
}

/// The notice for `seen`, with who it's from (`who`) and the message (`text`)
/// shown or waiting for Show. None for a notification with no words.
pub fn compose(seen: &Seen, who: bool, text: bool) -> Option<Said> {
    let lines: Vec<String> = seen.lines.iter().map(|l| clean(l)).filter(|l| !l.is_empty()).collect();
    let (from, message) = match lines.split_first() {
        None => return None,
        // One line is the message itself (a group's "3 new messages").
        Some((only, [])) => (None, only.clone()),
        Some((first, rest)) => (Some(first.clone()), cut(&rest.join(" "))),
    };
    let app = match clean(&seen.app) {
        a if a.is_empty() => "An app".to_string(),
        a => a,
    };
    let said = match (from, who) {
        (Some(from), true) => Said {
            title: from,
            title_dim: app,
            subtitle: if text { message.clone() } else { MESSAGE_HIDDEN.into() },
            reveal: (!text).then_some(message),
        },
        (Some(from), false) => Said {
            title: app,
            title_dim: String::new(),
            subtitle: if text { message.clone() } else { BOTH_HIDDEN.into() },
            reveal: (!text).then(|| cut(&format!("{from}: {message}"))),
        },
        (None, _) => Said {
            title: app,
            title_dim: String::new(),
            subtitle: if text { message.clone() } else { MESSAGE_HIDDEN.into() },
            reveal: (!text).then_some(message),
        },
    };
    Some(said)
}

/// One line for the island: runs of spaces, line breaks and control
/// characters become one space, invisible direction marks go (WhatsApp wraps
/// phone numbers in them), and a long line is cut at [`LINE_CHARS`].
pub fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if invisible(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            gap = !out.is_empty();
            continue;
        }
        if gap {
            out.push(' ');
            gap = false;
        }
        out.push(c);
    }
    cut(&out)
}

/// `s` cut at [`LINE_CHARS`] characters, with an ellipsis.
fn cut(s: &str) -> String {
    match s.char_indices().nth(LINE_CHARS) {
        Some((i, _)) => format!("{}\u{2026}", s[..i].trim_end()),
        None => s.to_string(),
    }
}

/// Zero-width characters and the marks that steer text direction.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    const WA: &str = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App";
    const TG: &str = "Telegram.TelegramDesktop";
    const PL: &str = "Microsoft.YourPhone_8wekyb3d8bbwe!App";
    const OUTLOOK: &str = "Microsoft.Office.OUTLOOK.EXE.15";

    fn seen(app_id: &str, app: &str, lines: &[&str]) -> Seen {
        Seen { app_id: app_id.into(), app: app.into(), lines: lines.iter().map(|l| l.to_string()).collect() }
    }

    #[test]
    fn chat_apps_start_on_and_others_off() {
        assert!(starts_on(WA, "WhatsApp"));
        assert!(starts_on(TG, "Telegram Desktop"));
        assert!(starts_on("TelegramMessengerLLP.TelegramDesktop_t4vj0pshhgkwm!Telegram.TelegramDesktop.Store", "Telegram"));
        assert!(starts_on(PL, "Phone Link"));
        assert!(starts_on("Some.Id", "WhatsApp Beta"), "found by its name too");
        assert!(!starts_on(OUTLOOK, "Outlook"));
        assert!(!starts_on("Windows.SystemToast.SecurityAndMaintenance", "Security and Maintenance"));
        assert_eq!(color(WA, "WhatsApp"), Some(WHATSAPP));
        assert_eq!(color(TG, "Telegram"), Some(TELEGRAM));
        assert_eq!(color(PL, "Phone Link"), Some(PHONE_LINK));
        assert_eq!(color(OUTLOOK, "Outlook"), None);
    }

    #[test]
    fn apps_are_remembered_once_with_their_switch() {
        let mut s = NotificationSettings::default();
        assert_eq!(s.remember(WA, "WhatsApp"), (true, true), "a chat app joins, on");
        assert_eq!(s.remember(OUTLOOK, "Outlook"), (false, true), "another app joins, off");
        assert_eq!(s.remember(WA, "WhatsApp"), (true, false), "seen before: nothing changes");
        s.set(WA, false);
        assert_eq!(s.remember(WA, "WhatsApp"), (false, false), "switched off by hand stays off");
        assert_eq!(s.remember(WA, "WhatsApp Beta"), (false, true), "a new name is kept");
        assert_eq!(s.apps[0].name, "WhatsApp Beta");
        assert_eq!(s.remember(OUTLOOK, ""), (false, false), "no name: the old one stays");
        assert_eq!(s.apps.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), [WA, OUTLOOK], "in the order they came");
        s.remember("No.Name", " ");
        assert_eq!(s.apps[2].name, "No.Name", "an app without a name shows its id");
    }

    #[test]
    fn a_long_list_forgets_the_oldest_app_switched_off() {
        let mut s = NotificationSettings::default();
        s.remember(WA, "WhatsApp");
        for i in 0..MAX_APPS {
            s.remember(&format!("App.{i}"), &format!("App {i}"));
        }
        assert_eq!(s.apps.len(), MAX_APPS);
        assert_eq!(s.apps[0].id, WA, "the chat app that's on stays");
        assert!(s.apps.iter().all(|a| a.id != "App.0"), "the oldest one switched off went");
    }

    #[test]
    fn a_message_shows_who_and_what() {
        let said = compose(&seen(WA, "WhatsApp", &["Mom", "Dinner at 7 tonight?"]), true, true).unwrap();
        assert_eq!(said, Said { title: "Mom".into(), title_dim: "WhatsApp".into(), subtitle: "Dinner at 7 tonight?".into(), reveal: None });
        // Later lines join the message.
        let said = compose(&seen(TG, "Telegram", &["Design team", "Rina:", "the deck is up"]), true, true).unwrap();
        assert_eq!(said.subtitle, "Rina: the deck is up");
    }

    #[test]
    fn hidden_parts_wait_for_show() {
        let n = seen(WA, "WhatsApp", &["Mom", "Dinner at 7 tonight?"]);
        let said = compose(&n, true, false).unwrap();
        assert_eq!((said.title.as_str(), said.subtitle.as_str()), ("Mom", MESSAGE_HIDDEN));
        assert_eq!(said.reveal.as_deref(), Some("Dinner at 7 tonight?"));
        let said = compose(&n, false, true).unwrap();
        assert_eq!((said.title.as_str(), said.title_dim.as_str(), said.subtitle.as_str()), ("WhatsApp", "", "Dinner at 7 tonight?"));
        assert_eq!(said.reveal, None);
        let said = compose(&n, false, false).unwrap();
        assert_eq!((said.title.as_str(), said.subtitle.as_str()), ("WhatsApp", BOTH_HIDDEN));
        assert_eq!(said.reveal.as_deref(), Some("Mom: Dinner at 7 tonight?"));
    }

    #[test]
    fn one_line_is_the_message_and_no_words_no_notice() {
        let said = compose(&seen(TG, "Telegram", &["3 new messages"]), true, true).unwrap();
        assert_eq!((said.title.as_str(), said.title_dim.as_str(), said.subtitle.as_str()), ("Telegram", "", "3 new messages"));
        assert_eq!(compose(&seen(TG, "Telegram", &[]), true, true), None);
        assert_eq!(compose(&seen(TG, "Telegram", &["  ", "\u{200E}"]), true, true), None);
        assert_eq!(compose(&seen(TG, "", &["Hi"]), true, true).unwrap().title, "An app");
    }

    #[test]
    fn lines_are_cleaned_and_cut() {
        assert_eq!(clean("  Dinner\n at\t7 \r\n tonight?  "), "Dinner at 7 tonight?");
        assert_eq!(clean("\u{202A}+62 812-3456-7890\u{202C}"), "+62 812-3456-7890");
        assert_eq!(clean("a\u{200B}b\u{FEFF}c"), "abc");
        let long = "word ".repeat(100);
        let cut = clean(&long);
        assert_eq!(cut.chars().count(), LINE_CHARS, "cut at the limit, ellipsis included");
        assert!(cut.ends_with('\u{2026}'));
        // Cutting counts characters, not bytes.
        let emoji = "\u{1F602}".repeat(LINE_CHARS + 5);
        assert!(clean(&emoji).starts_with('\u{1F602}') && clean(&emoji).ends_with('\u{2026}'));
    }

    #[test]
    fn settings_round_trip_and_missing_fields_get_defaults() {
        let mut s = NotificationSettings::default();
        s.remember(WA, "WhatsApp");
        s.text = false;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<NotificationSettings>(&json).unwrap(), s);
        let old: NotificationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, NotificationSettings::default());
        assert!(old.who && old.text && old.apps.is_empty());
    }
}
