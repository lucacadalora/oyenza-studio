//! Romanized "pronunciation" lines under Japanese, Korean and Chinese lyrics
//! (like Apple Music's "Show Pronunciation").
//!
//! Every function splits a line into [`Ruby`] pieces. Joining all `text`s
//! gives back the line exactly; joining the non-empty `roman`s with single
//! spaces gives the pronunciation line. Punctuation rides on the piece before
//! it (opening brackets on the piece after) and reads as ASCII in `roman`.
//! Japanese lines with kanji need readings from a morphological analyzer:
//! Windows' JapanesePhoneticAnalyzer, passed in as [`JaWord`]s, or, where
//! Windows has no Japanese data, Oyenza Studio's own dictionary ([`JaDict`]).

mod hangul;
mod ja_dict;
mod kana;
mod pinyin;
mod pinyin_data;

pub use ja_dict::JaDict;
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

/// A line's pieces as one line of Latin letters, the way it is sung: each
/// piece's reading, and the words that need none (Latin words, numbers, a
/// kanji left unread) as they are written, one space apart. `None` when no
/// piece has a reading: the line reads as it is.
pub fn reading(pieces: &[Ruby]) -> Option<String> {
    if pieces.iter().all(|p| p.roman.is_empty()) {
        return None;
    }
    let mut out = String::new();
    for p in pieces {
        let said = if p.roman.is_empty() { &p.text } else { &p.roman };
        for word in said.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    Some(out)
}

/// A word from a Japanese morphological analyzer (Windows' JapanesePhoneticAnalyzer gives
/// DisplayText, YomiText in hiragana, IsPhraseStart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JaWord<'a> {
    pub display: &'a str,
    pub yomi: &'a str,
    pub phrase_start: bool,
}

/// Group analyzer words into phrases (a new Ruby at each phrase_start). Inside a phrase a
/// particle or the copula stands apart after a space ("asa wo", "suki da") and every other
/// word joins the one before it, the way lyric romaji writes verb endings ("hakonde",
/// "itta"). へ that isn't a phrase start reads "e"; を reads "wo". は reads "wa" when it
/// follows a word, also where the analyzer starts a phrase with it (after a verb: "terasu
/// wa") and where it ends a word (には "ni wa", 本当は "hontou wa", see [`split_topic`]).
/// Latin/digit words get empty roman, and so do kanji the analyzer gave no kana reading
/// (an empty `yomi`): the rest of the line still reads.
///
/// Spaces, punctuation, Latin and unread words also end a phrase (they are pieces of their own).
pub fn japanese_ruby(words: &[JaWord]) -> Vec<Ruby> {
    let words = split_topic(words);
    let mut toks: Vec<Tok> = Vec::new();
    // The last token is a Japanese phrase that later words of the same phrase join.
    let mut open = false;
    // The kana of the phrase's last word with the endings joined to it, and where its
    // romaji starts in the token's reading: spelled again as one run, so that a っ
    // ending one piece doubles the next one's consonant (なかっ|た -> nakatta).
    let (mut run, mut run_at) = (String::new(), 0usize);
    for w in &words {
        let d = w.display;
        if !d.chars().any(|c| matches!(class_of(c), Class::Kana | Class::Han)) {
            toks.extend(tokens(d, Script::Japanese));
            open = false;
            continue;
        }
        let kana = if w.yomi.chars().any(|c| class_of(c) == Class::Kana) { w.yomi } else { d };
        if kana.chars().any(is_han) {
            toks.push(Tok::Word { text: d.to_string(), roman: String::new(), latin: false });
            open = false;
            continue;
        }
        // は right after a word is the topic particle, even where the analyzer
        // starts a phrase with it.
        let joins = !w.phrase_start || (d == "は" && open);
        let fixed = match d {
            "は" if joins => Some("wa"),
            "へ" if !w.phrase_start => Some("e"),
            _ => None,
        };
        match toks.last_mut() {
            Some(Tok::Word { text, roman: r, .. }) if open && joins => {
                text.push_str(d);
                if fixed.is_some() || PARTICLES.contains(&d) {
                    if !r.is_empty() {
                        r.push(' ');
                    }
                    run_at = r.len();
                    run = kana.to_string();
                    r.push_str(fixed.map_or_else(|| kana_to_romaji(kana), str::to_string).as_str());
                } else {
                    run.push_str(kana);
                    r.truncate(run_at);
                    r.push_str(&kana_to_romaji(&run));
                }
            }
            _ => {
                run = kana.to_string();
                run_at = 0;
                let roman = fixed.map_or_else(|| kana_to_romaji(kana), str::to_string);
                toks.push(Tok::Word { text: d.to_string(), roman, latin: false });
            }
        }
        open = true;
    }
    assemble(toks)
}

/// Particles that end in the topic particle は, which analyzers can hand back as one word.
const TOPIC_PARTICLES: &[&str] = &["には", "では", "とは", "へは", "からは", "までは", "よりは", "のは", "ては"];

/// `words` with a final topic particle は split off where it ends a word: words that end
/// in a particle like には or では (Windows' analyzer returns them whole, and それでは too)
/// and words with kanji like 本当は (one word in the dictionaries). The は then reads "wa"
/// like any other.
fn split_topic<'a>(words: &[JaWord<'a>]) -> Vec<JaWord<'a>> {
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let head = w.display.strip_suffix('は').filter(|h| !h.is_empty());
        let topic = head.filter(|h| TOPIC_PARTICLES.iter().any(|p| w.display.ends_with(p)) || h.chars().any(is_han));
        // The reading ends with は too (or is empty: the dictionary leaves kana words without one).
        let head_yomi = if w.yomi.is_empty() { Some("") } else { w.yomi.strip_suffix('は') };
        match (topic, head_yomi) {
            (Some(head), Some(head_yomi)) => {
                out.push(JaWord { display: head, yomi: head_yomi, phrase_start: w.phrase_start });
                out.push(JaWord { display: "は", yomi: "は", phrase_start: false });
            }
            _ => out.push(*w),
        }
    }
    out
}

