//! The AI tab: how much of each AI coding plan is left, one card per
//! service on this computer (`studio_usage`, read by `services::usage`
//! into `st.usage`; the person's choices are `settings.usage`).
//!
//! A card: the service's mark, name, plan and masked account; a row per
//! limit (its name, when it resets, a bar and how much is used; "in 2 h 10
//! min" under the pointer); money left and spent; Oyenza Inference's last
//! days as small bars. Numbers read from the app's own log, or a while ago,
//! say so ("From Codex's log, 2 h ago"). A trouble says what's wrong in
//! plain words with the one button that fixes it (Add key, Connect, Try
//! again), and a plan that ran out points to Oyenza Inference. A service
//! that isn't on this computer (or has no key) has no card unless the
//! person added it, and "not asked online yet" is never a trouble.
//!
//! The cards keep the services' own order (`ProviderId::ALL`) rather than
//! the tightest first: the numbers refresh every minute while the tab is
//! open, and a card that moved would move under the pointer. The bars'
//! colours already show which plan is close (orange from the person's
//! notice line, red when it's used up).
//!
//! Above the cards: "Tell me before I run out" (the heads-up notices),
//! when the numbers were read, Refresh, and Services, the list where each
//! service is shown or hidden, a key is typed (masked) for the services
//! that take one, and Oyenza Inference connects with a code approved in its
//! dashboard. All of it goes out as `Command`s: the page never calls the
//! worker. The page is as tall as the System page and scrolls inside, so
//! the window never grows for it, and nothing on it moves by itself.

use crate::commands::{Command, UsageCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::state::{AppState, InferenceLink, Reading};
use crate::ui::{wid, wid_n, CursorKind, Input, KeyInput, PillStyle, Ui, WidgetId, PILL_GAP, PILL_SMALL};
use poni_core::datetime::{civil_from_days, fmt_clock, fmt_date_short, LocalDateTime, MONTH_SHORT, WEEKDAY_SHORT};
use poni_core::geom::{palette, Color, Point, Rect};
use studio_usage::{self as usage, oyenza, DayUse, KeyAsk, Money, ProviderId, Snapshot, Source, Spend, Trouble, Window};

/// The System page's height: the window never grows for this page, and the
/// cards scroll inside it.
const HEIGHT: f32 = 256.0;
/// The row above the cards, and where the cards (or the Services list) start.
const TOP_H: f32 = 28.0;
const BODY_Y: f32 = 36.0;
/// Between cards, and inside one.
const GAP: f32 = 8.0;
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 10.0;
const CARD_R: f32 = 14.0;
/// Cards sit two to a row while each gets this wide (three on a wide island).
const CARD_MIN_W: f32 = 210.0;
/// The card's name line, and the room under it.
const HEAD_H: f32 = 20.0;
const HEAD_GAP: f32 = 6.0;
const MARK: f32 = 16.0;
/// A limit: its words, then its bar under them.
const LIMIT_H: f32 = 28.0;
const WORDS_H: f32 = 15.0;
const BAR_Y: f32 = 18.0;
const BAR_H: f32 = 4.0;
const BALANCE_H: f32 = 26.0;
const SPENDS_H: f32 = 34.0;
const LINE_H: f32 = 18.0;
const NOTE_H: f32 = 16.0;
const DAYS_H: f32 = 26.0;
const CODE_BOX_H: f32 = 48.0;
/// The Services list: its privacy line, a row per service, the key field
/// under the row it's open for.
const INTRO_GAP: f32 = 4.0;
const ROW_H: f32 = 36.0;
const EDIT_H: f32 = 62.0;
const FIELD_H: f32 = 32.0;
const NAME_X: f32 = 28.0;
const NAME_W: f32 = 124.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
/// Scrolling: a wheel notch, the glide (as Stocks'), the fades at a cut edge.
const WHEEL_STEP: f32 = 48.0;
const GLIDE: f32 = 16.0;
const FADE_H: f32 = 14.0;
/// Numbers older than this say how old they are (they refresh every couple
/// of minutes while the tab is open).
const OLD_S: i64 = 10 * 60;
/// The longest key the field takes (Oyenza's own keys stop at 256).
const KEY_MAX: usize = 256;
/// A page drawn again after this long starts over (as Today does).
const STALE_S: f64 = 1.0;
/// Oyenza's colour, as on the Oyenza page.
const OYENZA: Color = palette::ORANGE;
const CHECKING: &str = "Checking…";

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The AI tab's own state between frames (`st.ui.usage`).
#[derive(Clone, Default)]
pub struct UsageUi {
    /// The Services list shows instead of the cards.
    pub services: bool,
    /// The service whose key field is open in the Services list.
    pub editing: Option<ProviderId>,
    /// What's typed in it, shown only as dots, and dropped once it's saved.
    key: String,
    /// The key field takes the keyboard on its next frame.
    focus: bool,
    /// Bring this one into view on the next frame: its card (Connect was
    /// pressed elsewhere), or its row in the Services list.
    reveal: Option<ProviderId>,
    /// The cards' or the list's scroll: where the wheel sent it, and where
    /// it shows on its way there.
    scroll: f32,
    shown_scroll: f32,
    /// 1 right after a switch between the cards and the list, easing to 0
    /// as the new one fades in.
    swap: f32,
    /// When the page last drew (mono seconds): the glides' step.
    drawn: f64,
    /// Still pictures (the self-test, the Mac renders): every glide is at
    /// its end.
    still: bool,
}

/// Never shows the key being typed: the page state can end up printed.
impl std::fmt::Debug for UsageUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageUi")
            .field("services", &self.services)
            .field("editing", &self.editing)
            .field("key", &format_args!("<{} chars>", self.key.chars().count()))
            .field("scroll", &self.scroll)
            .finish_non_exhaustive()
    }
}

impl UsageUi {
    /// From the next frame on, every glide shows at its end, so the
    /// self-test's and the Mac renders' pictures hold still.
    pub fn settle(&mut self, now: f64) {
        self.still = true;
        self.drawn = now;
        self.swap = 0.0;
        self.shown_scroll = self.scroll;
    }

    /// The cards, or the Services list: from the top, fading in.
    fn switch(&mut self, services: bool) {
        self.services = services;
        self.scroll = 0.0;
        self.shown_scroll = 0.0;
        self.swap = if self.still { 0.0 } else { 1.0 };
        if !services {
            self.close_key();
        }
    }

    fn close_key(&mut self) {
        self.editing = None;
        self.key.clear();
        self.focus = false;
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // A fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let now = ui.now;
    let s = &mut st.ui.usage;
    // Opened again after a while (the island closed, or another tab showed):
    // the cards from the top, and a key half typed is dropped.
    if !s.still && now - s.drawn > STALE_S {
        s.switch(false);
        s.swap = 0.0;
        s.reveal = None;
    }
    // After an idle gap (the frame clock was parked) step as if one frame
    // passed; still pictures jump to the end.
    let dt = if s.still {
        1.0
    } else if now > s.drawn {
        (now - s.drawn).min(1.0 / 30.0) as f32
    } else {
        0.0
    };
    s.drawn = now;
    let mut moving = false;
    s.swap = glide(s.swap, 0.0, dt, 0.01, &mut moving);
    let fade_in = ease_out(1.0 - s.swap);
    let services = s.services;
    top(ui, st, Rect::new(area.x, area.y, area.w, TOP_H));
    let body = Rect::new(area.x, area.y + BODY_Y, area.w, area.h - BODY_Y);
    let base = ui.c.alpha();
    ui.c.set_alpha(base * fade_in);
    if services {
        list(ui, st, body, dt, &mut moving);
    } else {
        cards(ui, st, body, dt, &mut moving);
    }
    ui.c.set_alpha(base);
    if moving {
        ui.request_anim();
    }
}

// ---------------------------------------------------------------- the top row

/// The notices switch on the left; when the numbers were read, Refresh and
/// Services (Done while the list shows) on the right.
fn top(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let services = st.ui.usage.services;
    let (label, icon, style) = if services { ("Done", None, PillStyle::Neutral) } else { ("Services", Some(Icon::Sliders), PillStyle::Utility) };
    let pw = ui.pill_width(label, icon.is_some(), true);
    let pill = Rect::new(r.right() - pw, r.center().y - PILL_SMALL / 2.0, pw, PILL_SMALL);
    if ui.pill(wid("usage.services"), pill, label, icon, style).clicked {
        if services {
            ui.blur();
        }
        st.ui.usage.switch(!services);
    }
    let busy = st.usage.refreshing;
    let refresh = Rect::new(pill.x - PILL_GAP - PILL_SMALL, pill.y, PILL_SMALL, PILL_SMALL);
    if ui.icon_button(wid("usage.refresh"), refresh, Icon::Reset, if busy { palette::DISABLED } else { palette::TEXT_DIM }, 15.0).clicked && !busy {
        st.cmd(Command::Usage(UsageCmd::Refresh));
    }

    // Label then switch, as Settings' rows; the whole of it toggles.
    let words = "Tell me before I run out";
    let lw = ui.c.measure(words, &TextStyle::body(), 400.0).0.ceil();
    let row = Rect::new(r.x, r.y, (lw + 10.0 + SWITCH_W).min((refresh.x - 16.0 - r.x).max(0.0)), r.h);
    if super::settings::switch_row(ui, wid("usage.notices"), row, words, Some(st.settings.usage.notices), st.accent) {
        st.settings.usage.notices = !st.settings.usage.notices;
        st.cmd(Command::SaveSettings);
    }

    let status = if busy {
        CHECKING.to_string()
    } else {
        st.usage.readings.values().map(|r| r.checked_at).max().map(|t| format!("Updated {}", clock_at(st, t))).unwrap_or_default()
    };
    let x = row.right() + 16.0;
    let room = refresh.x - 8.0 - x;
    if !status.is_empty() && room > 40.0 {
        ui.c.text(&status, Rect::new(x, r.y, room, r.h), &TextStyle::footnote().color(palette::TEXT_FAINT).right());
    }
}

// ---------------------------------------------------------------- what shows

/// The service has a card: not hidden, and added by the person, being
/// connected (Oyenza Inference), or found on this computer or by its key.
pub fn shown(st: &AppState, id: ProviderId) -> bool {
    let u = &st.settings.usage;
    let has = |list: &[String]| list.iter().any(|k| k == id.key());
    if has(&u.hidden) {
        return false;
    }
    if has(&u.added) || (id == ProviderId::Oyenza && st.usage.inference != InferenceLink::Off) {
        return true;
    }
    match st.usage.readings.get(&id) {
        Some(r) => !matches!(r.trouble, Some(Trouble::NotFound | Trouble::NeedsKey)),
        // A key just typed, before its first reading.
        None => usage::provider(id).takes_key().is_some_and(|k| st.usage.keys.contains(k.secret)),
    }
}

/// The numbers a card shows: the last good ones, unless the latest refresh
/// says they're gone (the app or the key went, the account was switched
/// off, or this computer was disconnected from Oyenza Inference).
fn numbers(id: ProviderId, r: &Reading) -> Option<&Snapshot> {
    match r.trouble {
        Some(Trouble::NotFound | Trouble::NeedsKey | Trouble::Disabled) => None,
        Some(Trouble::Refused) if id == ProviderId::Oyenza => None,
        _ => r.snapshot.as_ref(),
    }
}

/// Oyenza Inference is connected (a key is kept, or the code was just approved).
fn connected(st: &AppState) -> bool {
    matches!(st.usage.inference, InferenceLink::On { .. }) || st.usage.keys.contains(oyenza::SECRET)
}

/// The service's mark.
fn mark(ui: &mut Ui, id: ProviderId, r: Rect, color: Color) {
    ui.c.icon(crate::icons::service(id), r, color);
}

// ---------------------------------------------------------------- a card's lines

/// One line of a card, top to bottom.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    /// A limit that fills up and resets (as it stands now).
    Limit(Window),
    /// Money spent against a cap: drawn as a limit.
    Capped(Spend),
    /// Money left: "$4.20 left".
    Balance(Money),
    /// Money spent, side by side: "Today $0.12", "7 days $1.03".
    Spends(Vec<Spend>),
    Tokens(u64),
    /// Use per day, oldest first: small bars.
    Days(Vec<DayUse>),
    /// Where the numbers came from and how old they are.
    Note(String),
    /// A trouble while older numbers still show, and what fixes it.
    Warn(String, Option<Act>),
    /// No numbers: why, and the one thing that fixes it.
    Trouble { title: String, line: String, act: Option<Act> },
    /// "Checking…", "Getting a code…".
    Quiet(String),
    /// Oyenza Inference's connect code, waiting for its approval.
    Code { code: String, url: String, expires_at: i64 },
    /// A plan ran out: Oyenza Inference carries on.
    Nudge,
}

