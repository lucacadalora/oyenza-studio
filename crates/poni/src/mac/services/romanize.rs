//! Pronunciation lines for lyrics: how Japanese, Korean and Chinese lines
//! read in Latin letters, the way Apple Music's "Show Pronunciation" sets them.
//!
//! Korean and Chinese need nothing from the system (`poni_core::romanize`).
//! Japanese kanji have no single reading: Windows asks its own analyzer and
//! falls back to Oyen Poni's dictionary (`poni_core::romanize::JaDict`, built
//! into the app and loaded only for that song); the Mac reads every Japanese
//! song with that dictionary. Called on the lyrics worker thread.

use poni_core::lrc::Lyrics;
use poni_core::romanize::{self, Ruby, Script};

/// Chinese runs longer than this are cut so a piece always fits the column.
const HAN_PIECE_MAX: usize = 8;

/// The pieces of every line of `lyrics` with their readings; an empty list
/// for lines that need none (or that couldn't be read).
pub fn pronounce(lyrics: &Lyrics) -> Vec<Vec<Ruby>> {
    let texts: Vec<&str> = lyrics.lines.iter().map(|l| l.text.trim()).collect();
    match romanize::song_script(&texts) {
        None => Vec::new(),
        Some(Script::Korean) => each_line(&texts, romanize::korean_ruby),
        Some(Script::Chinese) => each_line(&texts, |t| split_long(romanize::chinese_ruby(t))),
        Some(Script::Japanese) => own_song(&texts),
    }
}

/// `read` for every line that needs a reading.
fn each_line(texts: &[&str], mut read: impl FnMut(&str) -> Vec<Ruby>) -> Vec<Vec<Ruby>> {
    texts.iter().map(|t| if romanize::needs_roman(t) { read(t) } else { Vec::new() }).collect()
}

/// Every line of a Japanese song through Oyen Poni's own dictionary. If it
/// can't load, kana lines still read.
pub fn own_song(texts: &[&str]) -> Vec<Vec<Ruby>> {
    let dict = romanize::JaDict::embedded().map_err(|e| pwarn!("lyrics: {e}; only kana lines get romaji")).ok();
    each_line(texts, |t| match &dict {
        Some(d) => d.ruby(t),
        None => romanize::japanese_kana_ruby(t).unwrap_or_default(),
    })
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
