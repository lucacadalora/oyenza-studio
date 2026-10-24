//! Apple Music's own lyrics for the lyrics worker: opt-in, with the user's
//! Apple Music token (`media-user-token`). Parsing lives in
//! `poni_core::applemusic`; this file does the HTTP.
//!
//! Per song: catalog search (artist + title) → the best match that has
//! time-synced lyrics → its TTML lyrics. The web player's developer token is
//! read from Apple's script bundle and cached on disk until it expires. The
//! user's token is checked with `/me/storefront`, which also gives the
//! account's storefront. Runs on the lyrics worker thread only.

use crate::services::http::{Client, FetchError, Response};
use poni_core::applemusic::{self, DevToken};
use poni_core::lrclib::{LrclibRecord, TrackQuery, MIN_LINES};
use poni_core::ttml::parse_ttml;
use std::time::Duration;

const USER_AGENT: &str = "Poni/0.1 (+https://github.com/lucacadalora/poni)";
/// API replies are small.
const API_TIMEOUT: Duration = Duration::from_secs(8);
/// The web player page and its script bundle are several MB each.
const BUNDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Songs in a row that Apple lists with synced lyrics but won't hand over
/// before giving up on the account (no active subscription).
const MAX_DENIED: u32 = 3;

/// The result of checking the user's token.
pub enum Check {
    Accepted,
    Rejected,
    Unreachable,
}

/// Why Apple gave no lyrics for a song.
pub enum Miss {
    /// A newer track came in, or the service is stopping.
    Abandoned,
    /// Apple didn't answer usefully; LRCLIB takes over for this song.
    Unreachable(String),
    /// Apple refuses the user's token.
    Rejected,
    /// The account can't read lyrics (no active subscription?).
    NoAccess,
}

pub struct AppleMusic {
    user_token: Option<String>,
    dev: Option<DevToken>,
    storefront: Option<String>,
    /// The token was refused, or the account can't read lyrics: no Apple
    /// lookups until the token changes.
    off: bool,
    denied_in_a_row: u32,
    /// Created on first use; reused so connections are pooled.
    client: Option<Client>,
}

impl AppleMusic {
    pub fn new() -> Self {
        let mut a = Self { user_token: None, dev: read_dev_token(), storefront: None, off: false, denied_in_a_row: 0, client: None };
        a.reload_token();
        a
    }

    /// Read the user's token from disk again (it was saved or removed).
    pub fn reload_token(&mut self) {
        self.user_token = crate::services::secret::load(&crate::paths::apple_music_token_path());
        self.storefront = None;
        self.off = false;
        self.denied_in_a_row = 0;
    }

    pub fn has_token(&self) -> bool {
        self.user_token.is_some()
    }

    /// Whether lookups should ask Apple first.
    pub fn enabled(&self) -> bool {
        self.user_token.is_some() && !self.off
    }

    /// Ask Apple whether it accepts the token.
    pub fn check(&mut self, live: &dyn Fn() -> bool) -> Check {
        self.storefront = None;
        match self.storefront(live) {
            Ok(_) => Check::Accepted,
            Err(Miss::Rejected) => Check::Rejected,
            Err(_) => Check::Unreachable,
        }
    }

    /// Apple's synced lyrics for `q` as an LRCLIB-style record; `Ok(None)`
    /// when Apple has none for it. `live` turns false when the lookup should
    /// stop (a newer track).
    pub fn find(&mut self, q: &TrackQuery, live: &dyn Fn() -> bool) -> Result<Option<LrclibRecord>, Miss> {
        let storefront = self.storefront(live)?;
        let resp = self.api(&applemusic::search_url(&storefront, q), live)?;
        if resp.status != 200 {
            return Err(Miss::Unreachable(format!("search answered HTTP {}", resp.status)));
        }
        let songs = applemusic::parse_search(&resp.body).map_err(|e| Miss::Unreachable(format!("unreadable search reply: {e}")))?;
        let Some(song) = applemusic::best_song(&songs, q).cloned() else { return Ok(None) };
        let resp = self.api(&applemusic::lyrics_url(&storefront, &song.id), live)?;
        match resp.status {
            200 => self.denied_in_a_row = 0,
            // The song is listed with synced lyrics, so a refusal is about the
            // account rather than the song.
            403 | 404 => {
                self.denied_in_a_row += 1;
                if self.denied_in_a_row >= MAX_DENIED {
                    self.off = true;
                    return Err(Miss::NoAccess);
                }
                return Ok(None);
            }
            s => return Err(Miss::Unreachable(format!("lyrics answered HTTP {s}"))),
        }
        let Some(ttml) = applemusic::parse_lyrics(&resp.body) else { return Ok(None) };
        let lyrics = parse_ttml(&ttml);
        if !lyrics.synced || lyrics.text_lines() < MIN_LINES {
            return Ok(None);
        }
        Ok(Some(applemusic::to_record(&song, &lyrics)))
    }