/// A card's or a row's button.
#[derive(Clone, Debug, PartialEq)]
struct Act {
    label: String,
    does: Does,
}

#[derive(Clone, Debug, PartialEq)]
enum Does {
    Refresh,
    /// Open this service's key field in the Services list.
    Key,
    /// Connect Oyenza Inference (again).
    Connect,
    CancelConnect,
    Disconnect,
    Url(String),
}

impl Act {
    fn new(label: impl Into<String>, does: Does) -> Option<Act> {
        Some(Act { label: label.into(), does })
    }
}

/// A card, laid out.
struct Card {
    id: ProviderId,
    parts: Vec<Part>,
    h: f32,
}

/// What the card of `id` says, line by line.
fn parts(st: &AppState, id: ProviderId) -> Vec<Part> {
    if id == ProviderId::Oyenza {
        match &st.usage.inference {
            InferenceLink::Starting => return vec![Part::Quiet(oyenza::STARTING.into())],
            InferenceLink::Code { code, url, expires_at } => return vec![Part::Code { code: code.clone(), url: url.clone(), expires_at: *expires_at }],
            InferenceLink::Failed(why) => {
                let (title, line) = oyenza::connect_failure_text(why);
                return vec![Part::Trouble { title: title.into(), line: line.into(), act: Act::new(oyenza::CONNECT_AGAIN, Does::Connect) }];
            }
            InferenceLink::Off | InferenceLink::On { .. } => {}
        }
    }
    let Some(r) = st.usage.readings.get(&id) else { return vec![Part::Quiet(CHECKING.into())] };
    // "Not asked online yet" only means the online answer is on its way.
    let trouble = r.trouble.as_ref().filter(|t| **t != Trouble::NeedsOnline);
    let Some(snap) = numbers(id, r) else {
        return match trouble {
            Some(t) => {
                let w = words(id, t);
                vec![Part::Trouble { title: w.title, line: w.line, act: w.act }]
            }
            None => vec![Part::Quiet(CHECKING.into())],
        };
    };
    let now = st.unix;
    let mut out: Vec<Part> = snap.windows.iter().map(|w| Part::Limit(w.at(now))).collect();
    out.extend(snap.spend.iter().filter(|s| s.cap.as_ref().is_some_and(|c| c.micros > 0)).cloned().map(Part::Capped));
    if let Some(b) = &snap.balance {
        out.push(Part::Balance(b.clone()));
    }
    let open: Vec<Spend> = snap.spend.iter().filter(|s| !s.cap.as_ref().is_some_and(|c| c.micros > 0)).cloned().collect();
    if !open.is_empty() {
        out.push(Part::Spends(open));
    }
    if let Some(t) = snap.tokens_today {
        out.push(Part::Tokens(t));
    }
    if snap.days.len() >= 2 {
        out.push(Part::Days(snap.days.clone()));
    }
    if out.is_empty() {
        out.push(Part::Quiet("No limits to show for this plan".into()));
    }
    let age = (now - snap.as_of).max(0);
    let note = match snap.source {
        Source::Local => Some(format!("From {}'s log, {}", id.name(), ago(age))),
        Source::Online if age > OLD_S || trouble.is_some() => Some(format!("Updated {}", ago(age))),
        Source::Online => None,
    };
    out.extend(note.map(Part::Note));
    if let Some(t) = trouble {
        let w = words(id, t);
        out.push(Part::Warn(w.short, w.act));
    }
    // A plan that ran out points to Oyenza Inference, unless the person hid it.
    let out_now = snap.windows.iter().any(|w| w.at(now).used >= 1.0);
    if out_now && id != ProviderId::Oyenza && !st.settings.usage.hidden.iter().any(|k| k == ProviderId::Oyenza.key()) {
        out.push(Part::Nudge);
    }
    out
}

/// A trouble in plain words: a title and a line for a card without numbers,
/// a short form for one whose older numbers still show, and the one button
/// that fixes it.
#[derive(Clone, Debug, PartialEq)]
struct Words {
    title: String,
    line: String,
    short: String,
    act: Option<Act>,
}

fn words(id: ProviderId, t: &Trouble) -> Words {
    let name = id.name();
    let key = usage::provider(id).takes_key().is_some();
    let w = |title: String, line: String, short: String, act: Option<Act>| Words { title, line, short, act };
    let again = || Act::new("Try again", Does::Refresh);
    if id == ProviderId::Oyenza {
        match t {
            Trouble::NeedsKey | Trouble::NotFound => {
                return w("Not connected".into(), oyenza::CONNECT_LINE.into(), "Not connected".into(), Act::new(oyenza::CONNECT, Does::Connect))
            }
            Trouble::Refused => {
                let (title, line) = oyenza::refused_text(oyenza::Refusal::Key);
                return w(title.into(), line.into(), title.into(), Act::new(oyenza::CONNECT_AGAIN, Does::Connect));
            }
            Trouble::Disabled => {
                let (title, line) = oyenza::refused_text(oyenza::Refusal::Disabled);
                return w(title.into(), line.into(), title.into(), None);
            }
            _ => {}
        }
    }
    match t {
        Trouble::NotFound => w("Not on this computer".into(), format!("Sign in to {name} here and its plan shows up"), "Not on this computer".into(), None),
        Trouble::SignedOut => w("Not signed in".into(), format!("Sign in to {name} on this computer"), format!("Signed out of {name}"), None),
        Trouble::Expired => w("Sign-in expired".into(), format!("Open {name} once to refresh"), format!("Open {name} once to refresh"), None),
        Trouble::NeedsKey => w("Needs a key".into(), format!("Add your {name} key to see your plan"), "Needs a key".into(), Act::new("Add key", Does::Key)),
        Trouble::Refused if key => w("Key not accepted".into(), format!("{name} refused the key. Check it, or make a new one"), "Key not accepted".into(), Act::new("Change key", Does::Key)),
        Trouble::Refused => w("Sign-in not accepted".into(), format!("Sign in to {name} again on this computer"), format!("Sign in to {name} again"), None),
        Trouble::Disabled => {
            let page = usage_page(id);
            let act = Act::new(format!("Open {}", host(page)), Does::Url(page.into()));
            w("Account switched off".into(), format!("{name} switched this account off"), "Account switched off".into(), act)
        }
        Trouble::Busy { retry_after_s } => {
            let when = soon(*retry_after_s);
            w("Asked too often".into(), format!("Trying again {when}"), format!("Asked too often, trying again {when}"), None)
        }
        Trouble::Offline => w(format!("Can't reach {name}"), "The numbers come back once you're online".into(), "Offline".into(), again()),
        Trouble::Unreadable(_) => w("Couldn't read the numbers".into(), format!("{name} answered in a way this version doesn't know"), "Couldn't read the latest numbers".into(), again()),
        Trouble::Denied => w("Keychain said no".into(), "Choose Allow when macOS asks about “security”".into(), "Keychain said no".into(), again()),
        // Never said (it only means "not asked online yet"): kept for completeness.
        Trouble::NeedsOnline => w(CHECKING.into(), String::new(), CHECKING.into(), None),
    }
}

/// Each service's own usage page (where CodexBar's menu points), for an
/// account the service switched off.
fn usage_page(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Codex => "https://chatgpt.com/codex/cloud/settings/analytics#usage",
        ProviderId::Claude => "https://claude.ai/settings/usage",
        ProviderId::OpenCode => "https://opencode.ai/auth",
        ProviderId::Gemini => "https://gemini.google.com",
        ProviderId::Cursor => "https://cursor.com/dashboard?tab=usage",
        ProviderId::Copilot => "https://github.com/settings/copilot",
        ProviderId::Kimi => "https://www.kimi.com/code/console",
        ProviderId::Zai => "https://z.ai/manage-apikey/coding-plan/personal/my-plan",
        ProviderId::MiniMax => "https://platform.minimax.io/user-center/payment/coding-plan?cycle_type=3",
        ProviderId::OpenRouter => "https://openrouter.ai/activity",
        ProviderId::Oyenza => oyenza::BASE,
    }
}

/// "github.com" from "https://www.github.com/settings": the name a link shows.
fn host(url: &str) -> &str {
    let h = url.split_once("://").map_or(url, |(_, h)| h);
    let h = h.split(['/', '?', '#']).next().unwrap_or(h);
    h.strip_prefix("www.").unwrap_or(h)
}

fn part_h(ui: &mut Ui, p: &Part, w: f32) -> f32 {
    match p {
        Part::Limit(_) | Part::Capped(_) => LIMIT_H,
        Part::Balance(_) => BALANCE_H,
        Part::Spends(_) => SPENDS_H,
        Part::Tokens(_) | Part::Quiet(_) => LINE_H,
        Part::Days(_) => DAYS_H,
        Part::Note(_) => NOTE_H,
        Part::Warn(..) => NOTE_H + 4.0,
        Part::Nudge => LINE_H + 4.0,
        Part::Trouble { line, act, .. } => {
            let (_, lh) = ui.c.measure(line, &trouble_line(), w);
            19.0 + lh.ceil() + if act.is_some() { 8.0 + PILL_SMALL } else { 2.0 }
        }
        Part::Code { code, url, .. } => {
            let (_, ah) = ui.c.measure(&oyenza::approve_text(code, url), &trouble_line(), w);
            ah.ceil() + 4.0 + CODE_BOX_H + 8.0 + PILL_SMALL
        }
    }
}

fn trouble_line() -> TextStyle {
    TextStyle::footnote().color(palette::TEXT_DIM).wrap().top()
}

// ---------------------------------------------------------------- the cards

