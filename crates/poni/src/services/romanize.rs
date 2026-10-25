//! Pronunciation lines for lyrics: how Japanese, Korean and Chinese lines
//! read in Latin letters, the way Apple Music's "Show Pronunciation" sets them.
//!
//! Japanese kanji have no single reading, so Japanese goes through Windows'
//! own morphological analyzer (`Windows.Globalization.JapanesePhoneticAnalyzer`):
//! it splits a line into words with their readings in kana, which
//! `poni_core::romanize` spells in romaji. Where Windows has no Japanese
//! reading data (Windows 11 keeps it in the Japanese typing feature, which
//! most PCs outside Japan don't have) or the analyzer fails, the song is read
//! with Oyenza Studio's own dictionary instead (`poni_core::romanize::JaDict`),
//! which the Japanese readings add-on brings (`poni_core::addons::
//! JAPANESE_READINGS`, loaded only for that song). Without the add-on, kana
//! lines still read, and the song says so (`wants_ja_readings`): the app
//! switches the add-on on the first time, or the Music page offers it once
//! it was switched off by hand. Korean and Chinese need no analyzer
//! (`poni_core::romanize`). Called on the lyrics worker thread.
//!
//! The analyzer is a single-threaded (STA) class. From the lyrics worker's
//! multithreaded apartment every call fails with RO_E_UNSUPPORTED_FROM_MTA
//! (0x8000001D), so a Japanese song is read on a short-lived STA thread.

use poni_core::addons::{JAPANESE_READINGS, JA_READINGS_FILE};
use poni_core::lrc::Lyrics;
use poni_core::romanize::{self, JaWord, Ruby, Script};
use windows::core::HSTRING;
use windows::Globalization::JapanesePhoneticAnalyzer;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};

/// The analyzer reads at most 100 characters per call.
const ANALYZER_MAX: usize = 100;
/// Chinese runs longer than this are cut so a piece always fits the column.
const HAN_PIECE_MAX: usize = 8;

/// A song's readings.
#[derive(Default)]
pub struct Pronounced {
    /// The pieces of every line with their readings; an empty list for lines
    /// that need none (or that couldn't be read).
    pub ruby: Vec<Vec<Ruby>>,
    /// Kanji went unread because Windows has no Japanese reading data and the
    /// Japanese readings add-on isn't on this PC: the app switches it on the
    /// first time, and the Music page offers it after a switch-off by hand.
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
        None => Pronounced::default(),
        Some(Script::Korean) => Pronounced::read(each_line(&texts, romanize::korean_ruby)),
        Some(Script::Chinese) => Pronounced::read(each_line(&texts, |t| split_long(romanize::chinese_ruby(t)))),
        Some(Script::Japanese) => on_sta(|| japanese_song(&texts)).unwrap_or_else(|| {
            crate::pwarn!("lyrics: couldn't start the Japanese reading thread; reading with Oyenza Studio's own dictionary");
            own_song(&texts)
        }),
    }
}

/// `read` for every line that needs a reading.
fn each_line(texts: &[&str], mut read: impl FnMut(&str) -> Vec<Ruby>) -> Vec<Vec<Ruby>> {
    texts.iter().map(|t| if romanize::needs_roman(t) { read(t) } else { Vec::new() }).collect()
}

/// Every line of a Japanese song through the analyzer. If it fails outright
/// (no Japanese reading data, or an error) it isn't asked again for this
/// song, and the rest is read with Oyenza Studio's own dictionary. A word that
/// can't be read costs just that word's reading.
fn japanese_song(texts: &[&str]) -> Pronounced {
    let mut analyzer = true;
    let mut own = OwnDictionary::default();
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
                    crate::pinfo!("lyrics: {e}; reading with Oyenza Studio's own dictionary");
                }
            }
        }
        let pieces = own.read(t);
        unread += pieces.iter().filter(|p| p.roman.is_empty() && has_kanji(&p.text)).count();
        pieces
    });
    if unread > 0 {
        crate::pinfo!("lyrics: {unread} kanji words came back without a reading");
    }
    Pronounced { ruby: out, wants_ja_readings: own.wanted(texts) }
}

/// Every line of a Japanese song through Oyenza Studio's own dictionary (when the
/// analyzer's thread can't start, and for the self-test).
pub fn own_song(texts: &[&str]) -> Pronounced {
    let mut own = OwnDictionary::default();
    let ruby = each_line(texts, |t| own.read(t));
    Pronounced { ruby, wants_ja_readings: own.wanted(texts) }
}

/// Oyenza Studio's own reading dictionary from the Japanese readings add-on,
/// loaded on first use (about 0.1 s and 30 MB) and dropped with the song.
/// Without it, kana lines still read.
#[derive(Default)]
struct OwnDictionary {
    dict: Option<Option<romanize::JaDict>>,
    /// The add-on's file isn't on this PC (off, or still coming in).
    missing: bool,
}

impl OwnDictionary {
    fn read(&mut self, line: &str) -> Vec<Ruby> {
        if self.dict.is_none() {
            self.dict = Some(self.load());
        }
        match self.dict.as_ref().and_then(Option::as_ref) {
            Some(d) => d.ruby(line),
            None => romanize::japanese_kana_ruby(line).unwrap_or_default(),
        }
    }

