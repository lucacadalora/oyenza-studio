//! Romanized "pronunciation" lines under Japanese, Korean and Chinese lyrics
//! (like Apple Music's "Show Pronunciation").
//!
//! Every function splits a line into [`Ruby`] pieces. Joining all `text`s
//! gives back the line exactly; joining the non-empty `roman`s with single
//! spaces gives the pronunciation line. Punctuation rides on the piece before
//! it (opening brackets on the piece after) and reads as ASCII in `roman`.
//! Japanese lines with kanji need readings from a morphological analyzer
//! (Windows' JapanesePhoneticAnalyzer), passed in as [`JaWord`]s.

mod hangul;
mod kana;
mod pinyin;
mod pinyin_data;

pub use kana::kana_to_romaji;

/// One piece of a lyric line and how it reads in Latin letters.
/// `roman` is empty for pieces that need none (Latin words, numbers, spaces).
#[derive(Clone, Debug, PartialEq)]
pub struct Ruby {
    pub text: String,
    pub roman: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Japanese,
    Korean,
    Chinese,
}

/// What a whole song needs: any kana -> Japanese (its kanji-only lines too); else any Hangul -> Korean;
/// else any Han -> Chinese; else None (nothing to romanize).
pub fn song_script(lines: &[&str]) -> Option<Script> {
    let (mut hangul, mut han) = (false, false);
    for c in lines.iter().flat_map(|l| l.chars()) {
        match class_of(c) {
            Class::Kana => return Some(Script::Japanese),
            Class::Hangul => hangul = true,
            Class::Han => han = true,
            _ => {}
        }
    }
    if hangul {
        Some(Script::Korean)
    } else if han {
        Some(Script::Chinese)
    } else {
        None
    }
}

/// The line has kana, Hangul or Han characters.
pub fn needs_roman(line: &str) -> bool {
    line.chars().any(|c| matches!(class_of(c), Class::Kana | Class::Hangul | Class::Han))
}

/// A word from a Japanese morphological analyzer (Windows' JapanesePhoneticAnalyzer gives
/// DisplayText, YomiText in hiragana, IsPhraseStart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JaWord<'a> {
    pub display: &'a str,
    pub yomi: &'a str,
    pub phrase_start: bool,
}

/// Group analyzer words into phrases (a new Ruby at each phrase_start; words inside a phrase are
/// joined by single spaces in `roman`). Particles: a word that is exactly は or へ and not a
/// phrase start reads "wa" / "e"; を reads "wo". Latin/digit words get empty roman.
///
/// Spaces, punctuation and Latin words also end a phrase (Latin words are pieces of their own).
pub fn japanese_ruby(words: &[JaWord]) -> Vec<Ruby> {
    let mut toks: Vec<Tok> = Vec::new();
    // The last token is a Japanese phrase that later words of the same phrase join.
    let mut open = false;
    for w in words {
        let d = w.display;
        if !d.chars().any(|c| matches!(class_of(c), Class::Kana | Class::Han)) {
            toks.extend(tokens(d, Script::Japanese));
            open = false;
            continue;
        }
        let roman = match d {
            "は" if !w.phrase_start => "wa".to_string(),
            "へ" if !w.phrase_start => "e".to_string(),
            _ if w.yomi.chars().any(|c| class_of(c) == Class::Kana) => kana_to_romaji(w.yomi),
            _ => kana_to_romaji(d),
        };
        match toks.last_mut() {
            Some(Tok::Word { text, roman: r, .. }) if open && !w.phrase_start => {
                text.push_str(d);
                if !r.is_empty() && !roman.is_empty() {
                    r.push(' ');
                }
                r.push_str(&roman);
            }
            _ => toks.push(Tok::Word { text: d.to_string(), roman, latin: false }),
        }
        open = true;
    }
    assemble(toks)
}

/// A Japanese line without kanji (kana, Latin, punctuation only), split at spaces and
/// punctuation; None if the line has any kanji (needs the analyzer).
pub fn japanese_kana_ruby(line: &str) -> Option<Vec<Ruby>> {
    if line.chars().any(|c| class_of(c) == Class::Han) {
        return None;
    }
    Some(assemble(tokens(line, Script::Japanese)))
}

/// One Ruby per space-separated word; roman = the word's syllables separated by single spaces.
///
/// Sound changes (liaison, nasalization, ㄹ) apply inside a word, never across a space.
/// Hanja in a Korean line get empty roman.
pub fn korean_ruby(line: &str) -> Vec<Ruby> {
    assemble(tokens(line, Script::Korean))
}

/// One Ruby per run of Han characters (split at spaces, punctuation and non-Han text);
/// roman = pinyin syllables with tone marks separated by single spaces.
pub fn chinese_ruby(line: &str) -> Vec<Ruby> {
    assemble(tokens(line, Script::Chinese))
}