fn cards(ui: &mut Ui, st: &mut AppState, view: Rect, dt: f32, moving: &mut bool) {
    let ids: Vec<ProviderId> = ProviderId::ALL.into_iter().filter(|id| shown(st, *id)).collect();
    if ids.is_empty() {
        empty(ui, st, view);
        return;
    }
    let cols = if view.w >= 3.0 * CARD_MIN_W + 2.0 * GAP + 60.0 {
        3
    } else if view.w >= 2.0 * CARD_MIN_W + GAP {
        2
    } else {
        1
    };
    let card_w = ((view.w - (cols - 1) as f32 * GAP) / cols as f32).floor();
    let inner = card_w - 2.0 * PAD_X;
    let cards: Vec<Card> = ids
        .iter()
        .map(|&id| {
            let parts = parts(st, id);
            let h = PAD_Y + HEAD_H + HEAD_GAP + parts.iter().map(|p| part_h(ui, p, inner)).sum::<f32>() + PAD_Y;
            Card { id, parts, h }
        })
        .collect();
    // Each card goes under the shortest column so far, in order.
    let mut tops = vec![0.0f32; cols];
    let mut at = Vec::with_capacity(cards.len());
    for c in &cards {
        let col = (0..cols).min_by(|a, b| tops[*a].total_cmp(&tops[*b])).unwrap_or(0);
        at.push((col, tops[col]));
        tops[col] += c.h + GAP;
    }
    let content_h = tops.iter().copied().fold(0.0, f32::max) - GAP;
    let reveal = st.ui.usage.reveal.take();
    let target = reveal.and_then(|id| cards.iter().position(|c| c.id == id)).map(|i| (at[i].1, cards[i].h));
    let scroll = scroll_to(ui, &mut st.ui.usage, view, content_h, target, dt, moving);

    let pill_card = super::card::pill_card(st, Rect::new(view.x, view.y - BODY_Y, view.w, HEIGHT));
    let mut acts = Vec::new();
    ui.c.clip(view);
    for (i, c) in cards.iter().enumerate() {
        let (col, y) = at[i];
        let r = Rect::new(view.x + col as f32 * (card_w + GAP), view.y + y - scroll, card_w, c.h);
        if r.bottom() < view.y || r.y > view.bottom() {
            continue;
        }
        draw_card(ui, st, c, r, view, pill_card, &mut acts);
    }
    fades(ui, view, scroll, (content_h - view.h).max(0.0));
    ui.c.unclip();
    for (id, does) in acts {
        act(ui, st, id, does);
    }
}

fn draw_card(ui: &mut Ui, st: &AppState, c: &Card, r: Rect, view: Rect, pill_card: Option<(Rect, f32)>, acts: &mut Vec<(ProviderId, Does)>) {
    super::card::fill_box(ui, pill_card, r, CARD_R, palette::SURFACE);
    let snap = st.usage.readings.get(&c.id).and_then(|rd| numbers(c.id, rd));
    let x = r.x + PAD_X;
    let w = r.w - 2.0 * PAD_X;
    head(ui, st, c.id, snap, Rect::new(x, r.y + PAD_Y, w, HEAD_H));
    let mut y = r.y + PAD_Y + HEAD_H + HEAD_GAP;
    for (i, p) in c.parts.iter().enumerate() {
        let h = part_h(ui, p, w);
        let pr = Rect::new(x, y, w, h);
        // Widget ids: the service's place in the list, then the line's.
        let n = ((ProviderId::ALL.iter().position(|p| *p == c.id).unwrap_or(0) as u64) << 8) | i as u64;
        if let Some(does) = draw_part(ui, st, p, pr, view, n) {
            acts.push((c.id, does));
        }
        y += h;
    }
}

/// Mark, name and plan on the left, the masked account on the right while
/// there's room for it.
fn head(ui: &mut Ui, st: &AppState, id: ProviderId, snap: Option<&Snapshot>, r: Rect) {
    mark(ui, id, Rect::new(r.x, r.center().y - MARK / 2.0, MARK, MARK), palette::TEXT);
    let x = r.x + MARK + 7.0;
    let name_st = TextStyle::body().weight(Weight::Semibold);
    let nw = ui.c.measure(id.name(), &name_st, 1000.0).0.ceil().min((r.right() - x).max(0.0));
    ui.c.text(id.name(), Rect::new(x, r.y, nw + 1.0, r.h), &name_st);
    let mut used = x + nw;
    let dim = TextStyle::footnote().color(palette::TEXT_DIM);
    if let Some(plan) = snap.and_then(|s| s.plan.as_deref()).filter(|p| !p.trim().is_empty()) {
        let pw = ui.c.measure(plan, &dim, 1000.0).0.ceil();
        let px = used + 7.0;
        if px + 30.0 <= r.right() {
            let pw = pw.min(r.right() - px);
            ui.c.text(plan, Rect::new(px, r.y, pw + 1.0, r.h), &dim);
            used = px + pw;
        }
    }
    let account = match (snap.and_then(|s| s.account.as_deref()), &st.usage.inference) {
        (Some(a), _) => Some(a),
        (None, InferenceLink::On { account }) if id == ProviderId::Oyenza => Some(account.as_str()),
        _ => None,
    };
    // Whole or not at all: a cut email reads as broken.
    if let Some(a) = account.filter(|a| !a.trim().is_empty()) {
        let aw = ui.c.measure(a, &dim, 1000.0).0.ceil();
        if aw <= r.right() - used - 12.0 {
            ui.c.text(a, Rect::new(r.right() - aw - 1.0, r.y, aw + 1.0, r.h), &dim.right());
        }
    }
}

/// Draws one line of a card; the button it holds, if one was clicked.
fn draw_part(ui: &mut Ui, st: &AppState, p: &Part, r: Rect, view: Rect, n: u64) -> Option<Does> {
    match p {
        Part::Limit(w) => {
            let when = w.resets_at.map(|at| if ui.hovered(r) && inside(view, r) { (in_words(at - st.unix), None) } else { resets_words(st, at) });
            let right = match w.count {
                Some((used, of)) if of > 0 => format!("{} of {}", group(used), group(of)),
                _ => pct(w.used),
            };
            limit(ui, st, r, &w.label, when, &right, w.used);
            None
        }
        Part::Capped(s) => {
            let cap = s.cap.as_ref().map_or(1, |c| c.micros.max(1));
            let used = (s.money.micros as f64 / cap as f64) as f32;
            let right = format!("{} of {}", s.money.fmt(), s.cap.as_ref().map(Money::fmt).unwrap_or_default());
            limit(ui, st, r, &s.label, None, &right, used);
            None
        }
        Part::Balance(m) => {
            let (value, word) = money_words(m);
            let value_st = TextStyle::headline().weight(Weight::Semibold).tabular();
            let vw = ui.c.measure(&value, &value_st, r.w).0.ceil();
            ui.c.text(&value, Rect::new(r.x, r.y, vw + 1.0, 20.0), &value_st);
            ui.c.text(&word, Rect::new(r.x + vw + 6.0, r.y + 1.0, (r.w - vw - 6.0).max(0.0), 20.0), &TextStyle::footnote().color(palette::TEXT_DIM));
            None
        }
        Part::Spends(list) => {
            let n = list.len().clamp(1, 3);
            let cw = (r.w - (n as f32 - 1.0) * GAP) / n as f32;
            for (i, s) in list.iter().take(n).enumerate() {
                let cx = r.x + i as f32 * (cw + GAP);
                ui.c.text(&s.label, Rect::new(cx, r.y, cw, 14.0), &TextStyle::caption().color(palette::TEXT_DIM));
                ui.c.text(&s.money.fmt(), Rect::new(cx, r.y + 14.0, cw, 16.0), &TextStyle::footnote().weight(Weight::Semibold).tabular());
            }
            None
        }
        Part::Tokens(t) => {
            ui.c.text("Tokens today", Rect::new(r.x, r.y, r.w, 15.0), &TextStyle::footnote().color(palette::TEXT_DIM));
            ui.c.text(&tokens(*t), Rect::new(r.x, r.y, r.w, 15.0), &TextStyle::footnote().weight(Weight::Semibold).tabular().right());
            None
        }
        Part::Days(days) => {
            day_bars(ui, days, r);
            None
        }
        Part::Note(text) => {
            ui.c.text(text, Rect::new(r.x, r.y, r.w, 14.0), &TextStyle::caption().color(palette::TEXT_DIM));
            None
        }
        Part::Quiet(text) => {
            ui.c.text(text, Rect::new(r.x, r.y, r.w, 15.0), &TextStyle::footnote().color(palette::TEXT_DIM));
            None
        }
        Part::Warn(text, act) => {
            let line = Rect::new(r.x, r.y + 2.0, r.w, 15.0);
            let mut end = line.right();
            let mut out = None;
            if let Some(a) = act {
                let (clicked, x) = link(ui, wid_n("usage.warn", n), line, view, &a.label, st.accent, true);
                if clicked {
                    out = Some(a.does.clone());
                }
                end = x - 10.0;
            }
            ui.c.text(text, Rect::new(line.x, line.y, (end - line.x).max(0.0), line.h), &TextStyle::footnote().color(palette::ORANGE));
            out
        }
        Part::Nudge => {
            let line = Rect::new(r.x, r.y + 3.0, r.w, 16.0);
            let does = if connected(st) { Does::Url(oyenza::BASE.into()) } else { Does::Connect };
            let label_st = TextStyle::footnote().weight(Weight::Semibold);
            let lw = ui.c.measure(oyenza::NUDGE, &label_st, r.w).0.ceil().min(line.w - 18.0);
            let hit = Rect::new(line.x - 4.0, line.y - 3.0, lw + 26.0, line.h + 6.0);
            let resp = if inside(view, hit) { ui.button(wid_n("usage.nudge", n), hit) } else { Default::default() };
            let color = if resp.hovered { palette::TEXT } else { OYENZA };
            ui.c.text(oyenza::NUDGE, Rect::new(line.x, line.y, lw + 1.0, line.h), &label_st.color(color));
            ui.c.icon(Icon::Open, Rect::new(line.x + lw + 5.0, line.center().y - 6.0, 12.0, 12.0), color);
            resp.clicked.then_some(does)
        }
        Part::Trouble { title, line, act } => {
            ui.c.text(title, Rect::new(r.x, r.y, r.w, 17.0), &TextStyle::body());
            let (_, lh) = ui.c.measure(line, &trouble_line(), r.w);
            ui.c.text(line, Rect::new(r.x, r.y + 19.0, r.w, lh.ceil() + 1.0), &trouble_line());
            let a = act.as_ref()?;
            let style = match a.does {
                Does::Refresh | Does::Url(_) => PillStyle::Utility,
                Does::Connect => PillStyle::Primary(OYENZA),
                _ => PillStyle::Primary(st.accent),
            };
            let icon = matches!(a.does, Does::Url(_)).then_some(Icon::Open);
            let pw = ui.pill_width(&a.label, icon.is_some(), true).min(r.w);
            let pill = Rect::new(r.x, r.y + 19.0 + lh.ceil() + 8.0, pw, PILL_SMALL);
            pill_in(ui, view, wid_n("usage.fix", n), pill, &a.label, icon, style).then(|| a.does.clone())
        }
        Part::Code { code, url, expires_at } => connect_code(ui, st, r, view, code, url, *expires_at),
    }
}