    /// The account's storefront, asked once per token; `/me/storefront`
    /// answers 403 to a token Apple doesn't accept.
    fn storefront(&mut self, live: &dyn Fn() -> bool) -> Result<String, Miss> {
        if let Some(sf) = &self.storefront {
            return Ok(sf.clone());
        }
        let resp = self.api(&applemusic::storefront_url(), live)?;
        match resp.status {
            200 => {
                let sf = applemusic::parse_storefront(&resp.body).unwrap_or_else(|| applemusic::DEFAULT_STOREFRONT.to_string());
                self.storefront = Some(sf.clone());
                Ok(sf)
            }
            403 => {
                self.off = true;
                Err(Miss::Rejected)
            }
            s => Err(Miss::Unreachable(format!("storefront answered HTTP {s}"))),
        }
    }

    /// One API call with both tokens. A 401 means the developer token went
    /// stale: fetch a fresh one and try once more.
    fn api(&mut self, url: &str, live: &dyn Fn() -> bool) -> Result<Response, Miss> {
        let resp = self.send(url, false, live)?;
        if resp.status != 401 {
            return Ok(resp);
        }
        crate::pinfo!("apple music: developer token refused; fetching a new one");
        self.send(url, true, live)
    }

    fn send(&mut self, url: &str, fresh_dev_token: bool, live: &dyn Fn() -> bool) -> Result<Response, Miss> {
        let Some(user) = self.user_token.clone() else { return Err(Miss::Rejected) };
        let dev = self.dev_token(fresh_dev_token, live)?;
        if !live() {
            return Err(Miss::Abandoned);
        }
        let auth = format!("Bearer {}", dev.token);
        // Apple answers 401 without an Origin from its own site.
        let headers = [
            ("Authorization", auth.as_str()),
            ("media-user-token", user.as_str()),
            ("Origin", applemusic::ORIGIN),
            ("Referer", "https://music.apple.com/"),
        ];
        self.client()?.get_with_headers(url, &headers, API_TIMEOUT).map_err(fetch_miss)
    }

    /// The cached developer token, or a fresh one from the web player.
    fn dev_token(&mut self, refresh: bool, live: &dyn Fn() -> bool) -> Result<DevToken, Miss> {
        if !refresh {
            if let Some(t) = &self.dev {
                if !applemusic::dev_token_stale(t, crate::clock::unix_now()) {
                    return Ok(t.clone());
                }
            }
        }
        let token = fetch_dev_token(self.client()?, live)?;
        write_dev_token(&token);
        self.dev = Some(token.clone());
        Ok(token)
    }

    fn client(&mut self) -> Result<&Client, Miss> {
        if self.client.is_none() {
            self.client = Some(Client::new(USER_AGENT).map_err(fetch_miss)?);
        }
        self.client.as_ref().ok_or_else(|| Miss::Unreachable("no HTTP client".into()))
    }
}

/// Web player page → its script bundle → the developer token inside.
fn fetch_dev_token(client: &Client, live: &dyn Fn() -> bool) -> Result<DevToken, Miss> {
    if !live() {
        return Err(Miss::Abandoned);
    }
    let page = client.get(applemusic::WEB_PLAYER_URL, BUNDLE_TIMEOUT).map_err(fetch_miss)?;
    if page.status != 200 {
        return Err(Miss::Unreachable(format!("web player answered HTTP {}", page.status)));
    }
    for url in applemusic::find_bundle_urls(&page.body) {
        if !live() {
            return Err(Miss::Abandoned);
        }
        let js = client.get(&url, BUNDLE_TIMEOUT).map_err(fetch_miss)?;
        if js.status == 200 {
            if let Some(token) = applemusic::find_developer_token(&js.body) {
                crate::pinfo!("apple music: new developer token (expires at {})", token.expires_at);
                return Ok(token);
            }
        }
    }
    Err(Miss::Unreachable("no developer token in the web player".into()))
}

fn fetch_miss(e: FetchError) -> Miss {
    Miss::Unreachable(e.to_string())
}

fn read_dev_token() -> Option<DevToken> {
    let text = std::fs::read_to_string(crate::paths::apple_music_web_token_path()).ok()?;
    poni_core::store::parse_json(&text).ok()
}

fn write_dev_token(t: &DevToken) {
    let path = crate::paths::apple_music_web_token_path();
    if let Err(e) = poni_core::store::write_json_atomic(&path, t) {
        crate::pwarn!("apple music: cannot cache the developer token ({e})");
    }
}