/// What kind of text a character is, for splitting lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Kana,
    Hangul,
    Han,
    /// Latin letters, digits and other letters that need no romanizing.
    Other,
    Space,
    Punct,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if is_kana(c) {
        Class::Kana
    } else if is_hangul(c) {
        Class::Hangul
    } else if is_han(c) {
        Class::Han
    } else if c.is_alphanumeric() {
        Class::Other
    } else {
        Class::Punct
    }
}

/// Hiragana, katakana (full and half width) and their marks; not the middle dot ・.
fn is_kana(c: char) -> bool {
    matches!(c,
        '\u{3041}'..='\u{3096}' | '\u{3099}'..='\u{309F}' | '\u{30A1}'..='\u{30FA}'
        | '\u{30FC}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}')
}

fn is_hangul(c: char) -> bool {
    matches!(c,
        '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3131}'..='\u{318E}'
        | '\u{A960}'..='\u{A97F}' | '\u{D7B0}'..='\u{D7FF}')
}

/// CJK ideographs (all blocks) plus 々 〆 〇.
fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3005}'..='\u{3007}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FFFD}' | '\u{30000}'..='\u{3FFFD}')
}

/// Brackets and quotes that belong to the piece after them.
fn is_opening(c: char) -> bool {
    matches!(
        c,
        '「' | '『' | '（' | '(' | '[' | '{' | '［' | '｛' | '【' | '〔' | '〖' | '〈' | '《' | '“'
            | '‘' | '«' | '｢' | '〝'
    )
}

/// Append how punctuation reads in `roman`: full width to ASCII (、→ , 。→ .), CJK quotes and
/// brackets to ASCII ones, middle dots dropped, ASCII kept, other symbols (♪, emoji) dropped.
fn push_ascii_punct(out: &mut String, c: char) {
    let s = match c {
        '、' | '，' | '､' | '﹐' | '﹑' => ",",
        '。' | '．' | '｡' | '﹒' => ".",
        '「' | '」' | '『' | '』' | '“' | '”' | '„' | '〝' | '〞' | '〟' | '｢' | '｣' | '《' | '》'
        | '〈' | '〉' | '«' | '»' => "\"",
        '‘' | '’' | '‚' => "'",
        '【' | '〔' | '〖' | '〘' | '〚' => "[",
        '】' | '〕' | '〗' | '〙' | '〛' => "]",
        '…' | '⋯' => "...",
        '‥' => "..",
        '〜' | '～' | '〰' => "~",
        '—' | '–' | '―' | '‐' | '‑' | '‒' | '−' => "-",
        '・' | '･' | '·' | '•' => "",
        '\u{FF01}'..='\u{FF5E}' => {
            out.extend(char::from_u32(c as u32 - 0xFEE0));
            return;
        }
        _ if c.is_ascii() => {
            out.push(c);
            return;
        }
        _ => "",
    };
    out.push_str(s);
}

fn ascii_punct(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        push_ascii_punct(&mut out, c);
    }
    out
}

/// A line split into words, spaces and punctuation, before punctuation is attached.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// Text with its reading; Latin words have an empty one.
    Word { text: String, roman: String, latin: bool },
    Space(String),
    Punct(String),
}

impl Tok {
    fn text(&self) -> &str {
        match self {
            Tok::Word { text, .. } | Tok::Space(text) | Tok::Punct(text) => text,
        }
    }
}

/// Split `line` into runs of one class each and read the runs.
fn tokens(line: &str, script: Script) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut run = String::new();
    let mut run_class = Class::Space;
    for c in line.chars() {
        let class = class_of(c);
        if class != run_class && !run.is_empty() {
            toks.push(token(run_class, std::mem::take(&mut run), script));
        }
        run_class = class;
        run.push(c);
    }
    if !run.is_empty() {
        toks.push(token(run_class, run, script));
    }
    toks
}

fn token(class: Class, text: String, script: Script) -> Tok {
    let roman = match class {
        Class::Space => return Tok::Space(text),
        Class::Punct => return Tok::Punct(text),
        Class::Other => return Tok::Word { text, roman: String::new(), latin: true },
        Class::Kana => kana_to_romaji(&text),
        Class::Hangul => hangul::word_roman(&text),
        Class::Han if script == Script::Chinese => pinyin::run_roman(&text),
        Class::Han => String::new(),
    };
    Tok::Word { text, roman, latin: false }
}