/// A limit's words (its name, when it resets) and how much is used, over
/// its bar. `when`: the long form, and a shorter one for a narrow card.
fn limit(ui: &mut Ui, st: &AppState, r: Rect, label: &str, when: Option<(String, Option<String>)>, right: &str, used: f32) {
    let near = (st.settings.usage.near_pct as f32 / 100.0).clamp(0.05, 1.0);
    let fill = if used >= 1.0 {
        palette::RED
    } else if used >= near {
        palette::ORANGE
    } else {
        Color::white(0.86)
    };
    let right_st = TextStyle::footnote().weight(Weight::Semibold).tabular().right().color(if used >= 1.0 { palette::RED } else { palette::TEXT });
    let rw = ui.c.measure(right, &right_st, r.w).0.ceil();
    ui.c.text(right, Rect::new(r.x, r.y, r.w, WORDS_H), &right_st);
    let room = (r.w - rw - 10.0).max(0.0);
    let label_st = TextStyle::footnote();
    let lw = ui.c.measure(label, &label_st, 1000.0).0.ceil().min(room);
    ui.c.text(label, Rect::new(r.x, r.y, lw + 1.0, WORDS_H), &label_st);
    if let Some((long, short)) = when {
        let dim = TextStyle::footnote().color(palette::TEXT_DIM);
        let x = r.x + lw + 8.0;
        let avail = r.x + room - x;
        let text = match short {
            Some(s) if ui.c.measure(&long, &dim, 1000.0).0 > avail => s,
            _ => long,
        };
        if avail > 24.0 {
            ui.c.text(&text, Rect::new(x, r.y, avail, WORDS_H), &dim);
        }
    }
    let bar = Rect::new(r.x, r.y + BAR_Y, r.w, BAR_H);
    ui.c.fill_rrect(bar, BAR_H / 2.0, palette::TRACK);
    let k = used.clamp(0.0, 1.0);
    if k > 0.0 {
        ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * k).max(BAR_H), BAR_H), BAR_H / 2.0, fill);
    }
}

/// "14 days" and a bar per day (money, else requests), today's the brightest.
fn day_bars(ui: &mut Ui, days: &[DayUse], r: Rect) {
    let label = format!("{} days", days.len());
    let label_st = TextStyle::caption().color(palette::TEXT_DIM);
    let lw = ui.c.measure(&label, &label_st, 200.0).0.ceil();
    let bars = Rect::new(r.x + lw + 10.0, r.y + 2.0, (r.w - lw - 10.0).max(0.0), r.h - 8.0);
    ui.c.text(&label, Rect::new(r.x, bars.y, lw + 1.0, bars.h), &label_st);
    let vals: Vec<f64> = days.iter().map(|d| d.money.as_ref().map(|m| m.micros.max(0) as f64).or(d.requests.map(|n| n as f64)).unwrap_or(0.0)).collect();
    let max = vals.iter().copied().fold(0.0, f64::max);
    let n = vals.len().max(1) as f32;
    let gap = 3.0;
    let bw = ((bars.w - gap * (n - 1.0)) / n).max(1.0);
    for (i, v) in vals.iter().enumerate() {
        let h = if max > 0.0 && *v > 0.0 { (bars.h * (*v / max) as f32).max(3.0) } else { 2.0 };
        let color = if i + 1 == vals.len() {
            Color::white(0.86)
        } else if *v > 0.0 {
            Color::white(0.38)
        } else {
            Color::white(0.16)
        };
        let x = bars.x + i as f32 * (bw + gap);
        ui.c.fill_rrect(Rect::new(x, bars.bottom() - h, bw, h), (bw / 2.0).min(1.5), color);
    }
}

/// Oyenza Inference's code on its card: where to approve it, the code, the
/// approve page, Cancel, and until when it works.
fn connect_code(ui: &mut Ui, st: &AppState, r: Rect, view: Rect, code: &str, url: &str, expires_at: i64) -> Option<Does> {
    // "Approve KPJX-TMNQ on inference.oyenza.com", on two lines on a narrow card.
    let approve = oyenza::approve_text(code, url);
    let (_, ah) = ui.c.measure(&approve, &trouble_line(), r.w);
    ui.c.text(&approve, Rect::new(r.x, r.y, r.w, ah.ceil() + 1.0), &trouble_line());
    let code_r = Rect::new(r.x, r.y + ah.ceil() + 4.0, r.w, CODE_BOX_H);
    ui.c.fill_rrect(code_r, 10.0, Color::white(0.07));
    ui.c.text(code, code_r, &TextStyle::display().tabular().center());
    let y = code_r.bottom() + 8.0;
    let cancel_w = ui.pill_width("Cancel", false, true);
    let mut open_label = format!("Open {}", host(url));
    if ui.pill_width(&open_label, true, true) + PILL_GAP + cancel_w > r.w {
        open_label = "Open".into();
    }
    let open_w = ui.pill_width(&open_label, true, true);
    let open = Rect::new(r.x, y, open_w, PILL_SMALL);
    let mut out = None;
    if pill_in(ui, view, wid("usage.code-open"), open, &open_label, Some(Icon::Open), PillStyle::Primary(OYENZA)) {
        out = Some(Does::Url(url.to_string()));
    }
    let cancel = Rect::new(open.right() + PILL_GAP, y, cancel_w, PILL_SMALL);
    if pill_in(ui, view, wid("usage.code-cancel"), cancel, "Cancel", None, PillStyle::Utility) {
        out = Some(Does::CancelConnect);
    }
    let until = format!("Works until {}", clock_at(st, expires_at));
    let x = cancel.right() + 10.0;
    let dim = TextStyle::footnote().color(palette::TEXT_DIM).right();
    if ui.c.measure(&until, &dim, 400.0).0 <= r.right() - x {
        ui.c.text(&until, Rect::new(x, y, r.right() - x, PILL_SMALL), &dim);
    }
    out
}

// ---------------------------------------------------------------- nothing found

/// No card to show: what the tab looks for, Add a key, and Oyenza Inference.
fn empty(ui: &mut Ui, st: &mut AppState, r: Rect) {
    const BADGE: f32 = 52.0;
    let looking = st.usage.readings.is_empty();
    let title = if looking { "Looking for your AI tools…" } else { "No AI plans found on this computer" };
    // Looked for here: every service with something to read on the computer
    // (OpenCode Go and Kimi Code too, though they also take a key); a key:
    // every service that takes one. The worker's own rule decides which is which.
    let dirs = usage::Dirs::sample(if cfg!(target_os = "macos") { usage::Os::Mac } else { usage::Os::Windows });
    let ids = ProviderId::ALL.into_iter().filter(|id| *id != ProviderId::Oyenza);
    let found: Vec<ProviderId> = ids.clone().filter(|id| !usage::pace::typed_key_only(&usage::provider(*id).wants(&dirs), false)).collect();
    let keyed: Vec<ProviderId> = ids.filter(|id| usage::provider(*id).takes_key().is_some()).collect();
    let mut body = format!("Studio looks for {} on this computer.", names(&found, "and"));
    if !keyed.is_empty() {
        body.push_str(&format!(" Add a key for {}.", names(&keyed, "or")));
    }
    let x = r.x + BADGE + 18.0;
    let w = (r.w - BADGE - 18.0).clamp(0.0, 440.0);
    let title_st = TextStyle::headline().wrap().top();
    let body_st = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    let (_, th) = ui.c.measure(title, &title_st, w);
    let (_, bh) = ui.c.measure(&body, &body_st, w);
    let block = th + 4.0 + bh + if looking { 0.0 } else { 12.0 + PILL_SMALL };
    let mut y = r.y + ((r.h - block) / 2.0).max(0.0);
    let c = Point::new(r.x + BADGE / 2.0, r.y + r.h / 2.0);
    ui.c.fill_circle(c, BADGE / 2.0, st.accent.with_alpha(0.18));
    ui.c.icon(PageKind::Ai.icon(), Rect::from_center(c, 24.0, 24.0), st.accent);
    ui.c.text(title, Rect::new(x, y, w, th + 1.0), &title_st);
    y += th + 4.0;
    ui.c.text(&body, Rect::new(x, y, w, bh + 1.0), &body_st);
    if looking {
        return;
    }
    y += bh + 12.0;
    let add_w = ui.pill_width("Add a key", true, true);
    let add = Rect::new(x, y, add_w, PILL_SMALL);
    if ui.pill(wid("usage.empty-add"), add, "Add a key", Some(Icon::Plus), PillStyle::Primary(st.accent)).clicked {
        let first = keyed.first().copied();
        st.ui.usage.switch(true);
        if let Some(id) = first {
            open_key(&mut st.ui.usage, id);
        }
    }
    let label = "Connect Oyenza Inference";
    let cw = ui.pill_width(label, false, true);
    let connect = Rect::new(add.right() + PILL_GAP, y, cw, PILL_SMALL);
    if connect.right() <= r.right() + 0.5 && ui.pill(wid("usage.empty-connect"), connect, label, None, PillStyle::Utility).clicked {
        act(ui, st, ProviderId::Oyenza, Does::Connect);
    }
}

/// "Codex, Claude Code and Gemini CLI" (or "... or ...").
fn names(ids: &[ProviderId], last_word: &str) -> String {
    match ids {
        [] => String::new(),
        [one] => one.name().to_string(),
        [rest @ .., last] => format!("{} {last_word} {}", rest.iter().map(|p| p.name()).collect::<Vec<_>>().join(", "), last.name()),
    }
}

// ---------------------------------------------------------------- the Services list

/// Every service, in order: shown or hidden, its key (typed into a masked
/// field that opens under its row), and Oyenza Inference's connection.
fn list(ui: &mut Ui, st: &mut AppState, view: Rect, dt: f32, moving: &mut bool) {
    let intro = "Each sign-in is read on this computer and goes only to its own service. Oyenza never sees your other plans.";
    let intro_st = TextStyle::caption().color(palette::TEXT_DIM).wrap().top();
    let intro_h = ui.c.measure(intro, &intro_st, view.w).1.ceil();
    let editing = st.ui.usage.editing.filter(|id| usage::provider(*id).takes_key().is_some());
    let mut rows = Vec::with_capacity(ProviderId::ALL.len());
    let mut y = intro_h + INTRO_GAP;
    for id in ProviderId::ALL {
        rows.push((id, y));
        y += ROW_H + if editing == Some(id) { EDIT_H } else { 0.0 };
    }
    let content_h = y;
    let reveal = st.ui.usage.reveal.take();
    let target = reveal.and_then(|id| rows.iter().find(|r| r.0 == id)).map(|&(id, y)| (y, ROW_H + if editing == Some(id) { EDIT_H } else { 0.0 }));
    let scroll = scroll_to(ui, &mut st.ui.usage, view, content_h, target, dt, moving);

    let wide = Rect::new(view.x - 8.0, view.y, view.w + 16.0, view.h);
    ui.c.clip(wide);
    let intro_r = Rect::new(view.x, view.y - scroll, view.w, intro_h + 1.0);
    if intro_r.bottom() > view.y {
        ui.c.text(intro, intro_r, &intro_st);
    }
    let mut acts = Vec::new();
    for (i, &(id, y)) in rows.iter().enumerate() {
        let r = Rect::new(view.x, view.y + y - scroll, view.w, ROW_H);
        let bottom = r.bottom() + if editing == Some(id) { EDIT_H } else { 0.0 };
        if bottom < view.y || r.y > view.bottom() {
            continue;
        }
        if i > 0 {
            ui.c.line(Point::new(r.x, r.y + 0.5), Point::new(r.right(), r.y + 0.5), Color::white(0.07), 1.0);
        }
        acts.extend(row(ui, st, id, r, view).into_iter().map(|d| (id, d)));
        if editing == Some(id) {
            if let Some(ask) = usage::provider(id).takes_key() {
                key_editor(ui, st, id, ask, Rect::new(r.x, r.bottom(), r.w, EDIT_H), view);
            }
        }
    }
    fades(ui, wide, scroll, (content_h - view.h).max(0.0));
    ui.c.unclip();
    for (id, does) in acts {
        act(ui, st, id, does);
    }
}

