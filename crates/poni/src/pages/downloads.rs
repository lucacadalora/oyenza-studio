//! Downloads page (the Downloader add-on, `poni_core::addons::DOWNLOADER`):
//! a link field with Paste, Video or Sound, and Download, the app's message
//! under it for a few seconds ([`note`]), and a link copied before the page
//! came up waiting above the list (`downloads_offer`). The list: the
//! downloads, newest first, each with its site's letter, title, the line
//! under it (`poni_core::downloads::line`), a bar while it comes in, and
//! Stop (Cancel while it waits its turn), or Open, Show in folder and
//! remove; a finished file also drags out of its row, as from the Shelf.
//! yt-dlp does the work (`services::downloader`); nothing runs at
//! rest, and the page moves by itself only while its list glides to where
//! the wheel sent it.

use crate::commands::{Command, DownloadCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, CursorKind, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::downloads::{self, Job, State};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::ytdlp::Mode;

/// The page's height: the window never grows for it.
const HEIGHT: f32 = 256.0;
/// A message from the app ("Copy a link first") shows this long.
const NOTE_S: f64 = 4.0;
/// The link row (a field as tall as Tasks' and Stocks'); the list, or the
/// empty page, starts under it.
const FIELD_H: f32 = 34.0;
const LIST_TOP: f32 = 44.0;
/// A message's line under the field (the list moves down for it), and the
/// field's text column it starts in (`Ui::text_input_row`'s).
const NOTE_H: f32 = 18.0;
const FIELD_TEXT_X: f32 = 36.0;
/// The longest link the field takes.
const LINK_MAX: usize = 2048;
/// Video or Sound: the Settings picker, as tall as on Prayer and System.
const PICKER_H: f32 = 24.0;
/// A download's row, as the Add-ons page's: a badge, the title over its
/// line, and a bar under them while the bytes come in.
const ROW_H: f32 = 45.0;
const BADGE: f32 = 36.0;
const TEXT_X: f32 = 48.0;
const BAR_H: f32 = 3.0;
/// The remove button (an ×), and its gap to the pill before it: the icon
/// has room round it.
const REMOVE: f32 = 26.0;
const REMOVE_GAP: f32 = 4.0;
/// Clear finished sits this far under the list.
const FOOT_GAP: f32 = 4.0;
/// Height of the fade at a scrollable edge.
const FADE: f32 = 14.0;
/// Press-and-move distance (DIPs) that drags a finished file out (the Shelf's).
const DRAG_START: f32 = 6.0;
/// Where Show in folder shows the file.
const REVEAL: &str = if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in folder" };

/// What the page keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct DownloadsUi {
    /// The link being typed or pasted.
    pub link: String,
    pub mode: Mode,
    /// The list's scroll: where the wheel sent it, and where it shows on its way.
    pub scroll: f32,
    shown_scroll: f32,
    /// A link copied before the page came up, waiting above the list until
    /// it is downloaded or dismissed (`downloads_offer`).
    pub offer: Option<String>,
    /// The clipboard's link the last time the page came up: each link is
    /// offered once, so a dismissed one doesn't come back.
    pub offered: String,
    /// A message under the link field until a time (mono seconds).
    pub note: Option<(&'static str, f64)>,
    /// When the page last drew (mono seconds): the glide's step.
    drawn: f64,
    /// A finished file is being dragged out (once per press).
    dragging: bool,
}

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// Show a short message under the link field.
pub fn note(st: &mut AppState, text: &'static str) {
    st.ui.downloads.note = Some((text, st.now + NOTE_S));
}

/// When the message ends (the app's wake timer takes it away).
pub fn next_wake(st: &AppState) -> Option<f64> {
    st.ui.downloads.note.map(|(_, until)| until).filter(|&until| until > st.now)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let area = Rect::new(area.x, area.y, area.w, area.h.min(HEIGHT));
    if !ui.input.down {
        st.ui.downloads.dragging = false;
    }
    link_row(ui, st, Rect::new(area.x, area.y, area.w, FIELD_H));
    // A message from the app ("Copy a link first") under the field while it
    // lasts ([`next_wake`] brings the frame that takes it away).
    let mut top = area.y + LIST_TOP;
    if let Some((text, _)) = st.ui.downloads.note.filter(|&(_, until)| until > st.now) {
        let x = area.x + FIELD_TEXT_X;
        ui.c.text(text, Rect::new(x, top - 4.0, (area.right() - x).max(0.0), 16.0), &TextStyle::footnote().color(palette::TEXT_DIM));
        top += NOTE_H;
    }
    let list_r = Rect::new(area.x, top, area.w, (area.bottom() - top).max(0.0));
    // A link copied before the page came up waits above the list.
    let used = super::downloads_offer::draw(ui, st, list_r);
    let list_r = Rect::new(list_r.x, list_r.y + used, list_r.w, (list_r.h - used).max(0.0));
    if st.downloads.list.is_empty() {
        let s = &mut st.ui.downloads;
        s.scroll = 0.0;
        s.shown_scroll = 0.0;
        super::empty_state(ui, list_r, Icon::Download, "Paste a link to save its video or sound.", "yt-dlp does the work; files go to your Downloads folder.");
    } else {
        list(ui, st, list_r);
    }
}

/// The link field, then Paste, Video or Sound, and Download, which waits for
/// a link (`downloads::link_in`); Enter in the field downloads too.
fn link_row(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let accent = st.accent;
    let y = r.center().y - PILL_SMALL / 2.0;
    let modes = ["Video", "Sound"];
    // From the right: Download, the mode, Paste; the field takes the rest.
    let go_w = ui.pill_width("Download", true, true);
    let go = Rect::new(r.right() - go_w, y, go_w, PILL_SMALL);
    let mode_x = go.x - PILL_GAP - super::settings::picker_width(ui, &modes);
    let paste_w = ui.pill_width("Paste", true, true);
    let paste = Rect::new(mode_x - PILL_GAP - paste_w, y, paste_w, PILL_SMALL);
    let field = Rect::new(r.x, r.y, (paste.x - PILL_GAP - r.x).max(0.0), r.h);

    let s = &mut st.ui.downloads;
    let typed = ui.text_input_row(wid("downloads.link"), field, &mut s.link, "Paste a link", LINK_MAX, accent, Some(Icon::Plus));
    let sel = if s.mode == Mode::Audio { 1 } else { 0 };
    if let Some(i) = super::settings::picker(ui, "downloads.mode", mode_x, r.center().y - PICKER_H / 2.0, PICKER_H, &modes, sel) {
        s.mode = if i == 1 { Mode::Audio } else { Mode::Video };
    }
    let url = downloads::link_in(&s.link);
    let go_clicked = if url.is_some() {
        ui.pill(wid("downloads.go"), go, "Download", Some(Icon::Download), PillStyle::Primary(accent)).clicked
    } else {
        disabled_pill(ui, go, "Download", Icon::Download);
        false
    };
    if ui.pill(wid("downloads.paste"), paste, "Paste", Some(Icon::Clipboard), PillStyle::Utility).clicked {
        st.cmd(Command::Download(DownloadCmd::PasteLink));
    }
    // The new row comes in at the top of the list. The app empties the field
    // once it takes the download (a link already downloading stays, with a
    // note saying so).
    if let Some(url) = url.filter(|_| go_clicked || typed.submitted) {
        let mode = st.ui.downloads.mode;
        st.ui.downloads.scroll = 0.0;
        st.cmd(Command::Download(DownloadCmd::Start { url, mode }));
    }
}

/// The downloads, newest first, scrolling under the wheel (they glide there,
/// faded where more go on), and Clear finished under them once any has
/// finished, failed or stopped.
fn list(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let accent = st.accent;
    let downloads = &st.downloads;
    let jobs = &downloads.list;
    let s = &mut st.ui.downloads;
    let finished = jobs.iter().any(|j| !j.state.live());
    let foot = Rect::new(r.x, r.bottom() - PILL_SMALL, r.w, PILL_SMALL);
    let view_h = if finished { foot.y - FOOT_GAP - r.y } else { r.h };
    let view = Rect::new(r.x, r.y, r.w, view_h.max(0.0));

    // The wheel is taken only when there's more than shows (otherwise it
    // keeps switching pages).
    let max_scroll = (jobs.len() as f32 * ROW_H - view.h).max(0.0);
    if max_scroll > 0.0 {
        s.scroll -= ui.wheel_in(view) * ROW_H;
    }
    s.scroll = s.scroll.clamp(0.0, max_scroll);
    // After an idle gap (the frame clock was parked) step as if one frame passed.
    let now = ui.now;
    let dt = if now > s.drawn { (now - s.drawn).min(1.0 / 30.0) as f32 } else { 0.0 };
    s.drawn = now;
    let (scroll, moving) = super::tasks::glide(s.shown_scroll.clamp(0.0, max_scroll), s.scroll, dt);
    s.shown_scroll = scroll;
    if moving {
        ui.request_anim();
    }

    let mut cmds = Vec::new();
    ui.c.clip(view);
    for (i, job) in jobs.iter().enumerate() {
        let row = Rect::new(view.x, view.y + i as f32 * ROW_H - scroll, view.w, ROW_H);
        if row.bottom() > view.y && row.y < view.bottom() {
            let v = RowView { job, bar: downloads.progress(job.id), accent };
            draw_row(ui, v, row, view, &mut s.dragging, &mut cmds);
        }
    }
    if scroll > 0.5 {
        super::tasks::fade(ui, Rect::new(view.x, view.y, view.w, FADE), true);
    }
    if scroll < max_scroll - 0.5 {
        super::tasks::fade(ui, Rect::new(view.x, view.bottom() - FADE, view.w, FADE), false);
    }
    ui.c.unclip();

    if finished {
        let w = ui.pill_width("Clear finished", false, true);
        if ui.pill(wid("downloads.clear"), Rect::new(foot.right() - w, foot.y, w, PILL_SMALL), "Clear finished", None, PillStyle::Utility).clicked {
            cmds.push(Command::Download(DownloadCmd::ClearDone));
        }
    }
    for c in cmds {
        st.cmd(c);
    }
}

/// What a row's buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAct {
    Stop,
    /// Stop for one still waiting its turn: it leaves the list (`Jobs::stop`).
    Cancel,
    Open,
    Reveal,
    /// Off the list (the file stays).
    Remove,
}

