//! Pronunciation lines for lyrics: how Japanese, Korean and Chinese lines
//! read in Latin letters, the way Apple Music's "Show Pronunciation" sets them.
//!
//! Japanese kanji have no single reading, so Japanese goes through Windows'
//! own morphological analyzer (`Windows.Globalization.JapanesePhoneticAnalyzer`,
//! part of Windows, no download): it splits a line into words with their
//! readings in kana, which `poni_core::romanize` spells in romaji. Korean and
//! Chinese need no analyzer (`poni_core::romanize`). Called on the lyrics
//! worker thread.
//!
//! The analyzer is a single-threaded (STA) class. From the lyrics worker's
//! multithreaded apartment every call fails with RO_E_UNSUPPORTED_FROM_MTA
//! (0x8000001D), so a Japanese song is read on a short-lived STA thread.

use poni_core::lrc::Lyrics;
use poni_core::romanize::{self, JaWord, Ruby, Script};
use windows::core::HSTRING;
use windows::Globalization::JapanesePhoneticAnalyzer;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};

/// The analyzer reads at most 100 characters per call.
const ANALYZER_MAX: usize = 100;
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
        Some(Script::Japanese) => on_sta(|| japanese_song(&texts)).unwrap_or_else(|| {
            crate::pwarn!("lyrics: couldn't start the Japanese reading thread; only kana lines get romaji");
            each_line(&texts, |t| romanize::japanese_kana_ruby(t).unwrap_or_default())
        }),
    }
}

/// `read` for every line that needs a reading.
fn each_line(texts: &[&str], mut read: impl FnMut(&str) -> Vec<Ruby>) -> Vec<Vec<Ruby>> {
    texts.iter().map(|t| if romanize::needs_roman(t) { read(t) } else { Vec::new() }).collect()
}

/// Every line of a Japanese song through the analyzer. If it fails outright
/// (missing, or no words back) it isn't asked again for this song and only
/// kana lines get romaji. A word it can't read costs just that word's reading.
fn japanese_song(texts: &[&str]) -> Vec<Vec<Ruby>> {
    let mut analyzer = true;
    let mut unread = 0;
    let out = each_line(texts, |t| {
        if analyzer {
            match japanese(t) {
                Ok((pieces, n)) => {
                    unread += n;
                    return pieces;
                }
                Err(e) => {
                    analyzer = false;
                    crate::pwarn!("lyrics: Windows' Japanese reading analyzer failed ({e}); only kana lines get romaji");
                }
            }
        }
        romanize::japanese_kana_ruby(t).unwrap_or_default()
    });
    if unread > 0 {
        crate::pinfo!("lyrics: {unread} kanji words came back without a reading");
    }
    out
}

/// One Japanese line through the analyzer, and how many of its kanji words
/// came back without a reading (those pieces get none).
fn japanese(line: &str) -> Result<(Vec<Ruby>, usize), String> {
    let mut words: Vec<(String, String, bool)> = Vec::new();
    for chunk in chunks(line, ANALYZER_MAX) {
        let found = JapanesePhoneticAnalyzer::GetWords(&HSTRING::from(chunk)).map_err(|e| e.to_string())?;
        if found.Size().unwrap_or(0) == 0 && romanize::needs_roman(chunk) {
            return Err("it returned no words".into());
        }
        let mut read: Vec<(String, String, bool)> = Vec::new();
        for w in &found {
            let display = w.DisplayText().map_err(|e| e.to_string())?.to_string_lossy();
            let yomi = w.YomiText().map(|y| y.to_string_lossy()).unwrap_or_default();
            read.push((display, yomi, w.IsPhraseStart().unwrap_or(true)));
        }
        // The analyzer hands Latin letters and spaces back in full width
        // ("ｈｏｌｄ", "　"): the pieces show the line's own characters.
        if let Some(own) = align(chunk, read.iter().map(|w| w.0.as_str())) {
            for (w, text) in read.iter_mut().zip(own) {
                w.0 = text.to_string();
            }
        } else {
            for w in &mut read {
                w.0 = w.0.chars().map(narrow).collect();
            }
        }
        words.extend(read);
    }
    // Kanji handed back as their own "reading" (or with none) aren't read.
    let has_kanji = |s: &str| s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c));
    let mut unread = 0;
    let ja: Vec<JaWord> = words
        .iter()
        .map(|(d, y, p)| {
            let ok = !has_kanji(d) || (!y.is_empty() && !has_kanji(y));
            unread += usize::from(!ok);
            JaWord { display: d, yomi: if ok { y } else { "" }, phrase_start: *p }
        })
        .collect();
    Ok((romanize::japanese_ruby(&ja), unread))
}

/// `line` cut into the analyzer's `words` (same characters up to width), or
/// None when they don't line up.
fn align<'a, 'w>(line: &'a str, words: impl Iterator<Item = &'w str>) -> Option<Vec<&'a str>> {
    let mut out = Vec::new();
    let mut rest = line;
    for w in words {
        let mut end = 0;
        let mut own = rest.char_indices();
        for c in w.chars() {
            let (i, o) = own.next()?;
            if narrow(o) != narrow(c) {
                return None;
            }
            end = i + o.len_utf8();
        }
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    rest.is_empty().then_some(out)
}

/// Full-width ASCII and the ideographic space as plain ASCII.
fn narrow(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

/// `s` cut into consecutive pieces of at most `max` characters, after a
/// space when it can; together they are `s` exactly.
fn chunks(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut].rfind(' ').map(|i| i + 1).filter(|&i| i < cut).unwrap_or(cut);
        out.push(&rest[..at]);
        rest = &rest[at..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Run `f` on a short-lived thread in a single-threaded COM apartment
/// (synchronous calls only, so it needs no message loop). None when the
/// thread can't start or `f` panics.
fn on_sta<R: Send>(f: impl FnOnce() -> R + Send) -> Option<R> {
    struct Sta(bool);
    impl Drop for Sta {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    std::thread::scope(|s| {
        let t = std::thread::Builder::new().name("poni-romaji".into()).spawn_scoped(s, || {
            let _com = Sta(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok());
            f()
        });
        t.ok()?.join().ok()
    })
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
        assert_eq!(chunks("aaaa bbbb cccc", 9), vec!["aaaa ", "bbbb cccc"]);
        let long = "あ".repeat(250);
        let parts = chunks(&long, 100);
        assert!(parts.iter().all(|c| c.chars().count() <= 100));
        assert_eq!(parts.concat(), long);
    }

    #[test]
    fn full_width_words_map_back_onto_the_line() {
        let words = ["指", "で", "ｈｏｌｄ", "　", "ｍｅ", "　", "ｔｉｇｈｔ"];
        assert_eq!(align("指でhold me tight", words.into_iter()), Some(vec!["指", "で", "hold", " ", "me", " ", "tight"]));
        // A word the line doesn't have: no alignment.
        assert_eq!(align("指で", ["指", "で", "ｘ"].into_iter()), None);
        // The analyzer dropped a character: no alignment either.
        assert_eq!(align("指で!", ["指", "で"].into_iter()), None);
    }

    #[test]
    fn analyzer_runs_on_its_own_sta_thread() {
        let caller = std::thread::current().id();
        assert_ne!(on_sta(|| std::thread::current().id()), Some(caller));
        assert_eq!(on_sta(|| -> u8 { panic!("analyzer blew up") }), None);
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
