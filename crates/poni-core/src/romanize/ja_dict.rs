//! Japanese readings from Oyenza Studio's own dictionary, for Macs and for PCs
//! where Windows' reading analyzer has no data (Windows 11 keeps it in the
//! Japanese typing feature, which most PCs outside Japan don't have).
//!
//! The dictionary is mecab-ipadic 2.7.0-20070801 trimmed for lyrics and
//! compiled for vibrato, a Rust port of MeCab's method: `data/ja-readings.dic.zst`
//! (3.8 MB packed, 14 MB loaded), built by `tools/make_ja_dict.py` and
//! downloaded as the Japanese readings add-on (`crate::addons`). Every copy
//! must carry mecab-ipadic's notice (THIRD-PARTY-NOTICES.txt). A line is split
//! into the cheapest chain of dictionary words, and the words go through the
//! same [`japanese_ruby`] as the analyzer's.
//!
//! Each entry's feature is a class letter, then its reading in katakana when
//! the word has kanji ("N,ヨル"): N content word, P particle, A auxiliary verb,
//! T te/ta ending, J suffix or dependent word, F prefix, S symbol, D number,
//! C counter. A number and its counter read as one word, with the sound
//! changes the dictionary leaves out (一歩 ippo, 三本 sanbon, 二人 futari).

use super::{is_han, is_kana, japanese_ruby, JaWord, Ruby};
use std::io::Read;
use vibrato::{Dictionary, Tokenizer};

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
    /// Load a dictionary packed by `tools/make_ja_dict.py` (the Japanese
    /// readings add-on's file; about 0.1 s in a release build).
    pub fn load(packed: &[u8]) -> Result<JaDict, String> {
        let fail = |e: &dyn std::fmt::Display| format!("couldn't load Oyenza Studio's Japanese reading dictionary ({e})");
        let mut raw = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(packed).map_err(|e| fail(&e))?.read_to_end(&mut raw).map_err(|e| fail(&e))?;
        let dict = Dictionary::read(&raw[..]).map_err(|e| fail(&e))?;
        Ok(JaDict { tokenizer: Tokenizer::new(dict) })
    }

    /// Each word of `line` and its reading in hiragana (empty for words written
    /// in kana and for kanji the dictionary doesn't know), after verb endings
    /// and counters are joined: what [`ruby`](Self::ruby) spells.
    pub fn readings(&self, line: &str) -> Vec<(String, String)> {
        self.words(line).into_iter().map(|w| (w.display, w.yomi)).collect()
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
    /// "yonda", not "yon da"), and counters joined to their number.
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
                let yomi = reading.map(hiragana).unwrap_or_default();
                let after_number = words.last().is_some_and(|w| w.class == 'D');
                if class == 'C' && after_number {
                    // A whole number on its own (二人), not the end of a longer one (十二人).
                    let alone = words.len() < 2 || words[words.len() - 2].class != 'D';
                    let number = words.last_mut().expect("after a number");
                    if let Some(both) = counted(&number.display, &number.yomi, surface, &yomi, alone) {
                        number.display.push_str(surface);
                        number.yomi = both;
                        number.class = 'N';
                        continue;
                    }
                }
                words.push(Word {
                    display: surface.to_string(),
                    yomi,
                    // Numbers join the number before them (十一 is one).
                    phrase_start: matches!(class, 'N' | 'F' | 'S' | 'D') && !after_prefix && !(class == 'D' && after_number),
                    class,
                });
                after_prefix = class == 'F';
            }
        }
        words
    }
}

/// Counters whose sound changes after a number, and how they read after さん,
/// なん, せん or まん when that changes too (三本 sanbon, 何階 nangai).
const COUNTERS: &[(&str, Option<&str>)] = &[
    ("回", None),
    ("階", Some("がい")),
    ("個", None),
    ("曲", None),
    ("軒", Some("げん")),
    ("歳", None),
    ("冊", None),
    ("週", None),
    ("足", Some("ぞく")),
    ("点", None),
    ("着", None),
    ("通", None),
    ("句", None),
    ("分", Some("ぷん")),
    ("本", Some("ぼん")),
    ("杯", Some("ばい")),
    ("匹", Some("びき")),
    ("歩", Some("ぽ")),
    ("泊", Some("ぱく")),
    ("発", Some("ぱつ")),
    ("拍", Some("ぱく")),
    ("遍", Some("べん")),
];