/// Merge Latin runs ("Oh, oh" is one piece), attach punctuation, build the pieces.
fn assemble(toks: Vec<Tok>) -> Vec<Ruby> {
    let toks = merge(toks);
    let mut out: Vec<Ruby> = Vec::with_capacity(toks.len());
    // Opening punctuation waiting for the word after it.
    let mut prefix = String::new();
    let mut after_word = false;
    for (i, tok) in toks.iter().enumerate() {
        match tok {
            Tok::Word { text, roman, .. } => {
                let mut roman = roman.clone();
                if !roman.is_empty() && !prefix.is_empty() {
                    roman.insert_str(0, &ascii_punct(&prefix));
                }
                out.push(Ruby { text: std::mem::take(&mut prefix) + text, roman });
                after_word = true;
            }
            Tok::Space(s) => {
                out.push(Ruby { text: s.clone(), roman: String::new() });
                after_word = false;
            }
            Tok::Punct(p) => {
                let before_word = matches!(toks.get(i + 1), Some(Tok::Word { .. }));
                let split = match (after_word, before_word) {
                    (true, true) => p.find(is_opening).unwrap_or(p.len()),
                    (true, false) => p.len(),
                    (false, _) => 0,
                };
                let (tail, head) = p.split_at(split);
                if let Some(last) = out.last_mut().filter(|_| !tail.is_empty()) {
                    last.text.push_str(tail);
                    if !last.roman.is_empty() {
                        last.roman.push_str(&ascii_punct(tail));
                    }
                }
                if before_word {
                    prefix.push_str(head);
                } else if !head.is_empty() {
                    out.push(Ruby { text: head.to_string(), roman: ascii_punct(head) });
                    after_word = false;
                }
            }
        }
    }
    out
}