/// Words lyric romaji writes on their own inside a phrase: particles and the copula.
/// Windows' analyzer also splits verb endings off (運|んで, 行|った, 浮かべ|た); those join.
const PARTICLES: &[&str] = &[
    "は", "が", "を", "に", "へ", "と", "で", "も", "の", "や", "か", "ね", "よ", "な", "さ", "ぞ", "ぜ", "わ", "から", "まで", "より", "なら", "けど",
    "けれど", "だけ", "しか", "ほど", "など", "ばかり", "こそ", "でも", "のに", "ので", "とか", "かな", "かも", "よね", "だ", "だっ", "です", "でし",
];

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
    fn a_line_reads_as_one_line_of_latin_letters() {
        // The pieces of 今日はOKだよ、 フィルム2024 (as the analyzer test below reads them).
        let line = [rb("今日は", "kyou wa"), rb("OK", ""), rb("だよ、", "da yo,"), rb(" ", ""), rb("フィルム", "firumu"), rb("2024", "")];
        assert_eq!(reading(&line).as_deref(), Some("kyou wa OK da yo, firumu 2024"));
        // Latin runs keep their own spacing, ideographic spaces fold into one.
        let line = [rb("Oh  baby", ""), rb("\u{3000}", ""), rb("사랑해", "saranghae")];
        assert_eq!(reading(&line).as_deref(), Some("Oh baby saranghae"));
        // A kanji the reader couldn't read stays as it is written.
        assert_eq!(reading(&[rb("㐂", ""), rb("びの", "bino")]).as_deref(), Some("㐂 bino"));
        // Nothing to read: the line is its own reading.
        assert_eq!(reading(&[rb("OK", ""), rb(" ", "")]), None);
        assert_eq!(reading(&[]), None);
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
        // How Windows' analyzer really splits verbs (seen on a GitHub Windows runner):
        // endings join their verb, particles stand apart.
        let words = [
            w("言え", "いえ", true),
            w("なかっ", "なかっ", false),
            w("た", "た", false),
            w("言葉", "ことば", true),
            w("を", "を", false),
            w("運", "はこ", true),
            w("んで", "んで", false),
        ];
        assert_eq!(joined(&japanese_ruby(&words)).1, "ienakatta kotoba wo hakonde");
        let words = [w("遠く", "とおく", true), w("へ", "へ", false), w("行", "い", true), w("った", "った", false)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "tooku e itta");
        let words = [w("笑っ", "わらっ", true), w("た", "た", false), w("ん", "ん", false), w("なら", "なら", false)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "warattan nara");
        let words = [w("好き", "すき", true), w("だっ", "だっ", false), w("た", "た", false)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "suki datta");
        let words = [w("本", "ほん", true), w("を", "を", false), w("読む", "よむ", true)];
        assert_eq!(japanese_ruby(&words), vec![rb("本を", "hon wo"), rb("読む", "yomu")]);
        // は at a phrase start is not the particle.
        let words = [w("は", "は", true), w("ははは", "ははは", false)];
        assert_eq!(japanese_ruby(&words), vec![rb("はははは", "hahahaha")]);
    }

    #[test]
    fn topic_particle_reads_wa() {
        // Windows' analyzer hands back には whole, and starts a phrase with は after a verb.
        let words = [w("心", "こころ", true), w("には", "には", false), w("君", "きみ", true), w("が", "が", false)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "kokoro ni wa kimi ga");
        let words = [w("照らす", "てらす", true), w("は", "は", true), w("月", "つき", true)];
        assert_eq!(japanese_ruby(&words), vec![rb("照らすは", "terasu wa"), rb("月", "tsuki")]);
        let words = [w("それ", "それ", true), w("では", "では", false), w("また", "また", true)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "sore de wa mata");
        // Windows hands back それでは whole (seen on Windows CI).
        let words = [w("それでは", "それでは", true), w("また", "また", true)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "sorede wa mata");
        let words = [w("して", "して", true), w("は", "は", false), w("いけない", "いけない", true)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "shite wa ikenai");
        // A word ending in the particle (本当は, one word in the dictionaries).
        assert_eq!(japanese_ruby(&[w("本当は", "ほんとうは", true)]), vec![rb("本当は", "hontou wa")]);
        // Oyenza Studio's dictionary gives kana words no reading: the display is the kana.
        let words = [w("時", "とき", true), w("には", "", false)];
        assert_eq!(joined(&japanese_ruby(&words)).1, "toki ni wa");
        // は starting a line, and words that only end in は, stay as they are.
        assert_eq!(japanese_ruby(&[w("は", "は", true), w("ははは", "ははは", false)]), vec![rb("はははは", "hahahaha")]);
        assert_eq!(japanese_ruby(&[w("はは", "はは", true)]), vec![rb("はは", "haha")]);
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
    fn unread_kanji_cost_only_their_own_reading() {
        // The analyzer knew every word but 魑魅 (empty yomi): it stands alone
        // without a reading and the rest of the line still reads.
        let words = [w("夜", "よる", true), w("の", "の", false), w("魑魅", "", true), w("が", "が", false), w("笑う", "わらう", true)];
        assert_eq!(japanese_ruby(&words), vec![rb("夜の", "yoru no"), rb("魑魅", ""), rb("が", "ga"), rb("笑う", "warau")]);
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
