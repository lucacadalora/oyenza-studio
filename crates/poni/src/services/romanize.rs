//! Pronunciation lines for lyrics: how Japanese, Korean and Chinese lines
//! read in Latin letters, the way Apple Music's "Show Pronunciation" sets them.
//!
//! Japanese kanji have no single reading, so Japanese goes through Windows'
//! own morphological analyzer (`Windows.Globalization.JapanesePhoneticAnalyzer`,
//! part of Windows, no download): it splits a line into words with their
//! readings in kana, which `poni_core::romanize` spells in romaji. Korean and
//! Chinese need no analyzer (`poni_core::romanize`). Runs on the lyrics
//! worker thread (COM is initialised there).

use poni_core::lrc::Lyrics;
use poni_core::romanize::{self, JaWord, Ruby, Script};
use windows::core::HSTRING;
use windows::Globalization::JapanesePhoneticAnalyzer;

/// The analyzer reads at most 100 characters per call.
const ANALYZER_MAX: usize = 100;
/// Chinese runs longer than this are cut so a piece always fits the column.
const HAN_PIECE_MAX: usize = 8;

/// The pieces of every line of `lyrics` with their readings; an empty list
/// for lines that need none (or that couldn't be read).
pub fn pronounce(lyrics: &Lyrics) -> Vec<Vec<Ruby>> {
    let texts: Vec<&str> = lyrics.lines.iter().map(|l| l.text.trim()).collect();
    let Some(script) = romanize::song_script(&texts) else { return Vec::new() };
    let mut analyzer_ok = true;
    let out: Vec<Vec<Ruby>> = texts
        .iter()
        .map(|t| {
            if !romanize::needs_roman(t) {
                return Vec::new();
            }
            match script {
                Script::Korean => romanize::korean_ruby(t),
                Script::Chinese => split_long(romanize::chinese_ruby(t)),
                Script::Japanese => {
                    let read = if analyzer_ok { japanese(t) } else { None };
                    if read.is_none() {
                        analyzer_ok = false;
                    }
                    read.or_else(|| romanize::japanese_kana_ruby(t)).unwrap_or_default()
                }
            }
        })
        .collect();
    if script == Script::Japanese && !analyzer_ok {
        crate::pwarn!("lyrics: Windows' Japanese reading analyzer is unavailable; only kana lines get romaji");
    }
    out
}

/// One Japanese line through the analyzer; `None` when it can't read it.
fn japanese(line: &str) -> Option<Vec<Ruby>> {
    let mut words: Vec<(String, String, bool)> = Vec::new();
    for chunk in chunks(line, ANALYZER_MAX) {
        let found = JapanesePhoneticAnalyzer::GetWords(&HSTRING::from(chunk)).ok()?;
        for w in &found {
            words.push((w.DisplayText().ok()?.to_string_lossy(), w.YomiText().ok()?.to_string_lossy(), w.IsPhraseStart().unwrap_or(true)));
        }
    }
    // An analyzer that hands kanji back as their own "reading" isn't working.
    let has_kanji = |s: &str| s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c));
    if words.iter().any(|(d, y, _)| has_kanji(d) && (y.is_empty() || has_kanji(y))) {
        return None;
    }
    let ja: Vec<JaWord> = words.iter().map(|(d, y, p)| JaWord { display: d, yomi: y, phrase_start: *p }).collect();
    Some(romanize::japanese_ruby(&ja))
}

/// `s` cut into pieces of at most `max` characters, at spaces when it can.
fn chunks(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut].rfind(' ').filter(|&i| i > 0).unwrap_or(cut);
        out.push(&rest[..at]);
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Long runs of Chinese characters cut into pieces of `HAN_PIECE_MAX`, each
/// keeping its own syllables (one per character).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_stay_under_the_limit() {
        assert_eq!(chunks("abc def", 100), vec!["abc def"]);
        assert_eq!(chunks("aaaa bbbb cccc", 9), vec!["aaaa", "bbbb cccc"]);
        let long = "あ".repeat(250);
        assert!(chunks(&long, 100).iter().all(|c| c.chars().count() <= 100));
    }

    #[test]
    fn long_han_runs_split_with_their_syllables() {
        let p = Ruby { text: "一二三四五六七八九十".into(), roman: "yī èr sān sì wǔ liù qī bā jiǔ shí".into() };
        let out = split_long(vec![p]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "一二三四五六七八");
        assert_eq!(out[1].roman, "jiǔ shí");
    }
}