impl RowAct {
    fn label(self) -> &'static str {
        match self {
            RowAct::Stop => "Stop",
            RowAct::Cancel => "Cancel",
            RowAct::Open => "Open",
            RowAct::Reveal => REVEAL,
            RowAct::Remove => "",
        }
    }
}

/// A download's buttons, left to right: Cancel while it waits, Stop while
/// it runs; Open, Show in folder and remove once it's done; remove once it
/// failed or was stopped.
fn row_acts(state: &State) -> &'static [RowAct] {
    match state {
        State::Queued => &[RowAct::Cancel],
        State::Done { .. } => &[RowAct::Open, RowAct::Reveal, RowAct::Remove],
        s if s.live() => &[RowAct::Stop],
        _ => &[RowAct::Remove],
    }
}

/// What one row shows.
struct RowView<'a> {
    job: &'a Job,
    /// How far it has come, all its parts together (`Jobs::progress`).
    bar: Option<f32>,
    accent: Color,
}

/// One download: its site's letter on a badge, its title over its line
/// (orange once it failed) with a bar while the bytes come in and while
/// they're joined, and its buttons on the right. Buttons the list's edges
/// (`view`) cut into don't show: none takes a click outside the list.
fn draw_row(ui: &mut Ui, v: RowView, r: Rect, view: Rect, dragging: &mut bool, cmds: &mut Vec<Command>) {
    let RowView { job, bar, accent } = v;
    let cy = r.center().y;
    let c = Point::new(r.x + BADGE / 2.0, cy);
    ui.c.fill_circle(c, BADGE / 2.0, accent.with_alpha(0.18));
    match badge_letter(job) {
        Some(l) => ui.c.text(&l, Rect::from_center(c, BADGE, BADGE), &TextStyle::headline().color(accent).center()),
        None => ui.c.icon(Icon::Download, Rect::from_center(c, 18.0, 18.0), accent),
    }

    // A finished file drags out of its row, as from the Shelf (the buttons,
    // registered after the row, keep their clicks).
    if let State::Done { path, .. } = &job.state {
        let top = r.y.max(view.y);
        let hit = Rect::new(r.x, top, r.w, (r.bottom().min(view.bottom()) - top).max(0.0));
        let resp = ui.interact(wid_n("downloads.row", job.id), hit, CursorKind::Arrow);
        if resp.pressed && !*dragging {
            if let Some((dx, dy)) = resp.drag {
                if dx * dx + dy * dy > DRAG_START * DRAG_START {
                    *dragging = true;
                    cmds.push(Command::DragOut(vec![path.clone()]));
                }
            }
        }
    }

    // The buttons, from the right edge leftwards.
    let (mut left, mut right) = (r.right(), r.right());
    for &act in row_acts(&job.state).iter().rev() {
        let (b, gap) = match act {
            RowAct::Remove => (Rect::new(right - REMOVE, cy - REMOVE / 2.0, REMOVE, REMOVE), REMOVE_GAP),
            _ => {
                let w = ui.pill_width(act.label(), false, true);
                (Rect::new(right - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL), PILL_GAP)
            }
        };
        left = b.x;
        right = b.x - gap;
        if b.y < view.y || b.bottom() > view.bottom() {
            continue;
        }
        let clicked = match act {
            RowAct::Stop | RowAct::Cancel => ui.pill(wid_n("downloads.stop", job.id), b, act.label(), None, PillStyle::Utility).clicked,
            RowAct::Open => ui.pill(wid_n("downloads.open", job.id), b, act.label(), None, PillStyle::Neutral).clicked,
            RowAct::Reveal => ui.pill(wid_n("downloads.reveal", job.id), b, act.label(), None, PillStyle::Utility).clicked,
            RowAct::Remove => {
                let color = if ui.hovered(b) { palette::TEXT } else { palette::TEXT_DIM };
                ui.icon_button(wid_n("downloads.remove", job.id), b, Icon::Close, color, 12.0).clicked
            }
        };
        if clicked {
            let cmd = match act {
                RowAct::Stop | RowAct::Cancel => DownloadCmd::Stop(job.id),
                RowAct::Open => DownloadCmd::Open(job.id),
                RowAct::Reveal => DownloadCmd::Reveal(job.id),
                RowAct::Remove => DownloadCmd::Remove(job.id),
            };
            cmds.push(Command::Download(cmd));
        }
    }

    let x = r.x + TEXT_X;
    let w = (left - 12.0 - x).max(0.0);
    let title = job.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| link_text(&job.url));
    ui.c.text(title, Rect::new(x, r.y + 3.0, w, 20.0), &TextStyle::headline());
    let line_color = if matches!(job.state, State::Failed(_)) { palette::ORANGE } else { palette::TEXT_DIM };
    ui.c.text(&downloads::line(job), Rect::new(x, r.y + 22.0, w, 17.0), &TextStyle::footnote().color(line_color));
    if let Some(p) = bar {
        // At the whole percent the island's pill says (`downloads::percent`).
        let p = downloads::percent(p) as f32 / 100.0;
        ui.progress_bar(wid_n("downloads.bar", job.id), Rect::new(x, r.y + 40.0, w, BAR_H), p, accent, false);
    }
}

