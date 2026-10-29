//! The Downloads page's copied link: a link the person copied before the
//! page came up waits above the list as a chip, "Download copied link" over
//! its site, with Video and Sound to start it at once and × to put it away.
//! Each app reads the clipboard only as the page comes up
//! (`App::downloads_page_shown` fills `DownloadsUi::offer`, in
//! `app_downloads.rs` and the Mac's `mac/app_downloads.rs`).

use crate::commands::{Command, DownloadCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::geom::{palette, Point, Rect};
use poni_core::ytdlp::Mode;

/// The chip's height, and the gap under it before the list.
const CHIP_H: f32 = 44.0;
const GAP: f32 = 8.0;
const BADGE: f32 = 28.0;
/// The × button.
const CLOSE: f32 = 28.0;

/// Draws the offer at the top of `r` (the room above the list) and returns
/// the height it took, the gap under it included: 0 with nothing to offer.
pub fn draw(ui: &mut Ui, st: &mut AppState, r: Rect) -> f32 {
    let Some(link) = st.ui.downloads.offer.clone() else { return 0.0 };
    // Started or pasted since: nothing left to offer.
    if st.ui.downloads.link.trim() == link || st.downloads.list.iter().any(|j| j.url == link) {
        st.ui.downloads.offer = None;
        return 0.0;
    }
    let (accent, pick) = (st.accent, st.ui.downloads.mode);
    let chip = Rect::new(r.x, r.y, r.w, CHIP_H);
    let cy = chip.center().y;
    ui.c.fill_rrect(chip, 12.0, palette::SURFACE);
    let badge = Point::new(chip.x + 8.0 + BADGE / 2.0, cy);
    ui.c.fill_circle(badge, BADGE / 2.0, accent.with_alpha(0.18));
    ui.c.icon(Icon::Clipboard, Rect::from_center(badge, 15.0, 15.0), accent);

    // From the right: ×, then Sound and Video, the one picked on the page in colour.
    let close = Rect::new(chip.right() - 8.0 - CLOSE, cy - CLOSE / 2.0, CLOSE, CLOSE);
    let dismissed = ui.icon_button(wid("downloads.offer.close"), close, Icon::Close, palette::TEXT_DIM, 13.0).clicked;
    let pw = ui.pill_width("Video", false, true).max(ui.pill_width("Sound", false, true)).max(64.0);
    let sound = Rect::new(close.x - 4.0 - pw, cy - PILL_SMALL / 2.0, pw, PILL_SMALL);
    let video = Rect::new(sound.x - PILL_GAP - pw, sound.y, pw, PILL_SMALL);
    let style = |m: Mode| if m == pick { PillStyle::Primary(accent) } else { PillStyle::Neutral };
    let mut start = None;
    if ui.pill(wid("downloads.offer.video"), video, "Video", None, style(Mode::Video)).clicked {
        start = Some(Mode::Video);
    }
    if ui.pill(wid("downloads.offer.sound"), sound, "Sound", None, style(Mode::Audio)).clicked {
        start = Some(Mode::Audio);
    }

    let x = badge.x + BADGE / 2.0 + 10.0;
    let w = (video.x - 12.0 - x).max(0.0);
    ui.c.text("Download copied link", Rect::new(x, chip.y + 5.0, w, 18.0), &TextStyle::body().weight(Weight::Semibold));
    ui.c.text(&host(&link), Rect::new(x, chip.y + 23.0, w, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    if let Some(mode) = start {
        st.cmd(Command::Download(DownloadCmd::Start { url: link, mode }));
        // The new download comes in at the top of the list.
        st.ui.downloads.scroll = 0.0;
        st.ui.downloads.offer = None;
    } else if dismissed {
        st.ui.downloads.offer = None;
    }
    CHIP_H + GAP
}

/// The site of `link` as the chip names it: "video.blender.org" for
/// "https://video.blender.org/w/…", in lower case, without "www.", the port
/// or a user name and password (never shown).
fn host(link: &str) -> String {
    let rest = link.split_once("://").map_or(link, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // An IPv6 address keeps its brackets; any other host loses its port.
    let host = match host.find(']') {
        Some(i) if host.starts_with('[') => &host[..=i],
        _ => host.split(':').next().unwrap_or(""),
    };
    let host = host.to_ascii_lowercase();
    host.strip_prefix("www.").map(str::to_string).unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn names_the_site() {
        assert_eq!(host("https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605"), "video.blender.org");
        assert_eq!(host("https://www.youtube.com/watch?v=x"), "youtube.com");
        assert_eq!(host("https://archive.org?x=1#top"), "archive.org");
        assert_eq!(host("HTTPS://SoundCloud.com:443/forss/flickermood"), "soundcloud.com");
        assert_eq!(host("http://[::1]:8080/a"), "[::1]");
    }

    #[test]
    fn never_shows_a_password() {
        assert_eq!(host("https://me:secret@archive.org/details/x"), "archive.org");
        assert_eq!(host("https://me@www.archive.org"), "archive.org");
    }
}