    fn load(&mut self) -> Option<romanize::JaDict> {
        let path = super::addons::file_path(&JAPANESE_READINGS, JA_READINGS_FILE);
        match read_dictionary(&path) {
            Ok(bytes) => romanize::JaDict::load(&bytes).map_err(|e| crate::pwarn!("lyrics: {e}; only kana lines get romaji")).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::pinfo!("lyrics: the Japanese readings add-on isn't on this PC; only kana lines get romaji");
                self.missing = true;
                None
            }
            Err(e) => {
                crate::pwarn!("lyrics: couldn't read {} ({e}); only kana lines get romaji", path.display());
                None
            }
        }
    }

    /// Whether the song's kanji needed the add-on that isn't here.
    fn wanted(&self, texts: &[&str]) -> bool {
        self.missing && texts.iter().any(|t| has_kanji(t))
    }
}

/// The add-on's dictionary file. Unit tests read the repo's copy, built into
/// the test binary only.
#[cfg(not(test))]
fn read_dictionary(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

#[cfg(test)]
fn read_dictionary(_path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    Ok(include_bytes!("../../../poni-core/data/ja-readings.dic.zst").to_vec())
}

/// Whether Windows' analyzer reads Japanese on this PC (the self-test's
/// "required" mode: on Windows CI it must, and not via the fallback).
pub fn analyzer_works() -> Result<(), String> {
    on_sta(|| match japanese("今日は晴れ") {
        Ok((_, 0)) => Ok(()),
        Ok((_, n)) => Err(format!("Windows' Japanese reading analyzer left {n} kanji words unread")),
        Err(e) => Err(e),
    })
    .unwrap_or_else(|| Err("couldn't start the Japanese reading thread".into()))
}

/// One Japanese line through the analyzer, and how many of its kanji words
/// came back without a reading (those pieces get none).
pub(super) fn japanese(line: &str) -> Result<(Vec<Ruby>, usize), String> {
    let mut words: Vec<(String, String, bool)> = Vec::new();
    for chunk in chunks(line, ANALYZER_MAX) {
        let found = JapanesePhoneticAnalyzer::GetWords(&HSTRING::from(chunk)).map_err(|e| format!("Windows' Japanese reading analyzer failed ({e})"))?;
        if found.Size().unwrap_or(0) == 0 && romanize::needs_roman(chunk) {
            // Windows 11 keeps the analyzer's dictionary (IME\IMEJP\DICTS\
            // SDDS0411.DIC) in the Japanese typing feature; without it every
            // line comes back empty.
            return Err("Windows has no Japanese reading data (add Japanese typing: Settings > Time & language > Language & region)".into());
        }
        let mut read: Vec<(String, String, bool)> = Vec::new();
        for w in &found {
            let display = w.DisplayText().map_err(|e| format!("Windows' Japanese reading analyzer failed ({e})"))?.to_string_lossy();
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

/// The text has CJK ideographs (kanji).
fn has_kanji(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
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
pub(super) fn on_sta<R: Send>(f: impl FnOnce() -> R + Send) -> Option<R> {
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

    // Test lines are original phrases, not song lyrics.

    fn roman(pieces: &[Ruby]) -> String {
        pieces.iter().map(|r| r.roman.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn windows_reads_the_topic_particle_as_wa() {
        // Only where Windows has its Japanese reading data (Windows CI); without it
        // there's nothing of Windows' to check.
        if analyzer_works().is_err() {
            return;
        }
        for (line, want) in [("心には君がいる", "ni wa"), ("光を照らすは月", "terasu wa"), ("本当は好きだよ", "hontou wa"), ("それではまた", "de wa")] {
            let pieces = on_sta(|| japanese(line)).expect("the analyzer's thread starts").expect("Windows reads the line").0;
            let read = roman(&pieces);
            eprintln!("{line} -> {read}");
            assert!(read.contains(want) && !read.contains("ha"), "{line} read as {read:?}");
        }
    }

    #[test]
    fn own_dictionary_reads_kanji_lines() {
        let song = own_song(&["君の名前を呼んだ夜", "Oh yeah", "ありがとう"]);
        let read = &song.ruby;
        assert_eq!(roman(&read[0]), "kimi no namae wo yonda yoru");
        assert!(read[1].is_empty());
        assert_eq!(roman(&read[2]), "arigatou");
        assert!(!song.wants_ja_readings);
    }

    #[test]
    fn without_the_add_on_kana_still_reads_and_kanji_ask_for_it() {
        let mut own = OwnDictionary { dict: Some(None), missing: true };
        assert_eq!(roman(&own.read("ありがとう")), "arigatou");
        assert!(own.wanted(&["ありがとう", "君の名前"]));
        assert!(!own.wanted(&["ありがとう", "la la la"]), "kana alone never needs it");
        assert!(!OwnDictionary { dict: Some(None), missing: false }.wanted(&["君の名前"]), "a file that failed to load isn't fixed by the offer");
    }

    #[test]
    fn every_japanese_line_reads_with_or_without_windows_data() {
        // Windows' analyzer where it has its data (Windows CI), Oyenza Studio's own
        // dictionary where it doesn't (Wine, most Windows 11 PCs outside Japan).
        let lyrics = poni_core::lrc::parse_lrc("[00:01.00]朝の光を浴びて\n[00:02.00]遠くへ行こう\n[00:03.00]la la la\n");
        let song = pronounce(&lyrics);
        assert!(!song.wants_ja_readings);
        let read = &song.ruby;
        assert_eq!(read.len(), 3);
        assert!(read[0].iter().any(|p| p.text.contains('光') && !p.roman.is_empty()), "{:?}", read[0]);
        assert!(roman(&read[1]).starts_with("tooku e"), "{:?}", read[1]);
        assert!(read[2].is_empty());
    }

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
