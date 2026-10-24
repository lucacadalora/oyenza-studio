//! Japanese readings from Oyen Poni's own dictionary, for PCs where Windows'
//! reading analyzer has no data (Windows 11 keeps it in the Japanese typing
//! feature, which most PCs outside Japan don't have).
//!
//! The dictionary is mecab-ipadic 2.7.0-20070801 trimmed for lyrics and
//! compiled for vibrato, a Rust port of MeCab's method: `data/ja-readings.dic.zst`
//! (3.8 MB packed, 14 MB loaded), built by `tools/make_ja_dict.py`. Every copy
//! must carry mecab-ipadic's notice (THIRD-PARTY-NOTICES.txt). A line is split
//! into the cheapest chain of dictionary words, and the words go through the
//! same [`japanese_ruby`] as the analyzer's.
//!
//! Each entry's feature is a class letter, then its reading in katakana when
//! the word has kanji ("N,ヨル"): N content word, P particle, A auxiliary verb,
//! T te/ta ending, J suffix or dependent word, F prefix, S symbol.

use super::{is_han, is_kana, japanese_ruby, JaWord, Ruby};
use std::io::Read;
use vibrato::{Dictionary, Tokenizer};

/// The packed dictionary, built into the binary.
#[cfg(feature = "ja-dict")]
static PACKED: &[u8] = include_bytes!("../../data/ja-readings.dic.zst");

/// Lines longer than this are read in chunks, cut after a space when there is one.
const CHUNK_MAX: usize = 400;

/// A loaded reading dictionary (about 14 MB of memory).
pub struct JaDict {
    tokenizer: Tokenizer,
}

/// A word with its reading in hiragana (empty when it's written in kana, or
/// has kanji the dictionary doesn't know).
struct Word {
    display: String,
    yomi: String,
    phrase_start: bool,
    class: char,
}

impl JaDict {
    /// The dictionary built into the binary (about 0.1 s in a release build).
    #[cfg(feature = "ja-dict")]
    pub fn embedded() -> Result<JaDict, String> {
        JaDict::load(PACKED)
    }

    /// Load a dictionary packed by `tools/make_ja_dict.py`.
    pub fn load(packed: &[u8]) -> Result<JaDict, String> {
        let fail = |e: &dyn std::fmt::Display| format!("couldn't load Oyen Poni's Japanese reading dictionary ({e})");
        let mut raw = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(packed).map_err(|e| fail(&e))?.read_to_end(&mut raw).map_err(|e| fail(&e))?;
        let dict = Dictionary::read(&raw[..]).map_err(|e| fail(&e))?;
        Ok(JaDict { tokenizer: Tokenizer::new(dict) })
    }

    /// `line` in pieces with their romaji, as [`japanese_ruby`] gives them for
    /// Windows' analyzer: joining the pieces' text gives back the line.
    pub fn ruby(&self, line: &str) -> Vec<Ruby> {
        let words = self.words(line);
        let ja: Vec<JaWord> = words.iter().map(|w| JaWord { display: &w.display, yomi: &w.yomi, phrase_start: w.phrase_start }).collect();
        japanese_ruby(&ja)
    }

    /// The line's words, with te/ta endings joined to their verb the way
    /// Windows' analyzer splits verbs (呼ん|だ becomes 呼んだ, so it reads
    /// "yonda", not "yon da").
    fn words(&self, line: &str) -> Vec<Word> {
        let mut worker = self.tokenizer.new_worker();
        let mut words: Vec<Word> = Vec::new();
        for chunk in chunks(line, CHUNK_MAX) {
            worker.reset_sentence(chunk);
            worker.tokenize();
            let mut after_prefix = false;
            for i in 0..worker.num_tokens() {
                let token = worker.token(i);
                let surface = token.surface();
                let (class, reading) = feature(token.feature());
                if class == 'T' {
                    if let Some(verb) = words.last_mut().filter(|w| matches!(w.class, 'N' | 'J') && kana_known(w)) {
                        if verb.yomi.is_empty() {
                            verb.yomi = hiragana(&verb.display);
                        }
                        verb.yomi.push_str(&hiragana(surface));
                        verb.display.push_str(surface);
                        continue;
                    }
                }
                words.push(Word {
                    display: surface.to_string(),
                    yomi: reading.map(hiragana).unwrap_or_default(),
                    phrase_start: matches!(class, 'N' | 'F' | 'S') && !after_prefix,
                    class,
                });
                after_prefix = class == 'F';
            }
        }
        words
    }
}

/// A token's class letter and reading (katakana), from "N,ヨル" or "P".
fn feature(f: &str) -> (char, Option<&str>) {
    let (class, reading) = f.split_once(',').unwrap_or((f, ""));
    (class.chars().next().unwrap_or('N'), Some(reading).filter(|r| !r.is_empty()))
}