/// Join neighbouring punctuation, and Latin words with only spaces or
/// punctuation between them.
fn merge(toks: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(toks.len());
    for tok in toks {
        if let (Some(Tok::Punct(prev)), Tok::Punct(p)) = (out.last_mut(), &tok) {
            prev.push_str(p);
            continue;
        }
        if let Tok::Word { latin: true, text, .. } = &tok {
            let start = out.iter().rposition(|t| matches!(t, Tok::Word { .. }));
            if let Some(j) = start.filter(|&j| matches!(out[j], Tok::Word { latin: true, .. })) {
                let between: String = out.drain(j + 1..).map(|t| t.text().to_string()).collect();
                if let Some(Tok::Word { text: joined, .. }) = out.last_mut() {
                    joined.push_str(&between);
                    joined.push_str(text);
                }
                continue;
            }
        }
        out.push(tok);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixtures are original phrases, not real song lyrics.

    fn rb(text: &str, roman: &str) -> Ruby {
        Ruby { text: text.into(), roman: roman.into() }
    }

    fn w<'a>(display: &'a str, yomi: &'a str, phrase_start: bool) -> JaWord<'a> {
        JaWord { display, yomi, phrase_start }
    }

    fn joined(r: &[Ruby]) -> (String, String) {
        let text = r.iter().map(|p| p.text.as_str()).collect();
        let roman: Vec<&str> =
            r.iter().map(|p| p.roman.as_str()).filter(|s| !s.is_empty()).collect();
        (text, roman.join(" "))
    }

    #[test]
    fn script_of_a_song() {
        assert_eq!(song_script(&["愛", "こんにちは"]), Some(Script::Japanese));
        assert_eq!(song_script(&["愛", "ラーメン"]), Some(Script::Japanese));
        assert_eq!(song_script(&["사랑해", "愛"]), Some(Script::Korean));
        assert_eq!(song_script(&["我爱你", "Oh baby"]), Some(Script::Chinese));
        assert_eq!(song_script(&["麥可・傑克森"]), Some(Script::Chinese));
        assert_eq!(song_script(&["hello", "2024", ""]), None);
        assert_eq!(song_script(&[]), None);
    }

    #[test]
    fn which_lines_need_roman() {
        assert!(needs_roman("ねえ"));
        assert!(needs_roman("ｶﾞ"));
        assert!(needs_roman("Oh 사랑"));
        assert!(needs_roman("月"));
        assert!(!needs_roman("Oh, oh 2024!"));
        assert!(!needs_roman("・…♪"));
    }

    #[test]
    fn kana_line_pieces() {
        assert_eq!(japanese_kana_ruby("ラーメン食べる"), None);
        assert_eq!(
            japanese_kana_ruby("ありがとう、さよなら"),
            Some(vec![rb("ありがとう、", "arigatou,"), rb("さよなら", "sayonara")])
        );
        assert_eq!(
            japanese_kana_ruby("ねえ「さよなら」と"),
            Some(vec![rb("ねえ", "nee"), rb("「さよなら」", "\"sayonara\""), rb("と", "to")])
        );
        assert_eq!(
            japanese_kana_ruby("OKだよ！ ロミオ・ジュリエット"),
            Some(vec![
                rb("OK", ""),
                rb("だよ！", "dayo!"),
                rb(" ", ""),
                rb("ロミオ・", "romio"),
                rb("ジュリエット", "jurietto"),
            ])
        );
    }

    #[test]
    fn analyzer_words_become_phrases() {
        let words = [
            w("私", "わたし", true),
            w("は", "は", false),
            w("学校", "がっこう", true),
            w("へ", "へ", false),
            w("行く", "いく", true),
        ];
        assert_eq!(
            japanese_ruby(&words),
            vec![rb("私は", "watashi wa"), rb("学校へ", "gakkou e"), rb("行く", "iku")]
        );
        let words = [w("本", "ほん", true), w("を", "を", false), w("読む", "よむ", true)];
        assert_eq!(japanese_ruby(&words), vec![rb("本を", "hon wo"), rb("読む", "yomu")]);
        // は at a phrase start is not the particle.
        let words = [w("は", "は", true), w("ははは", "ははは", false)];
        assert_eq!(japanese_ruby(&words), vec![rb("はははは", "ha hahaha")]);
    }

    #[test]
    fn analyzer_latin_space_and_punctuation() {
        let words = [
            w("今日", "きょう", true),
            w("は", "は", false),
            w("OK", "OK", false),
            w("だ", "だ", false),
            w("よ", "よ", false),
            w("、", "、", false),
            w(" ", " ", false),
            w("フィルム", "ふぃるむ", true),
            w("2024", "2024", true),
        ];
        assert_eq!(
            japanese_ruby(&words),
            vec![
                rb("今日は", "kyou wa"),
                rb("OK", ""),
                rb("だよ、", "da yo,"),
                rb(" ", ""),
                rb("フィルム", "firumu"),
                rb("2024", ""),
            ]
        );
        // Latin words with spaces between them stay one piece.
        let words = [w("Oh", "Oh", true), w(" ", " ", false), w("baby", "baby", true)];
        assert_eq!(japanese_ruby(&words), vec![rb("Oh baby", "")]);
    }

    #[test]
    fn korean_line_pieces() {
        assert_eq!(
            korean_ruby("사랑해, 너를 Oh, oh"),
            vec![
                rb("사랑해,", "sa rang hae,"),
                rb(" ", ""),
                rb("너를", "neo reul"),
                rb(" ", ""),
                rb("Oh, oh", ""),
            ]
        );
        // No liaison across a space.
        assert_eq!(korean_ruby("옷 입어"), vec![rb("옷", "ot"), rb(" ", ""), rb("입어", "i beo")]);
        assert_eq!(korean_ruby("2024년"), vec![rb("2024", ""), rb("년", "nyeon")]);
    }

    #[test]
    fn chinese_line_pieces() {
        assert_eq!(chinese_ruby("我爱你"), vec![rb("我爱你", "wǒ ài nǐ")]);
        assert_eq!(
            chinese_ruby("你好，世界！"),
            vec![rb("你好，", "nǐ hǎo,"), rb("世界！", "shì jiè!")]
        );
        assert_eq!(
            chinese_ruby("OK 我们 2024年"),
            vec![
                rb("OK", ""),
                rb(" ", ""),
                rb("我们", "wǒ men"),
                rb(" ", ""),
                rb("2024", ""),
                rb("年", "nián"),
            ]
        );
        assert_eq!(chinese_ruby("《月光》"), vec![rb("《月光》", "\"yuè guāng\"")]);
    }

    #[test]
    fn punctuation_with_spaces_around() {
        assert_eq!(
            korean_ruby("♪ 사랑 - 해"),
            vec![
                rb("♪", ""),
                rb(" ", ""),
                rb("사랑", "sa rang"),
                rb(" ", ""),
                rb("-", "-"),
                rb(" ", ""),
                rb("해", "hae"),
            ]
        );
        assert_eq!(korean_ruby("(사랑)"), vec![rb("(사랑)", "(sa rang)")]);
        assert_eq!(korean_ruby("…"), vec![rb("…", "...")]);
        assert_eq!(korean_ruby(""), vec![]);
    }

    #[test]
    fn text_always_rebuilds_the_line() {
        let lines = [
            "  사랑해,  너를…  ",
            "Oh, oh 我爱你！ (yeah)",
            "「ねえ」、どうして？ ｶﾞﾝﾊﾞﾚ",
            "愛してる ♡ 2024",
            "",
        ];
        for line in lines {
            for pieces in [korean_ruby(line), chinese_ruby(line)] {
                assert_eq!(joined(&pieces).0, line);
            }
            if let Some(pieces) = japanese_kana_ruby(line) {
                assert_eq!(joined(&pieces).0, line);
            }
        }
        let (_, roman) = joined(&chinese_ruby("Oh, oh 我爱你！ (yeah)"));
        assert_eq!(roman, "wǒ ài nǐ!");
        let (_, roman) = joined(&japanese_kana_ruby("「ねえ」、どうして？ ｶﾞﾝﾊﾞﾚ").unwrap());
        assert_eq!(roman, "\"nee\", doushite? ganbare");
    }
}