/// The letter on a download's badge: its link's site's ("V" for
/// video.blender.org), for the row's whole life. Not yt-dlp's name for the
/// site (`Job::site`): that comes only once yt-dlp starts, and some sites it
/// names by their software or owner (a PeerTube server is "PeerTube",
/// archive.org "Internet Archive"), so the letter would change halfway, V to
/// P. Before yt-dlp starts only the link tells the site, and it never
/// changes; the preview names a site from its address alone the same way
/// (`ytdlp_info::site_name` with no extractor gives the domain).
fn badge_letter(job: &Job) -> Option<String> {
    host(&job.url).chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect())
}

/// A link as its row shows it until the title comes: without "https://" or
/// a user name and password (never shown, as on the copied link's chip).
fn link_text(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..end].rfind('@').map_or(rest, |at| &rest[at + 1..])
}

/// The link's site ("video.blender.org"), without "www." or a phone's "m."
/// (m.youtube.com is youtube.com), or the port.
fn host(url: &str) -> &str {
    let h = link_text(url).split(['/', '?', '#', ':']).next().unwrap_or("");
    h.strip_prefix("www.").or_else(|| h.strip_prefix("m.")).unwrap_or(h)
}

/// A Small capsule drawn inert (Focus's and Record's look at the small
/// size, no hit registered): Download, until there's a link.
fn disabled_pill(ui: &mut Ui, r: Rect, label: &str, icon: Icon) {
    ui.c.fill_rrect(r, r.h / 2.0, palette::SURFACE.mul_alpha(0.6));
    let st = TextStyle::footnote().weight(Weight::Semibold).color(palette::DISABLED);
    let tw = ui.c.measure(label, &st, r.w).0;
    let (isz, gap) = (13.0, 6.0);
    let x = r.x + (r.w - isz - gap - tw) / 2.0;
    ui.c.icon(icon, Rect::new(x, r.center().y - isz / 2.0, isz, isz), palette::DISABLED);
    ui.c.text(label, Rect::new(x + isz + gap, r.y, tw + 2.0, r.h), &st);
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::settings::Settings;
    use poni_core::store::Store;
    use poni_core::ytdlp::Stage;

    fn job(url: &str, site: Option<&str>) -> Job {
        Job { id: 1, url: url.into(), mode: Mode::Video, title: None, site: site.map(str::to_string), thumbnail: None, state: State::Queued, added: 0 }
    }

    #[test]
    fn each_row_offers_what_its_download_can_do() {
        // Waiting its turn: Cancel, the same command, which takes it off the list.
        assert_eq!(row_acts(&State::Queued), [RowAct::Cancel]);
        assert_eq!(RowAct::Cancel.label(), "Cancel");
        let downloading = State::Downloading { done: 1, total: None, speed: None, eta_s: None };
        for running in [State::Starting, downloading, State::Finishing(Stage::Merging)] {
            assert_eq!(row_acts(&running), [RowAct::Stop], "{running:?}");
        }
        let done = State::Done { path: "C:\\Users\\a\\Downloads\\Big Buck Bunny.mp4".into(), size: 4_200_000 };
        assert_eq!(row_acts(&done), [RowAct::Open, RowAct::Reveal, RowAct::Remove]);
        assert_eq!(row_acts(&State::Failed("This video is private.".into())), [RowAct::Remove]);
        assert_eq!(row_acts(&State::Stopped), [RowAct::Remove]);
    }

    #[test]
    fn the_badge_keeps_the_link_s_letter_once_yt_dlp_names_the_site() {
        let letter = |url, site| badge_letter(&job(url, site));
        let film = "https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605";
        assert_eq!(letter(film, None).as_deref(), Some("V"), "before yt-dlp starts");
        assert_eq!(letter(film, Some("PeerTube")).as_deref(), Some("V"), "and after: never P");
        assert_eq!(letter("https://archive.org/details/BigBuckBunny_124", Some("Internet Archive")).as_deref(), Some("A"));
        assert_eq!(letter("https://www.youtube.com/watch?v=x", None).as_deref(), Some("Y"));
        assert_eq!(letter("https://m.youtube.com/watch?v=x", Some("YouTube")).as_deref(), Some("Y"), "a phone's link, the same site");
        assert_eq!(letter("https://music.youtube.com/watch?v=x", None).as_deref(), Some("M"));
        assert_eq!(letter("http://video.blender.org:443/w/b29", Some(" ")).as_deref(), Some("V"));
        assert_eq!(link_text("https://archive.org/details/BigBuckBunny_124"), "archive.org/details/BigBuckBunny_124");
        assert_eq!(link_text("https://me:secret@archive.org/details/x@y"), "archive.org/details/x@y", "never a password");
        assert_eq!(letter("https://me:secret@www.archive.org", None).as_deref(), Some("A"));
    }

    #[test]
    fn the_page_never_makes_the_window_taller() {
        let st = AppState::new(Settings::default(), Store::default());
        assert!(height(&st) <= super::super::content_height(crate::pages::PageKind::System, &st));
    }
}