/// A Japanese word whose kana are known: written in kana, or with a reading.
fn kana_known(w: &Word) -> bool {
    w.display.chars().any(|c| is_kana(c) || is_han(c)) && (!w.yomi.is_empty() || !w.display.chars().any(is_han))
}

/// Katakana as hiragana (the analyzer's readings are hiragana too).
fn hiragana(s: &str) -> String {
    s.chars().map(|c| if ('\u{30A1}'..='\u{30F6}').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c }).collect()
}

/// `s` cut into consecutive chunks of at most `max` characters, after a
/// space when it can; together they are `s` exactly.
fn chunks(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut].rfind(char::is_whitespace).map(|i| i + rest[i..].chars().next().map_or(1, char::len_utf8)).filter(|&i| i < cut).unwrap_or(cut);
        out.push(&rest[..at]);
        rest = &rest[at..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    // Test lines are original phrases, not song lyrics.

    fn dict() -> &'static JaDict {
        static DICT: OnceLock<JaDict> = OnceLock::new();
        DICT.get_or_init(|| JaDict::load(include_bytes!("../../data/ja-readings.dic.zst")).expect("dictionary loads"))
    }

    fn rb(text: &str, roman: &str) -> Ruby {
        Ruby { text: text.into(), roman: roman.into() }
    }

    fn roman(line: &str) -> String {
        let pieces = dict().ruby(line);
        let parts: Vec<&str> = pieces.iter().map(|p| p.roman.as_str()).filter(|s| !s.is_empty()).collect();
        parts.join(" ")
    }

    #[test]
    fn kanji_lines_read_like_the_analyzer() {
        assert_eq!(
            dict().ruby("君の名前を呼んだ夜"),
            vec![rb("君の", "kimi no"), rb("名前を", "namae wo"), rb("呼んだ", "yonda"), rb("夜", "yoru")]
        );
        assert_eq!(roman("私は学校へ行く"), "watashi wa gakkou e iku");
        assert_eq!(roman("言えなかった言葉を運んで"), "ienakatta kotoba wo hakonde");
        assert_eq!(roman("東京の空は今日も青い"), "toukyou no sora wa kyou mo aoi");
        assert_eq!(roman("明日も会えるかな"), "ashita mo aeru ka na");
        assert_eq!(roman("何度でも立ち上がるよ"), "nando demo tachiagaru yo");
    }

    #[test]
    fn verb_endings_join_and_the_copula_stands_apart() {
        assert_eq!(roman("好きだった"), "suki datta");
        assert_eq!(roman("手紙を読んでいる"), "tegami wo yondeiru");
        assert_eq!(roman("笑って泣いた"), "waratte naita");
    }

    #[test]
    fn kana_latin_and_punctuation() {
        assert_eq!(dict().ruby("ありがとう、さよなら"), vec![rb("ありがとう、", "arigatou,"), rb("さよなら", "sayonara")]);
        let pieces = dict().ruby("OKだよ！ ロミオ");
        assert_eq!(pieces.first(), Some(&rb("OK", "")));
        assert_eq!(roman("OKだよ！ ロミオ"), "da yo! romio");
    }

    #[test]
    fn unknown_kanji_cost_only_their_own_reading() {
        // 㐂 isn't in the dictionary: it gets no reading and the rest still reads.
        let pieces = dict().ruby("夜の㐂が笑う");
        assert!(pieces.iter().any(|p| p.text.contains('㐂') && p.roman.is_empty()));
        assert_eq!(roman("夜の㐂が笑う"), "yoru no ga warau");
    }

    #[test]
    fn text_always_rebuilds_the_line() {
        let lines = ["  君と僕　2024年 ♪ Oh yeah!  ", "「ねえ」、どうして？ ｶﾞﾝﾊﾞﾚ", "愛してる ♡ 2024", "😀夜明け🌙", ""];
        for line in lines {
            let text: String = dict().ruby(line).iter().map(|p| p.text.as_str()).collect();
            assert_eq!(text, line);
        }
    }

    #[test]
    fn long_lines_are_read_in_chunks() {
        let line = "空を見上げて 歩いた ".repeat(60);
        let pieces = dict().ruby(&line);
        assert_eq!(pieces.iter().map(|p| p.text.as_str()).collect::<String>(), line);
        assert!(roman(&line).starts_with("sora wo miagete aruita"));
        let parts = chunks(&line, CHUNK_MAX);
        assert!(parts.len() > 1 && parts.iter().all(|c| c.chars().count() <= CHUNK_MAX));
        assert_eq!(parts.concat(), line);
        assert_eq!(chunks("abc def", 100), vec!["abc def"]);
        assert_eq!(chunks("aaaa bbbb cccc", 9), vec!["aaaa ", "bbbb cccc"]);
    }

    #[test]
    fn features_and_kana() {
        assert_eq!(feature("N,ヨル"), ('N', Some("ヨル")));
        assert_eq!(feature("P"), ('P', None));
        assert_eq!(hiragana("ヨンダ ー ヴ"), "よんだ ー ゔ");
    }
}