/// How a number and the counter after it read together, when that's more
/// than one reading after the other: 一人 hitori, 二人 futari, 四人 yonin,
/// and the sound changes of 一歩 ippo, 六回 rokkai, 三本 sanbon, 何分 nanpun.
/// None when nothing changes. `alone`: the number isn't the end of a longer
/// one (the 二 of 十二人 reads ni).
fn counted(number: &str, number_yomi: &str, counter: &str, counter_yomi: &str, alone: bool) -> Option<String> {
    if counter == "人" {
        return match number {
            "一" | "1" | "１" if alone => Some("ひとり".into()),
            "二" | "2" | "２" if alone => Some("ふたり".into()),
            "四" | "4" | "４" => Some("よにん".into()),
            _ => None,
        };
    }
    let (_, after_n) = COUNTERS.iter().find(|(c, _)| *c == counter)?;
    let first = counter_yomi.chars().next()?;
    let row = |chars: &str| chars.contains(first);
    let (k, s, t, h) = (row("かきくけこ"), row("さしすせそ"), row("たちつてと"), row("はひふへほ"));
    // いち, はち and じゅう end in a double consonant before k, s, t and h; ろく
    // and ひゃく only before k and h. After it, h becomes p.
    let doubled = if number_yomi.ends_with("いち") || number_yomi.ends_with("はち") || number_yomi.ends_with("じゅう") {
        k || s || t || h
    } else {
        (number_yomi.ends_with("ろく") || number_yomi.ends_with("ひゃく")) && (k || h)
    };
    if doubled {
        let mut both: String = number_yomi.chars().take(number_yomi.chars().count() - 1).collect();
        both.push('っ');
        both.push(if h { pa_row(first) } else { first });
        both.extend(counter_yomi.chars().skip(1));
        return Some(both);
    }
    let voiced = ["さん", "なん", "せん", "まん"].iter().any(|n| number_yomi.ends_with(n)) || (number_yomi.ends_with("よん") && counter == "分");
    after_n.filter(|_| voiced).map(|after| format!("{number_yomi}{after}"))
}

/// は ひ ふ へ ほ as ぱ ぴ ぷ ぺ ぽ.
fn pa_row(c: char) -> char {
    match c {
        'は' => 'ぱ',
        'ひ' => 'ぴ',
        'ふ' => 'ぷ',
        'へ' => 'ぺ',
        'ほ' => 'ぽ',
        _ => c,
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
    fn topic_particle_reads_wa() {
        // 本当は is one word in the dictionary.
        assert_eq!(roman("本当は嫌だ"), "hontou wa iya da");
        assert_eq!(roman("心には君がいる"), "kokoro ni wa kimi ga iru");
        assert_eq!(roman("光を照らすは月"), "hikari wo terasu wa tsuki");
    }

    #[test]
    fn verb_endings_join_and_the_copula_stands_apart() {
        assert_eq!(roman("好きだった"), "suki datta");
        assert_eq!(roman("手紙を読んでいる"), "tegami wo yondeiru");
        assert_eq!(roman("笑って泣いた"), "waratte naita");
    }

    #[test]
    fn numbers_and_counters_read_together() {
        assert_eq!(roman("二人で歩いた"), "futari de aruita");
        assert_eq!(roman("2人だけの秘密"), "futari dake no himitsu");
        assert_eq!(roman("一人で"), "hitori de");
        assert_eq!(roman("四人で"), "yonin de");
        // The 二 of 十二 isn't 二人 (futari).
        assert_eq!(roman("十二人"), "juuninin");
        assert_eq!(roman("一歩ずつ"), "ippozutsu");
        assert_eq!(roman("あと一回だけ"), "ato ikkai dake");
        assert_eq!(roman("一言一句"), "hitokoto ikku");
        assert_eq!(roman("三本の"), "sanbon no");
        assert_eq!(roman("三階"), "sangai");
        assert_eq!(roman("八杯"), "happai");
        assert_eq!(roman("何分か"), "nanpun ka");
        // Nothing changes here.
        assert_eq!(roman("六歳"), "rokusai");
        assert_eq!(roman("五分"), "gofun");
        assert_eq!(dict().readings("2人"), vec![("2人".to_string(), "ふたり".to_string())]);
    }

    #[test]
    fn words_from_the_fix_list() {
        assert_eq!(roman("愛おしい日々"), "itooshii hibi");
        assert_eq!(roman("失くした鍵"), "nakushita kagi");
        assert_eq!(roman("夜を彷徨う"), "yoru wo samayou");
        assert_eq!(roman("揺蕩う"), "tayutau");
        assert_eq!(roman("其処に"), "soko ni");
        assert_eq!(roman("貴女の声"), "anata no koe");
        assert_eq!(roman("違くて"), "chigakute");
        assert_eq!(roman("碧い海"), "aoi umi");
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
