//! Yandex Music's own lyrics for the lyrics worker: opt-in, with the user's
//! OAuth token. Signing and parsing live in `poni_core::yandex`; this file
//! does the HTTP.
//!
//! Per song: `/search?type=track` (anonymous) → the best hit whose
//! `lyricsInfo.hasAvailableSyncLyrics` is set → a signed `/lyrics?format=LRC`
//! → the presigned LRC file itself. Without Plus Yandex answers 200 with a
//! paywall notice, which switches Yandex off for this session.

use crate::services::applemusic::Miss;
use crate::services::http::{Client, FetchError};
use poni_core::lrclib::{self, LrclibRecord, TrackQuery};
use poni_core::urlenc::encode;
use poni_core::yandex;
use std::time::Duration;

const USER_AGENT: &str = "Poni/0.1 (+https://github.com/lucacadalora/oyenza-studio)";
/// API replies are small.
const API_TIMEOUT: Duration = Duration::from_secs(8);
/// The lyrics file is a few KB, but Yandex's bucket can be slow.
const FILE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Yandex {
    token: Option<String>,
    /// The account has no Plus: no Yandex lookups until the token changes.
    off: bool,
    /// Created on first use; reused so connections are pooled.
    client: Option<Client>,
}

impl Yandex {
    pub fn new() -> Self {
        let mut y = Self { token: None, off: false, client: None };
        y.reload_token();
        y
    }

    /// Read the user's token from disk again (it was saved or removed).
    pub fn reload_token(&mut self) {
        self.token = crate::services::secret::load(&crate::paths::yandex_token_path());
        self.off = false;
    }

    pub fn enabled(&self) -> bool {
        self.token.is_some() && !self.off
    }

    /// Synced lyrics for `q`, or `Ok(None)` when Yandex has none and LRCLIB
    /// should be asked instead.
    pub fn find(&mut self, q: &TrackQuery, live: &dyn Fn() -> bool) -> Result<Option<LrclibRecord>, Miss> {
        let Some(token) = self.token.clone() else {
            return Ok(None);
        };
        if self.off {
            return Ok(None);
        }
        let auth = format!("OAuth {token}");
        let text = if q.artist.trim().is_empty() { q.title.clone() } else { format!("{} {}", q.artist, q.title) };
        let search = format!("{}/search?text={}&type=track&page=0&nocorrect=false", yandex::API_BASE, encode(&text));
        let body = self.get(&search, &[("Authorization", auth.as_str())], live, API_TIMEOUT)?;
        if !live() {
            return Err(Miss::Abandoned);
        }
        let hits = yandex::parse_search(&body);
        let Some(hit) = best_hit(q, &hits) else {
            return Ok(None);
        };

        let ts = crate::clock::unix_now();
        let sign = yandex::sign(hit.id, ts);
        let url = format!(
            "{}/tracks/{}/lyrics?format=LRC&timeStamp={}&sign={}",
            yandex::API_BASE,
            hit.id,
            ts,
            encode(&sign)
        );
        let meta = self.get(
            &url,
            &[("Authorization", auth.as_str()), ("X-Yandex-Music-Client", yandex::CLIENT_HEADER)],
            live,
            API_TIMEOUT,
        )?;
        let Some(download) = yandex::download_url(&meta) else {
            crate::pinfo!("lyrics: Yandex has no file for {}", hit.id);
            return Ok(None);
        };
        if !live() {
            return Err(Miss::Abandoned);
        }
        let text = self.get(&download, &[], live, FILE_TIMEOUT)?;
        if yandex::is_paywall(&text) {
            crate::pwarn!("lyrics: Yandex wants Plus for lyrics; off until the token changes");
            self.off = true;
            return Err(Miss::NoAccess);
        }
        if !yandex::looks_synced(&text) {
            crate::pinfo!("lyrics: Yandex file for {} has no timings", hit.id);
            return Ok(None);
        }
        crate::pinfo!("lyrics: Yandex hit {} ({} sync)", hit.id, text.len());
        crate::pinfo!("lyrics: yandex debug hit_dur={:.1} playing_dur={}", hit.duration_s, q.duration_s);
        Ok(Some(yandex::record(&hit, text)))
    }

    fn get(&mut self, url: &str, headers: &[(&str, &str)], live: &dyn Fn() -> bool, timeout: Duration) -> Result<String, Miss> {
        if !live() {
            return Err(Miss::Abandoned);
        }
        let client = self.client()?;
        match client.get_with_headers(url, headers, timeout) {
            Ok(r) if r.status == 200 => Ok(r.body),
            Ok(r) if r.status == 401 => Err(Miss::Rejected),
            Ok(r) => Err(Miss::Unreachable(format!("Yandex error {}", r.status))),
            Err(e) => Err(fetch_miss(e)),
        }
    }

    fn client(&mut self) -> Result<&Client, Miss> {
        if self.client.is_none() {
            self.client = Some(Client::private(USER_AGENT).map_err(fetch_miss)?);
        }
        Ok(self.client.as_ref().expect("just set"))
    }
}

/// The best track whose lyrics are timed and whose title/artist match.
fn best_hit<'a>(q: &TrackQuery, hits: &'a [yandex::Hit]) -> Option<&'a yandex::Hit> {
    hits.iter()
        .filter(|h| h.has_sync)
        .filter_map(|h| {
            lrclib::match_score(q, &h.title, &h.artist, Some(h.duration_s), true).map(|score| (score, h))
        })
        .max_by_key(|(score, _)| *score)
        .filter(|(score, _)| *score >= lrclib::MIN_SCORE)
        .map(|(_, hit)| hit)
}

fn fetch_miss(e: FetchError) -> Miss {
    Miss::Unreachable(e.to_string())
}
