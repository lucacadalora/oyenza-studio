//! Pronunciation lines for lyrics: how Japanese, Korean and Chinese lines
//! read in Latin letters, the way Apple Music's "Show Pronunciation" sets them.
//!
//! Korean and Chinese need nothing from the system (`poni_core::romanize`).
//! Japanese kanji have no single reading: Windows asks its own analyzer and
//! falls back to Oyenza Studio's dictionary (`poni_core::romanize::JaDict`); the
//! Mac reads every Japanese song with that dictionary, which the Japanese
//! readings add-on brings (`poni_core::addons::JAPANESE_READINGS`, loaded
//! only for that song). Without the add-on, kana lines still read and the
//! Music page offers it. Called on the lyrics worker thread.

use super::addons;
use poni_core::addons::{JAPANESE_READINGS, JA_READINGS_FILE};
use poni_core::lrc::Lyrics;
use poni_core::romanize::{self, Ruby, Script};

/// Chinese runs longer than this are cut so a piece always fits the column.
const HAN_PIECE_MAX: usize = 8;

/// A song's readings, in the shape the lyrics worker (shared with Windows)
/// takes.
pub struct Pronounced {
    /// The pieces of every line with their readings; an empty list for lines
    /// that need none (or that couldn't be read).
    pub ruby: Vec<Vec<Ruby>>,
    /// Kanji went unread because the Japanese readings add-on isn't on this
    /// Mac: the Music page offers it.
    pub wants_ja_readings: bool,
}

impl Pronounced {
    fn read(ruby: Vec<Vec<Ruby>>) -> Self {
        Pronounced { ruby, wants_ja_readings: false }
    }
}

pub fn pronounce(lyrics: &Lyrics) -> Pronounced {
    let texts: Vec<&str> = lyrics.lines.iter().map(|l| l.text.trim()).collect();
    match romanize::song_script(&texts) {
        None => Pronounced::read(Vec::new()),
        Some(Script::Korean) => Pronounced::read(each_line(&texts, romanize::korean_ruby)),
        Some(Script::Chinese) => Pronounced::read(each_line(&texts, |t| split_long(romanize::chinese_ruby(t)))),
        Some(Script::Japanese) => own_song(&texts),
    }
}

/// `read` for every line that needs a reading.
fn each_line(texts: &[&str], mut read: impl FnMut(&str) -> Vec<Ruby>) -> Vec<Vec<Ruby>> {
    texts.iter().map(|t| if romanize::needs_roman(t) { read(t) } else { Vec::new() }).collect()
}

/// Every line of a Japanese song through Oyenza Studio's own dictionary from the
/// Japanese readings add-on, loaded for this song (about 0.1 s and 30 MB)
/// and dropped with it. Without it, kana lines still read.
pub fn own_song(texts: &[&str]) -> Pronounced {
    let path = addons::file_path(&JAPANESE_READINGS, JA_READINGS_FILE);
    let mut missing = false;
    let dict = match std::fs::read(&path) {
        Ok(bytes) => romanize::JaDict::load(&bytes).map_err(|e| pwarn!("lyrics: {e}; only kana lines get romaji")).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            pinfo!("lyrics: the Japanese readings add-on isn't on this Mac; only kana lines get romaji");
            missing = true;
            None
        }
        Err(e) => {
            pwarn!("lyrics: couldn't read {} ({e}); only kana lines get romaji", path.display());
            None
        }
    };
    let ruby = each_line(texts, |t| match &dict {
        Some(d) => d.ruby(t),
        None => romanize::japanese_kana_ruby(t).unwrap_or_default(),
    });
    // The song's kanji needed the add-on that isn't here.
    Pronounced { ruby, wants_ja_readings: missing && texts.iter().any(|t| has_kanji(t)) }
}

fn has_kanji(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
}

/// Long Chinese runs split into pieces that fit the lyric column.
fn split_long(pieces: Vec<Ruby>) -> Vec<Ruby> {
    let mut out = Vec::with_capacity(pieces.len());
    for p in pieces {
        let chars: Vec<char> = p.text.chars().collect();
        let syllables: Vec<&str> = p.roman.split(' ').filter(|s| !s.is_empty()).collect();
        if chars.len() <= HAN_PIECE_MAX || syllables.len() != chars.len() {
            out.push(p);
            continue;
        }
        for (cs, ss) in chars.chunks(HAN_PIECE_MAX).zip(syllables.chunks(HAN_PIECE_MAX)) {
            out.push(Ruby { text: cs.iter().collect(), roman: ss.join(" ") });
        }
    }
    out
}