/// One service: mark, name, what's known of it, its buttons and its switch.
/// Only the switch shows or hides it (the row holds other buttons).
fn row(ui: &mut Ui, st: &mut AppState, id: ProviderId, r: Rect, view: Rect) -> Option<Does> {
    let idx = ProviderId::ALL.iter().position(|p| *p == id).unwrap_or(0) as u64;
    let on = shown(st, id);
    let cy = r.center().y;
    mark(ui, id, Rect::new(r.x, cy - 9.0, 18.0, 18.0), if on { palette::TEXT } else { palette::TEXT_DIM });
    let name_r = Rect::new(r.x + NAME_X, r.y, NAME_W - 8.0, r.h);
    ui.c.text(id.name(), name_r, &TextStyle::body().color(if on { palette::TEXT } else { palette::TEXT_DIM }));

    // The switch, then the buttons right to left.
    let sw = Rect::new(r.right() - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    let hit = sw.inset(-8.0, -5.0);
    let resp = if inside(view, hit) { ui.button(wid_n("usage.switch", idx), hit) } else { Default::default() };
    super::settings::draw_switch(ui.c, sw, on, st.accent);
    if resp.clicked {
        st.cmd(Command::Usage(if on { UsageCmd::Hide(id) } else { UsageCmd::Show(id) }));
    }
    let mut right = sw.x - 12.0;
    let mut out = None;
    for (j, (label, icon, style, does)) in row_buttons(st, id).into_iter().enumerate().rev() {
        let w = ui.pill_width(&label, icon.is_some(), true);
        let pill = Rect::new(right - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL);
        if pill.x < name_r.right() {
            break;
        }
        if pill_in(ui, view, wid_n("usage.row-act", (idx << 4) | j as u64), pill, &label, icon, style) {
            out = Some(does);
        }
        right = pill.x - PILL_GAP;
    }

    // Between them: the code while one waits, else what's known.
    let sx = r.x + NAME_X + NAME_W;
    let status_r = Rect::new(sx, r.y, (right - 4.0 - sx).max(0.0), r.h);
    match &st.usage.inference {
        InferenceLink::Code { code, expires_at, .. } if id == ProviderId::Oyenza => {
            let code_st = TextStyle::headline().weight(Weight::Semibold).tabular();
            let cw = ui.c.measure(code, &code_st, status_r.w).0.ceil().min(status_r.w);
            ui.c.text(code, Rect::new(status_r.x, r.y, cw + 1.0, r.h), &code_st);
            let until = format!("until {}", clock_at(st, *expires_at));
            let ux = status_r.x + cw + 8.0;
            ui.c.text(&until, Rect::new(ux, r.y, (status_r.right() - ux).max(0.0), r.h), &TextStyle::footnote().color(palette::TEXT_DIM));
        }
        _ => {
            let (text, color) = status(st, id);
            ui.c.text(&text, status_r, &TextStyle::footnote().color(color));
        }
    }
    out
}

/// A row's buttons, left to right: a key's (Add key, Change key, Cancel
/// while its field is open; none for a service found without one), or
/// Oyenza Inference's connection.
fn row_buttons(st: &AppState, id: ProviderId) -> Vec<(String, Option<Icon>, PillStyle, Does)> {
    let b = |label: &str, icon: Option<Icon>, style: PillStyle, does: Does| (label.to_string(), icon, style, does);
    if id == ProviderId::Oyenza {
        return match &st.usage.inference {
            InferenceLink::Starting => vec![b("Cancel", None, PillStyle::Utility, Does::CancelConnect)],
            InferenceLink::Code { url, .. } => vec![b("Open", Some(Icon::Open), PillStyle::Primary(OYENZA), Does::Url(url.clone())), b("Cancel", None, PillStyle::Utility, Does::CancelConnect)],
            InferenceLink::Failed(_) => vec![b(oyenza::CONNECT_AGAIN, None, PillStyle::Primary(OYENZA), Does::Connect)],
            _ if connected(st) => vec![b(oyenza::DISCONNECT, None, PillStyle::Utility, Does::Disconnect)],
            _ => vec![b(oyenza::CONNECT, None, PillStyle::Primary(OYENZA), Does::Connect)],
        };
    }
    let Some(ask) = usage::provider(id).takes_key() else { return Vec::new() };
    let found = st.usage.readings.get(&id).is_some_and(|r| numbers(id, r).is_some());
    if st.ui.usage.editing == Some(id) {
        vec![b("Cancel", None, PillStyle::Utility, Does::Key)]
    } else if st.usage.keys.contains(ask.secret) {
        vec![b("Change key", None, PillStyle::Utility, Does::Key)]
    } else if found {
        Vec::new()
    } else {
        vec![b("Add key", Some(Icon::Plus), PillStyle::Utility, Does::Key)]
    }
}

/// What the Services list says of a service beside its name.
fn status(st: &AppState, id: ProviderId) -> (String, Color) {
    let (dim, warn) = (palette::TEXT_DIM, palette::ORANGE);
    if id == ProviderId::Oyenza {
        return match &st.usage.inference {
            InferenceLink::Starting => (oyenza::STARTING.into(), dim),
            InferenceLink::Code { code, .. } => (code.clone(), palette::TEXT),
            InferenceLink::Failed(why) => (oyenza::connect_failure_text(why).0.into(), warn),
            InferenceLink::On { account } => (oyenza::connected_text(account), dim),
            InferenceLink::Off => match st.usage.readings.get(&id).and_then(|r| r.trouble.clone()) {
                Some(t @ (Trouble::Refused | Trouble::Disabled)) => (words(id, &t).short, warn),
                _ if connected(st) => ("Connected".into(), dim),
                _ => ("Not connected".into(), dim),
            },
        };
    }
    let kept = usage::provider(id).takes_key().is_some_and(|k| st.usage.keys.contains(k.secret));
    match st.usage.readings.get(&id) {
        Some(r) => match (numbers(id, r), r.trouble.as_ref().filter(|t| **t != Trouble::NeedsOnline)) {
            (Some(s), None) => {
                let found = [s.plan.as_deref(), s.account.as_deref()].into_iter().flatten().filter(|t| !t.trim().is_empty()).collect::<Vec<_>>().join(" · ");
                (if found.is_empty() { "Found".into() } else { found }, dim)
            }
            (_, Some(Trouble::NeedsKey)) if !kept => ("No key yet".into(), dim),
            (_, Some(Trouble::NotFound)) => ("Not on this computer".into(), dim),
            (_, Some(t)) => (words(id, t).short, warn),
            (None, None) => (CHECKING.into(), dim),
        },
        None if kept => ("Key added".into(), dim),
        None if usage::provider(id).takes_key().is_some() => ("No key yet".into(), dim),
        None => (if st.usage.refreshing { CHECKING } else { "Not checked yet" }.into(), dim),
    }
}

/// The key field under a row: masked, Save, Remove (while a key is kept),
/// and where the service makes one.
fn key_editor(ui: &mut Ui, st: &mut AppState, id: ProviderId, ask: KeyAsk, r: Rect, view: Rect) {
    let x = r.x + NAME_X;
    let kept = st.usage.keys.contains(ask.secret);
    let save_w = ui.pill_width("Save", false, true);
    let remove_w = if kept { ui.pill_width("Remove", false, true) + PILL_GAP } else { 0.0 };
    let field = Rect::new(x, r.y + 2.0, (r.right() - x - save_w - PILL_GAP - remove_w).max(60.0), FIELD_H);
    let fid = wid("usage.key-field");
    let accent = st.accent;
    let s = &mut st.ui.usage;
    // The keyboard goes to the field once it's wholly in view (it may still be gliding in).
    let (submitted, cancelled) = if inside(view, field) {
        let focus_now = std::mem::take(&mut s.focus);
        key_field(ui, fid, field, &mut s.key, ask.hint, accent, focus_now)
    } else {
        (false, false)
    };
    let empty = s.key.trim().is_empty();
    let py = field.center().y - PILL_SMALL / 2.0;
    let save = Rect::new(field.right() + PILL_GAP, py, save_w, PILL_SMALL);
    let style = if empty { PillStyle::Utility } else { PillStyle::Primary(accent) };
    let mut saved = pill_in(ui, view, wid("usage.key-save"), save, "Save", None, style) && !empty;
    saved |= submitted && !empty;
    let remove = Rect::new(save.right() + PILL_GAP, py, remove_w - PILL_GAP, PILL_SMALL);
    let removed = kept && pill_in(ui, view, wid("usage.key-remove"), remove, "Remove", None, PillStyle::Utility);

    // Where to make one: the service's own page.
    let make = format!("Make one at {}", host(ask.url));
    let lr = Rect::new(x, field.bottom() + 6.0, r.right() - x, 16.0);
    if link(ui, wid("usage.key-link"), lr, view, &make, accent, false).0 {
        st.cmd(Command::OpenUrl(ask.url.to_string()));
    }

    if saved {
        let key: String = std::mem::take(&mut st.ui.usage.key).trim().to_string();
        st.cmd(Command::Usage(UsageCmd::SetKey { secret: ask.secret, key }));
        if !shown(st, id) {
            st.cmd(Command::Usage(UsageCmd::Show(id)));
        }
        st.ui.usage.close_key();
        ui.blur();
    } else if removed {
        st.cmd(Command::Usage(UsageCmd::ClearKey(ask.secret)));
        st.ui.usage.close_key();
        ui.blur();
    } else if cancelled {
        st.ui.usage.close_key();
    }
}

/// A one-line field that shows what's typed as dots, for a key: it takes
/// no spaces or control characters, and its caret stays at the end (a key
/// is pasted or typed, never edited in the middle). Returns (Enter, Esc).
fn key_field(ui: &mut Ui, id: WidgetId, r: Rect, buf: &mut String, hint: &str, accent: Color, focus_now: bool) -> (bool, bool) {
    let resp = ui.interact(id, r, CursorKind::Text);
    let keys = ui.key_focus(id, resp.clicked || focus_now);
    let (mut submitted, mut cancelled) = (false, false);
    for k in keys {
        match k {
            KeyInput::Char(c) if !c.is_control() && !c.is_whitespace() && buf.chars().count() < KEY_MAX => buf.push(c),
            KeyInput::Backspace | KeyInput::Delete => {
                buf.pop();
            }
            KeyInput::Enter => submitted = true,
            KeyInput::Escape => cancelled = true,
            _ => {}
        }
    }
    let focused = ui.input.focused == Some(id);
    let bg = if focused {
        Color::white(0.13)
    } else if resp.hovered {
        Color::white(0.10)
    } else {
        palette::SURFACE
    };
    ui.c.fill_rrect(r, 9.0, bg);
    if focused {
        ui.c.stroke_rrect(r, 9.0, accent, 1.5);
    }
    let inner = r.inset(10.0, 0.0);
    let st = TextStyle::body();
    let mut caret_x = inner.x;
    if buf.is_empty() {
        ui.c.text(hint, inner, &st.color(palette::TEXT_FAINT));
    } else {
        // As many dots as there are characters, up to what the field holds.
        let dot = ui.c.measure("•", &st, 100.0).0.max(1.0);
        let n = buf.chars().count().min((inner.w / dot).floor().max(1.0) as usize);
        let dots = "•".repeat(n);
        ui.c.text(&dots, inner, &st);
        caret_x = (inner.x + ui.c.measure(&dots, &st, 10_000.0).0 + 1.0).min(inner.right());
    }
    if focused {
        let cy = r.center().y;
        ui.c.line(Point::new(caret_x, cy - 8.0), Point::new(caret_x, cy + 8.0), Color::white(0.9), 1.5);
    }
    (submitted, cancelled)
}

/// Opens `id`'s key field in the Services list, taking the keyboard.
fn open_key(s: &mut UsageUi, id: ProviderId) {
    s.editing = Some(id);
    s.key.clear();
    s.focus = !s.still;
    s.reveal = Some(id);
}

/// What a card's or a row's button asked for.
fn act(ui: &mut Ui, st: &mut AppState, id: ProviderId, does: Does) {
    match does {
        Does::Refresh => st.cmd(Command::Usage(UsageCmd::Refresh)),
        Does::Key => {
            let s = &mut st.ui.usage;
            if s.services && s.editing == Some(id) {
                s.close_key();
                ui.blur();
            } else {
                if !s.services {
                    s.switch(true);
                }
                open_key(s, id);
            }
        }
        Does::Connect => {
            // Its card shows the code: bring it into view, shown even if it was hidden.
            st.cmd(Command::Usage(UsageCmd::Connect));
            if st.settings.usage.hidden.iter().any(|k| k == ProviderId::Oyenza.key()) {
                st.cmd(Command::Usage(UsageCmd::Show(ProviderId::Oyenza)));
            }
            if matches!(st.usage.inference, InferenceLink::Off | InferenceLink::Failed(_)) {
                st.usage.inference = InferenceLink::Starting;
            }
            if !st.ui.usage.services {
                st.ui.usage.reveal = Some(ProviderId::Oyenza);
            }
        }
        Does::CancelConnect => {
            st.cmd(Command::Usage(UsageCmd::CancelConnect));
            st.usage.inference = InferenceLink::Off;
        }
        Does::Disconnect => st.cmd(Command::Usage(UsageCmd::Disconnect)),
        Does::Url(u) => st.cmd(Command::OpenUrl(u)),
    }
}

// ---------------------------------------------------------------- widgets

/// A pill that takes clicks only while it's wholly in `view` (the scrolled
/// list); cut by an edge, it keeps its look but can't be clicked, so it
/// never takes a click meant for something above or below the list.
fn pill_in(ui: &mut Ui, view: Rect, id: WidgetId, r: Rect, label: &str, icon: Option<Icon>, style: PillStyle) -> bool {
    if inside(view, r) {
        return ui.pill(id, r, label, icon, style).clicked;
    }
    let (mut input, mut hits) = (Input::default(), Vec::new());
    let now = ui.now;
    let mut ghost = Ui::new(&mut *ui.c, &mut input, &mut hits, now);
    ghost.pill(id, r, label, icon, style);
    false
}

/// A short link at the left or right end of `line`, in `color` (brighter
/// under the pointer), taking clicks only while it's in `view`. Returns
/// whether it was clicked, and where its text starts.
fn link(ui: &mut Ui, id: WidgetId, line: Rect, view: Rect, label: &str, color: Color, at_right: bool) -> (bool, f32) {
    let st = TextStyle::footnote().weight(Weight::Semibold);
    let w = ui.c.measure(label, &st, line.w).0.ceil().min(line.w);
    let r = Rect::new(if at_right { line.right() - w } else { line.x }, line.y, w, line.h);
    let hit = r.inset(-5.0, -4.0);
    let resp = if inside(view, hit) { ui.button(id, hit) } else { Default::default() };
    ui.c.text(label, Rect::new(r.x, r.y, r.w + 1.0, r.h), &st.color(if resp.hovered { palette::TEXT } else { color }));
    (resp.clicked, r.x)
}

/// `r` lies wholly in `view` (half a DIP of slack).
fn inside(view: Rect, r: Rect) -> bool {
    r.y >= view.y - 0.5 && r.bottom() <= view.bottom() + 0.5
}

/// The wheel and the glide of a list `content_h` tall in `view`, brought
/// to show `target` (top, height) when asked; returns the scroll to draw at.
/// The wheel is only taken when there's something to scroll: otherwise the
/// app uses it to switch pages.
fn scroll_to(ui: &mut Ui, s: &mut UsageUi, view: Rect, content_h: f32, target: Option<(f32, f32)>, dt: f32, moving: &mut bool) -> f32 {
    let max = (content_h - view.h).max(0.0);
    if max > 0.0 {
        let notches = ui.wheel_in(view);
        s.scroll -= notches * WHEEL_STEP;
    }
    if let Some((top, h)) = target {
        if top < s.scroll {
            s.scroll = top;
        } else if top + h > s.scroll + view.h {
            s.scroll = top + h - view.h;
        }
    }
    s.scroll = s.scroll.clamp(0.0, max);
    s.shown_scroll = if s.still { s.scroll } else { glide(s.shown_scroll.clamp(0.0, max), s.scroll, dt, 0.25, moving) };
    s.shown_scroll
}

/// `pos` moved toward `target` for `dt` seconds (as Stocks' glides);
/// `moving` is set while it isn't there yet (within `eps`).
fn glide(pos: f32, target: f32, dt: f32, eps: f32, moving: &mut bool) -> f32 {
    let p = pos + (target - pos) * (1.0 - (-dt * GLIDE).exp());
    if (target - p).abs() <= eps {
        target
    } else {
        *moving = true;
        p
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The island's ground over the list's cut edges (none on Glass, which has
/// no solid ground to fade into).
fn fades(ui: &mut Ui, view: Rect, scroll: f32, max: f32) {
    if ui.c.look().material == poni_core::look::Material::Glass {
        return;
    }
    const STEPS: usize = 8;
    for (top, show) in [(true, scroll > 0.5), (false, scroll < max - 0.5)] {
        if !show {
            continue;
        }
        let step = FADE_H / STEPS as f32;
        for k in 0..STEPS {
            // k = 0 is the strip at the edge.
            let a = 0.92 * (1.0 - (k as f32 + 0.5) / STEPS as f32).powf(1.4);
            let y = if top { view.y + k as f32 * step } else { view.bottom() - (k as f32 + 1.0) * step };
            ui.c.fill_rect(Rect::new(view.x, y, view.w, step), palette::ISLAND.with_alpha(a));
        }
    }
}

// ---------------------------------------------------------------- words

/// "62%": whole percent used (a hair under a whole number counts as it).
fn pct(used: f32) -> String {
    format!("{}%", (used.clamp(0.0, 10.0) * 100.0 + 0.01) as u32)
}

/// "1,250".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1.2M", "820K", "950".
fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}K", (n + 500) / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// A balance as the card says it: "$4.20" and "left"; a unit that isn't a
/// currency ("credits") goes into the word: "1,250" and "credits left".
fn money_words(m: &Money) -> (String, String) {
    let iso = m.currency.len() == 3 && m.currency.bytes().all(|b| b.is_ascii_uppercase());
    let word = if m.micros < 0 { "balance" } else { "left" };
    if iso {
        return (m.fmt(), word.into());
    }
    let v = m.micros as f64 / 1e6;
    let n = if v.abs() >= 100.0 { group(v.abs().round() as u64) } else { format!("{:.2}", v.abs()) };
    (format!("{}{n}", if v < 0.0 { "-" } else { "" }), format!("{} {word}", m.currency.trim().to_lowercase()))
}

/// Local time minus UTC, in seconds (to the quarter hour, as the app's
/// other clocks read it).
fn offset_s(st: &AppState) -> i64 {
    poni_core::oyenza::offset_minutes(&st.local, st.unix) as i64 * 60
}

/// Unix seconds on this computer's clock.
fn local_of(t: i64) -> LocalDateTime {
    let (year, month, day) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    LocalDateTime { year, month, day, hour: (s / 3600) as u8, minute: (s / 60 % 60) as u8, second: (s % 60) as u8 }
}

/// "14:20" (or "2:20 PM"), to the nearest minute.
fn clock_at(st: &AppState, unix: i64) -> String {
    fmt_clock(&local_of((unix + 30).div_euclid(60) * 60 + offset_s(st)), st.settings.use_24h)
}

/// When a limit resets, on this computer's clock, as the heads-up notices
/// say it: "resets 14:20" today, "resets Thu 09:00" this week, "resets Tue
/// 3 Nov" further on; with a shorter form for a narrow card ("resets Thu",
/// "resets 3 Nov").
fn resets_words(st: &AppState, at: i64) -> (String, Option<String>) {
    let off = offset_s(st);
    let t = (at + 30).div_euclid(60) * 60 + off;
    let local = local_of(t);
    let days = t.div_euclid(86_400) - (st.unix + off).div_euclid(86_400);
    let clock = fmt_clock(&local, st.settings.use_24h);
    match days {
        d if d <= 0 => (format!("resets {clock}"), None),
        1..=6 => {
            let wd = WEEKDAY_SHORT[local.weekday() as usize % 7];
            (format!("resets {wd} {clock}"), Some(format!("resets {wd}")))
        }
        _ => (format!("resets {}", fmt_date_short(&local)), Some(format!("resets {} {}", local.day, MONTH_SHORT[(local.month.clamp(1, 12) - 1) as usize]))),
    }
}

/// "in 40 min", "in 2 h 10 min", "in 3 days".
fn in_words(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    match m {
        0 | 1 => "in a minute".into(),
        2..=59 => format!("in {m} min"),
        60..=1439 => match (m / 60, m % 60) {
            (h, 0) => format!("in {h} h"),
            (h, mm) => format!("in {h} h {mm} min"),
        },
        _ => match (m + 720) / 1440 {
            1 => "in 1 day".into(),
            d => format!("in {d} days"),
        },
    }
}

/// "just now", "5 min ago", "2 h ago", "yesterday", "3 days ago".
fn ago(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0 => "just now".into(),
        1..=59 => format!("{m} min ago"),
        60..=1439 => format!("{} h ago", m / 60),
        1440..=2879 => "yesterday".into(),
        _ => format!("{} days ago", m / 1440),
    }
}

/// "in 5 min", or "in a few minutes" without a Retry-After.
fn soon(retry_after_s: Option<u64>) -> String {
    match retry_after_s {
        Some(s) if s <= 90 => "in a minute".into(),
        Some(s) if s < 3600 => format!("in {} min", s.div_ceil(60)),
        Some(_) => "later".into(),
        None => "in a few minutes".into(),
    }
}

// ---------------------------------------------------------------- pictures

/// What a still picture of the AI tab shows (the self-test's, the Mac
/// renders'). All of it made up, on the picture's own clock.
// The Mac renders (`mac/render.rs`) take these once the Mac lane wires them.
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// Codex, Claude Code (past the notice line on its 5-hour limit),
    /// OpenCode Go and Oyenza Inference connected; notices on.
    Plans,
    /// A trouble mix: Codex's sign-in expired (numbers from its log), Claude
    /// Code used up (pointing to Oyenza Inference), Gemini CLI offline,
    /// GitHub Copilot's account switched off, an OpenRouter key refused.
    Troubles,
    /// Nothing found: what the tab looks for, Add a key.
    Empty,
    /// The first opening, before anything is read.
    Looking,
    /// Connecting Oyenza Inference: the code on its card, beside Codex.
    Connect,
    /// Oyenza Inference connected: its balance, spend and last 14 days,
    /// beside Codex.
    Inference,
    /// The Services list from its top: the privacy line, each service
    /// shown or hidden.
    Services,
    /// The Services list at OpenRouter's key, being typed (as dots).
    Key,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl Picture {
    pub const ALL: [Picture; 8] = [Picture::Plans, Picture::Troubles, Picture::Empty, Picture::Looking, Picture::Connect, Picture::Inference, Picture::Services, Picture::Key];

    /// The picture's file name: "page-ai", "page-ai-troubles", ...
    pub fn name(self) -> &'static str {
        match self {
            Picture::Plans => "page-ai",
            Picture::Troubles => "page-ai-troubles",
            Picture::Empty => "page-ai-empty",
            Picture::Looking => "page-ai-looking",
            Picture::Connect => "page-ai-connect",
            Picture::Inference => "page-ai-inference",
            Picture::Services => "page-ai-services",
            Picture::Key => "page-ai-key",
        }
    }
}

/// The tab as a still picture shows it: made-up readings on the picture's
/// clock (`st.unix`, `st.local`), every glide at its end. Sets `st.usage`,
/// `settings.usage` and `st.ui.usage` whole, so a picture never carries
/// another's state; the caller puts the island on the AI tab.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn picture(st: &mut AppState, shows: Picture, now: f64) {
    use studio_usage::WindowKind::{Monthly, Session, Weekly};
    use ProviderId::*;
    const H: i64 = 3600;
    const DAY: i64 = 86_400;
    // A whole ten minutes on the clock, so resets read as round times.
    let t = st.unix - st.local.second as i64 - (st.local.minute as i64 % 10) * 60;
    let unix = st.unix;
    let account = Some("a•••@example.com".to_string());
    let snap = |id: ProviderId, plan: &str, windows: Vec<Window>| {
        let mut s = Snapshot::new(id, unix - 40, Source::Online);
        s.plan = Some(plan.to_string()).filter(|p| !p.is_empty());
        s.account = account.clone();
        s.windows = windows;
        s
    };
    let five = |used: f32, at: i64| Window::new(Session, "5-hour", used).resets(Some(at)).length(Some(5 * 3600));
    let week = |label: &str, used: f32, at: i64| Window::new(Weekly, label, used).resets(Some(at)).length(Some(7 * 86_400));
    let codex = || snap(Codex, "Plus", vec![five(0.62, t + 2 * H + 20 * 60), week("Weekly", 0.31, t + 2 * DAY + 21 * H)]);
    let claude = || snap(Claude, "Max 5x", vec![five(0.84, t + H + 10 * 60), week("Weekly", 0.46, t + 4 * DAY + 3 * H), week("Weekly · Opus", 0.22, t + 4 * DAY + 3 * H)]);
    let opencode = || {
        let mut s = snap(OpenCode, "Go", vec![five(0.18, t + 3 * H + 50 * 60), week("Weekly", 0.52, t + DAY + 6 * H), Window::new(Monthly, "Monthly", 0.34).resets(Some(t + 19 * DAY))]);
        s.account = None;
        s
    };
    let inference = |st: &AppState| {
        let usd = [0.10, 0.32, 0.0, 0.18, 0.64, 0.41, 0.22, 0.05, 0.38, 0.71, 0.55, 0.12, 0.90, 0.42];
        let mut s = snap(Oyenza, "Pay as you go", Vec::new());
        s.balance = Some(Money::usd(18.4));
        let sum = |d: &[f64]| d.iter().sum::<f64>();
        s.spend = vec![
            Spend { label: "Today".into(), money: Money::usd(usd[13]), cap: None },
            Spend { label: "7 days".into(), money: Money::usd(sum(&usd[7..])), cap: None },
            Spend { label: "30 days".into(), money: Money::usd(sum(&usd) + 6.3), cap: None },
        ];
        s.days = usd
            .iter()
            .enumerate()
            .map(|(i, v)| DayUse { day: st.local.add_days(i as i64 - 13).day_key(), money: Some(Money::usd(*v)), requests: Some((v * 90.0) as u64), tokens: None })
            .collect();
        s
    };

    st.usage = crate::state::UsageState::default();
    st.settings.usage = poni_core::settings::UsageSettings::default();
    st.ui.usage = UsageUi::default();
    let put = |st: &mut AppState, id: ProviderId, outcome: Result<Snapshot, Trouble>| {
        let r = st.usage.readings.entry(id).or_insert(Reading { snapshot: None, trouble: None, checked_at: unix - 40 });
        match outcome {
            Ok(s) => r.snapshot = Some(s),
            Err(t) => r.trouble = Some(t),
        }
    };
    // Every service answers a refresh: those not on this computer say so.
    if shows != Picture::Looking {
        for id in ProviderId::ALL {
            let none = if usage::provider(id).takes_key().is_some() || id == Oyenza { Trouble::NeedsKey } else { Trouble::NotFound };
            put(st, id, Err(none));
        }
    }
    let readings = |st: &mut AppState, list: Vec<(ProviderId, Result<Snapshot, Trouble>)>| {
        for (id, outcome) in list {
            st.usage.readings.remove(&id);
            put(st, id, outcome);
        }
    };
    match shows {
        Picture::Plans | Picture::Services | Picture::Key => {
            let inf = inference(st);
            readings(st, vec![(Codex, Ok(codex())), (Claude, Ok(claude())), (OpenCode, Ok(opencode())), (Oyenza, Ok(inf))]);
            st.usage.inference = InferenceLink::On { account: "a•••@example.com".into() };
            st.usage.keys.insert(oyenza::SECRET);
            st.settings.usage.notices = shows == Picture::Plans;
        }
        Picture::Troubles => {
            let mut old = snap(Codex, "Plus", vec![five(0.38, t + H + 40 * 60), week("Weekly", 0.12, t + 5 * DAY)]);
            old.as_of = unix - 2 * H - 5 * 60;
            old.source = Source::Local;
            let out = snap(Claude, "Pro", vec![five(1.0, t + 50 * 60), week("Weekly", 0.71, t + 2 * DAY + 9 * H)]);
            let copilot = snap(Copilot, "Pro", vec![Window::new(Monthly, "Premium requests", 0.4).resets(Some(t + 12 * DAY)).counted(120, 300)]);
            readings(st, vec![(Codex, Ok(old)), (Claude, Ok(out)), (Gemini, Err(Trouble::Offline)), (Copilot, Ok(copilot)), (OpenRouter, Err(Trouble::Refused))]);
            if let Some(r) = st.usage.readings.get_mut(&Codex) {
                r.trouble = Some(Trouble::Expired);
            }
            if let Some(r) = st.usage.readings.get_mut(&Copilot) {
                r.trouble = Some(Trouble::Disabled);
            }
            st.settings.usage.added.push(OpenRouter.key().into());
            if let Some(k) = usage::provider(OpenRouter).takes_key() {
                st.usage.keys.insert(k.secret);
            }
        }
        Picture::Empty => {}
        Picture::Looking => st.usage.refreshing = true,
        Picture::Connect => {
            readings(st, vec![(Codex, Ok(codex()))]);
            // A code just asked for: good for ten minutes from now, on a whole minute.
            let expires_at = (unix + 10 * 60).div_euclid(60) * 60;
            st.usage.inference = InferenceLink::Code { code: "KPJX-TMNQ".into(), url: format!("{}/app/connect?code=KPJXTMNQ", oyenza::BASE), expires_at };
        }
        Picture::Inference => {
            let inf = inference(st);
            readings(st, vec![(Codex, Ok(codex())), (Oyenza, Ok(inf))]);
            st.usage.inference = InferenceLink::On { account: "a•••@example.com".into() };
            st.usage.keys.insert(oyenza::SECRET);
        }
    }
    if matches!(shows, Picture::Services | Picture::Key) {
        st.ui.usage.services = true;
    }
    if shows == Picture::Key {
        let s = &mut st.ui.usage;
        s.editing = Some(OpenRouter);
        s.key = "sk-or-v1-0000000000000000".into();
        s.reveal = Some(OpenRouter);
    }
    st.ui.usage.settle(now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state() -> AppState {
        let mut st = AppState::new(Settings::default(), Store::default());
        // Tue 27 Oct 2026, 12:00 in Jakarta (UTC+7).
        st.unix = 1_793_077_200;
        st.local = LocalDateTime { year: 2026, month: 10, day: 27, hour: 12, minute: 0, second: 0 };
        st
    }

    fn read(st: &mut AppState, id: ProviderId, snapshot: Option<Snapshot>, trouble: Option<Trouble>) {
        st.usage.readings.insert(id, Reading { snapshot, trouble, checked_at: st.unix });
    }

    #[test]
    fn a_service_not_here_has_no_card_unless_the_person_added_it() {
        let mut st = state();
        read(&mut st, ProviderId::Codex, None, Some(Trouble::NotFound));
        read(&mut st, ProviderId::OpenRouter, None, Some(Trouble::NeedsKey));
        assert!(!shown(&st, ProviderId::Codex) && !shown(&st, ProviderId::OpenRouter));
        st.settings.usage.added.push("openrouter".into());
        assert!(shown(&st, ProviderId::OpenRouter));
        // Its card says what's missing, with the button that fixes it.
        let p = parts(&st, ProviderId::OpenRouter);
        assert!(matches!(&p[..], [Part::Trouble { act: Some(Act { does: Does::Key, .. }), .. }]), "{p:?}");
        // Hidden wins over everything.
        let snap = Snapshot::new(ProviderId::Claude, st.unix, Source::Online);
        read(&mut st, ProviderId::Claude, Some(snap), None);
        assert!(shown(&st, ProviderId::Claude));
        st.settings.usage.hidden.push("claude".into());
        assert!(!shown(&st, ProviderId::Claude));
        // Gone since (uninstalled, key removed): the old numbers go with it.
        let snap = Snapshot::new(ProviderId::Codex, st.unix - 600, Source::Online);
        read(&mut st, ProviderId::Codex, Some(snap), Some(Trouble::NotFound));
        assert!(!shown(&st, ProviderId::Codex));
    }

    #[test]
    fn not_asked_online_yet_is_never_a_trouble() {
        let mut st = state();
        read(&mut st, ProviderId::OpenRouter, None, Some(Trouble::NeedsOnline));
        assert!(shown(&st, ProviderId::OpenRouter));
        assert_eq!(parts(&st, ProviderId::OpenRouter), [Part::Quiet(CHECKING.into())]);
        let mut s = Snapshot::new(ProviderId::OpenRouter, st.unix - 30, Source::Online);
        s.balance = Some(Money::usd(4.2));
        read(&mut st, ProviderId::OpenRouter, Some(s), Some(Trouble::NeedsOnline));
        let p = parts(&st, ProviderId::OpenRouter);
        assert_eq!(p, [Part::Balance(Money::usd(4.2))], "no warning, no note for fresh numbers");
    }

    #[test]
    fn old_or_local_numbers_say_where_they_came_from() {
        let mut st = state();
        let mut s = Snapshot::new(ProviderId::Codex, st.unix - 2 * 3600, Source::Local);
        s.windows.push(Window::new(usage::WindowKind::Session, "5-hour", 0.4));
        read(&mut st, ProviderId::Codex, Some(s.clone()), Some(Trouble::Expired));
        let p = parts(&st, ProviderId::Codex);
        assert!(p.contains(&Part::Note("From Codex's log, 2 h ago".into())), "{p:?}");
        assert!(p.contains(&Part::Warn("Open Codex once to refresh".into(), None)), "{p:?}");
        s.source = Source::Online;
        s.as_of = st.unix - 3 * 86_400;
        read(&mut st, ProviderId::Codex, Some(s), Some(Trouble::Offline));
        let p = parts(&st, ProviderId::Codex);
        assert!(p.contains(&Part::Note("Updated 3 days ago".into())), "{p:?}");
        assert!(matches!(p.last(), Some(Part::Warn(w, Some(Act { does: Does::Refresh, .. }))) if w == "Offline"), "{p:?}");
    }

    #[test]
    fn a_used_up_plan_points_to_oyenza_inference() {
        let mut st = state();
        let mut s = Snapshot::new(ProviderId::Claude, st.unix, Source::Online);
        s.windows.push(Window::new(usage::WindowKind::Session, "5-hour", 1.0).resets(Some(st.unix + 600)));
        read(&mut st, ProviderId::Claude, Some(s.clone()), None);
        assert_eq!(parts(&st, ProviderId::Claude).last(), Some(&Part::Nudge));
        // Not once the reset has passed, nor when the person hid Oyenza Inference.
        s.windows[0].resets_at = Some(st.unix - 1);
        read(&mut st, ProviderId::Claude, Some(s.clone()), None);
        assert!(!parts(&st, ProviderId::Claude).contains(&Part::Nudge));
        s.windows[0].resets_at = Some(st.unix + 600);
        read(&mut st, ProviderId::Claude, Some(s), None);
        st.settings.usage.hidden.push("oyenza".into());
        assert!(!parts(&st, ProviderId::Claude).contains(&Part::Nudge));
    }

    #[test]
    fn oyenza_inference_follows_its_connection() {
        let mut st = state();
        read(&mut st, ProviderId::Oyenza, None, Some(Trouble::NeedsKey));
        assert!(!shown(&st, ProviderId::Oyenza), "not connected, not added: in the Services list only");
        st.usage.inference = InferenceLink::Code { code: "KPJX-TMNQ".into(), url: "https://inference.oyenza.com/app/connect?code=KPJXTMNQ".into(), expires_at: st.unix + 600 };
        assert!(shown(&st, ProviderId::Oyenza));
        assert!(matches!(&parts(&st, ProviderId::Oyenza)[..], [Part::Code { .. }]));
        st.usage.inference = InferenceLink::Failed(oyenza::EXPIRED.into());
        let p = parts(&st, ProviderId::Oyenza);
        assert!(matches!(&p[..], [Part::Trouble { title, act: Some(Act { does: Does::Connect, .. }), .. }] if title == "Code expired"), "{p:?}");
        // Disconnected in the dashboard: its old balance goes, Connect again comes.
        st.usage.inference = InferenceLink::On { account: "a•••@example.com".into() };
        let mut s = Snapshot::new(ProviderId::Oyenza, st.unix, Source::Online);
        s.balance = Some(Money::usd(4.2));
        read(&mut st, ProviderId::Oyenza, Some(s), Some(Trouble::Refused));
        let p = parts(&st, ProviderId::Oyenza);
        assert!(matches!(&p[..], [Part::Trouble { title, act: Some(Act { does: Does::Connect, .. }), .. }] if title == "No longer connected"), "{p:?}");
        read(&mut st, ProviderId::Oyenza, None, Some(Trouble::Disabled));
        assert!(matches!(&parts(&st, ProviderId::Oyenza)[..], [Part::Trouble { act: None, .. }]));
    }

    #[test]
    fn every_trouble_has_plain_words() {
        let all = [
            Trouble::NotFound,
            Trouble::SignedOut,
            Trouble::Expired,
            Trouble::NeedsKey,
            Trouble::Refused,
            Trouble::Disabled,
            Trouble::Busy { retry_after_s: Some(300) },
            Trouble::Busy { retry_after_s: None },
            Trouble::Offline,
            Trouble::Unreadable("HTTP 404".into()),
            Trouble::Denied,
        ];
        for id in ProviderId::ALL {
            for t in &all {
                let w = words(id, t);
                for text in [&w.title, &w.line, &w.short] {
                    assert!(!text.is_empty() && !text.contains('—') && !text.contains('–'), "{id:?} {t:?}: {text:?}");
                    // The service's own wording never leaks through (it can be anything).
                    assert!(!text.contains("HTTP"), "{id:?} {t:?}: {text:?}");
                }
            }
        }
        assert_eq!(words(ProviderId::Codex, &Trouble::Busy { retry_after_s: Some(300) }).line, "Trying again in 5 min");
        assert_eq!(words(ProviderId::Copilot, &Trouble::Disabled).act.map(|a| a.label), Some("Open github.com".into()));
    }

    #[test]
    fn times_and_numbers_read_plainly() {
        let st = state();
        // 12:00 in Jakarta now: 14:20 today, Thursday 09:00, a week on.
        assert_eq!(resets_words(&st, st.unix + 2 * 3600 + 20 * 60), ("resets 14:20".into(), None));
        assert_eq!(resets_words(&st, st.unix + 45 * 3600), ("resets Thu 09:00".into(), Some("resets Thu".into())));
        assert_eq!(resets_words(&st, st.unix + 8 * 86_400).1, Some("resets 4 Nov".into()));
        assert_eq!(in_words(2 * 3600 + 10 * 60), "in 2 h 10 min");
        assert_eq!(in_words(40 * 60 - 5), "in 40 min");
        assert_eq!(in_words(3 * 86_400 + 3600), "in 3 days");
        assert_eq!((ago(30), ago(5 * 60), ago(2 * 3600 + 59), ago(30 * 3600), ago(4 * 86_400)), ("just now".into(), "5 min ago".into(), "2 h ago".into(), "yesterday".into(), "4 days ago".into()));
        assert_eq!((pct(0.62), pct(0.8 - 1e-7), pct(1.4)), ("62%".into(), "80%".into(), "140%".into()));
        assert_eq!((group(1_250), tokens(1_234_567), tokens(820_400)), ("1,250".into(), "1.2M".into(), "820K".into()));
        assert_eq!(money_words(&Money::usd(4.2)), ("$4.20".into(), "left".into()));
        assert_eq!(money_words(&Money { micros: 1_250_000_000, currency: "credits".into() }), ("1,250".into(), "credits left".into()));
        assert_eq!((host("https://www.github.com/settings/copilot"), host("https://openrouter.ai/settings/keys?x=1")), ("github.com", "openrouter.ai"));
        let mut st12 = state();
        st12.settings.use_24h = false;
        assert_eq!(clock_at(&st12, st12.unix + 2 * 3600 + 20 * 60), "2:20 PM");
    }

    #[test]
    fn a_key_is_offered_only_where_it_helps() {
        let mut st = state();
        let id = ProviderId::OpenRouter;
        let secret = usage::provider(id).takes_key().expect("OpenRouter takes a key").secret;
        let labels = |st: &AppState| row_buttons(st, id).into_iter().map(|b| b.0).collect::<Vec<_>>();
        read(&mut st, id, None, Some(Trouble::NeedsKey));
        assert_eq!(labels(&st), ["Add key"]);
        // Found without a typed key (its variable): nothing to add.
        let mut s = Snapshot::new(id, st.unix, Source::Online);
        s.balance = Some(Money::usd(1.0));
        read(&mut st, id, Some(s), None);
        assert!(labels(&st).is_empty());
        st.usage.keys.insert(secret);
        assert_eq!(labels(&st), ["Change key"]);
        st.ui.usage.editing = Some(id);
        assert_eq!(labels(&st), ["Cancel"]);
        // Oyenza Inference: its connection instead.
        assert_eq!(row_buttons(&st, ProviderId::Oyenza).into_iter().map(|b| b.0).collect::<Vec<_>>(), [oyenza::CONNECT]);
        st.usage.inference = InferenceLink::On { account: "a•••@example.com".into() };
        assert_eq!(row_buttons(&st, ProviderId::Oyenza).into_iter().map(|b| b.0).collect::<Vec<_>>(), [oyenza::DISCONNECT]);
    }

    #[test]
    fn a_typed_key_never_prints() {
        let mut st = state();
        st.ui.usage.key = "sk-or-v1-secret".into();
        let shown = format!("{:?}", st.ui);
        assert!(!shown.contains("secret") && shown.contains("<15 chars>"), "{shown}");
    }

    #[test]
    fn every_picture_has_its_cards() {
        for p in Picture::ALL {
            let mut st = state();
            picture(&mut st, p, 1.0);
            let ids: Vec<ProviderId> = ProviderId::ALL.into_iter().filter(|id| shown(&st, *id)).collect();
            use ProviderId::*;
            let want: &[ProviderId] = match p {
                Picture::Plans | Picture::Services | Picture::Key => &[Codex, Claude, OpenCode, Oyenza],
                Picture::Troubles => &[Codex, Claude, Gemini, Copilot, OpenRouter],
                Picture::Empty | Picture::Looking => &[],
                Picture::Connect | Picture::Inference => &[Codex, Oyenza],
            };
            assert_eq!(ids, want, "{p:?}");
            assert!(st.ui.usage.still, "{p:?} holds still");
            for id in ids {
                assert!(!parts(&st, id).is_empty());
            }
        }
        // Each picture starts clean: none carries another's state.
        let mut st = state();
        picture(&mut st, Picture::Key, 1.0);
        picture(&mut st, Picture::Empty, 1.0);
        assert!(!st.ui.usage.services && st.ui.usage.key.is_empty() && !st.settings.usage.notices);
    }
}
