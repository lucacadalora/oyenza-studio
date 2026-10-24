//! Kana to romaji (Hepburn, the way lyric sites write it).
//!
//! Long vowels stay as written (`おう` ou, `えい` ei), `ー` repeats the vowel
//! before it, `を` reads "wo", `ん` is "n'" before a vowel or y.

use super::push_ascii_punct;

/// Hiragana/katakana (full and half width) to romaji.
pub fn kana_to_romaji(kana: &str) -> String {
    let chars = normalize(kana);
    let mut out = String::with_capacity(kana.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            'っ' => {
                // Double the next consonant; dropped before a vowel or at the end.
                if let Some((next, _)) = syllable(&chars, i + 1) {
                    if next.starts_with("ch") {
                        out.push('t');
                    } else if let Some(first) = next.chars().next().filter(|c| !is_vowel(*c)) {
                        out.push(first);
                    }
                }
                i += 1;
            }
            'ん' => {
                out.push('n');
                if let Some((next, _)) = syllable(&chars, i + 1) {
                    if next.starts_with(|c: char| is_vowel(c) || c == 'y') {
                        out.push('\'');
                    }
                }
                i += 1;
            }
            'ー' => {
                if let Some(v) = out.chars().last().filter(|c| is_vowel(*c)) {
                    out.push(v);
                }
                i += 1;
            }
            _ => {
                if let Some((roman, len)) = syllable(&chars, i) {
                    out.push_str(&roman);
                    i += len;
                } else {
                    if c.is_whitespace() {
                        out.push(' ');
                    } else if super::is_han(c) || super::is_hangul(c) {
                        // Not readable here; the caller should not pass these.
                    } else if c.is_alphanumeric() {
                        out.extend(c.to_lowercase());
                    } else {
                        push_ascii_punct(&mut out, c);
                    }
                    i += 1;
                }
            }
        }
    }
    out
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o')
}

/// The syllable starting at `i` (a kana plus any small kana it combines with)
/// and how many chars it used. `None` for っ, ん, ー and non-kana.
fn syllable(chars: &[char], i: usize) -> Option<(String, usize)> {
    let c = *chars.get(i)?;
    let base = base_roman(c)?;
    if let Some(&small) = chars.get(i + 1) {
        if let Some(combo) = combine(c, base, small) {
            return Some((combo, 2));
        }
    }
    Some((base.to_string(), 1))
}

/// A kana followed by a small ゃゅょ or ぁぃぅぇぉ, when the pair has its own spelling.
fn combine(c: char, base: &str, small: char) -> Option<String> {
    let fixed = match (c, small) {
        ('ふ', 'ぁ') => "fa",
        ('ふ', 'ぃ') => "fi",
        ('ふ', 'ぇ') => "fe",
        ('ふ', 'ぉ') => "fo",
        ('ふ', 'ゃ') => "fya",
        ('ふ', 'ゅ') => "fyu",
        ('ふ', 'ょ') => "fyo",
        ('ゔ', 'ぁ') => "va",
        ('ゔ', 'ぃ') => "vi",
        ('ゔ', 'ぇ') => "ve",
        ('ゔ', 'ぉ') => "vo",
        ('ゔ', 'ゅ') => "vyu",
        ('て', 'ぃ') => "ti",
        ('て', 'ゅ') => "tyu",
        ('で', 'ぃ') => "di",
        ('で', 'ゅ') => "dyu",
        ('と', 'ぅ') => "tu",
        ('ど', 'ぅ') => "du",
        ('う', 'ぁ') => "wa",
        ('う', 'ぃ') => "wi",
        ('う', 'ぇ') => "we",
        ('う', 'ぉ') => "wo",
        ('つ', 'ぁ') => "tsa",
        ('つ', 'ぃ') => "tsi",
        ('つ', 'ぇ') => "tse",
        ('つ', 'ぉ') => "tso",
        ('く', 'ぁ') => "kwa",
        ('く', 'ぃ') => "kwi",
        ('く', 'ぇ') => "kwe",
        ('く', 'ぉ') => "kwo",
        ('ぐ', 'ぁ') => "gwa",
        ('す', 'ぃ') => "si",
        ('ず', 'ぃ') => "zi",
        ('い', 'ぇ') => "ye",
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(fixed.to_string());
    }
    // き + ゃ -> kya, し + ゃ -> sha, ち + ぇ -> che, ...
    let vowel = match small {
        'ゃ' => "a",
        'ゅ' => "u",
        'ょ' => "o",
        'ぇ' => "e",
        _ => return None,
    };
    let stem = base.strip_suffix('i').filter(|s| !s.is_empty())?;
    if matches!(stem, "sh" | "ch" | "j") {
        Some(format!("{stem}{vowel}"))
    } else {
        Some(format!("{stem}y{vowel}"))
    }
}

/// Romaji for one hiragana (after `normalize`). `None` for っ, ん, ー and non-kana.
fn base_roman(c: char) -> Option<&'static str> {
    const TABLE: [&str; 86] = [
        "a", "a", "i", "i", "u", "u", "e", "e", "o", "o", // ぁ-お
        "ka", "ga", "ki", "gi", "ku", "gu", "ke", "ge", "ko", "go", // か-ご
        "sa", "za", "shi", "ji", "su", "zu", "se", "ze", "so", "zo", // さ-ぞ
        "ta", "da", "chi", "ji", "", "tsu", "zu", "te", "de", "to", "do", // た-ど
        "na", "ni", "nu", "ne", "no", // な-の
        "ha", "ba", "pa", "hi", "bi", "pi", "fu", "bu", "pu", "he", "be", "pe", "ho", "bo",
        "po", // は-ぽ
        "ma", "mi", "mu", "me", "mo", // ま-も
        "ya", "ya", "yu", "yu", "yo", "yo", // ゃ-よ
        "ra", "ri", "ru", "re", "ro", // ら-ろ
        "wa", "wa", "wi", "we", "wo", "", "vu", "ka", "ke", // ゎ-ゖ
    ];
    match c {
        'ヷ' => Some("va"),
        'ヸ' => Some("vi"),
        'ヹ' => Some("ve"),
        'ヺ' => Some("vo"),
        'ゟ' => Some("yori"),
        'ヿ' => Some("koto"),
        '\u{3041}'..='\u{3096}' => {
            let r = TABLE[c as usize - 0x3041];
            (!r.is_empty()).then_some(r)
        }
        _ => None,
    }
}

/// Half-width katakana to full width, voicing marks merged into the kana,
/// iteration marks expanded, then katakana to hiragana (ー and ヷ-ヺ stay).
fn normalize(s: &str) -> Vec<char> {
    const HALF: &str = "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";
    // Small katakana for Ainu (U+31F0-U+31FF), read as the full-size kana.
    const AINU: &str = "クシストヌハヒフヘホムラリルレロ";
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            '\u{FF66}'..='\u{FF9D}' => HALF.chars().nth(c as usize - 0xFF66).unwrap_or(c),
            '\u{31F0}'..='\u{31FF}' => AINU.chars().nth(c as usize - 0x31F0).unwrap_or(c),
            '\u{FF9E}' | '\u{309B}' => '\u{3099}',
            '\u{FF9F}' | '\u{309C}' => '\u{309A}',
            _ => c,
        };
        match c {
            '\u{3099}' | '\u{309A}' => {
                if let Some(last) = out.last_mut() {
                    if let Some(v) = voiced(*last, c == '\u{309A}') {
                        *last = v;
                    }
                }
            }
            'ゝ' | 'ヽ' => {
                if let Some(&p) = out.last() {
                    out.push(p);
                }
            }
            'ゞ' | 'ヾ' => {
                if let Some(&p) = out.last() {
                    out.push(voiced(p, false).unwrap_or(p));
                }
            }
            _ => out.push(c),
        }
    }
    for c in &mut out {
        if let '\u{30A1}'..='\u{30F6}' = *c {
            *c = char::from_u32(*c as u32 - 0x60).unwrap_or(*c);
        }
    }
    out
}

/// The voiced (゛) or semi-voiced (゜) form of a kana, in the same script.
fn voiced(c: char, semi: bool) -> Option<char> {
    let kata = matches!(c, '\u{30A1}'..='\u{30F6}');
    let h = if kata { char::from_u32(c as u32 - 0x60)? } else { c };
    let v = match (h, semi) {
        (
            'か' | 'き' | 'く' | 'け' | 'こ' | 'さ' | 'し' | 'す' | 'せ' | 'そ' | 'た' | 'ち' | 'つ'
            | 'て' | 'と',
            false,
        ) => char::from_u32(h as u32 + 1)?,
        ('は' | 'ひ' | 'ふ' | 'へ' | 'ほ', _) => char::from_u32(h as u32 + 1 + semi as u32)?,
        ('う', false) => 'ゔ',
        ('わ', false) if kata => return Some('ヷ'),
        ('ゐ', false) if kata => return Some('ヸ'),
        ('ゑ', false) if kata => return Some('ヹ'),
        ('を', false) if kata => return Some('ヺ'),
        _ => return None,
    };
    if kata {
        char::from_u32(v as u32 + 0x60)
    } else {
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        kana_to_romaji(s)
    }

    #[test]
    fn basic_hepburn() {
        assert_eq!(r("ありがとう"), "arigatou");
        assert_eq!(r("さよなら"), "sayonara");
        assert_eq!(r("しちつふじ"), "shichitsufuji");
        assert_eq!(r("ちぢむ"), "chijimu");
        assert_eq!(r("つづく"), "tsuzuku");
        assert_eq!(r("ずっと"), "zutto");
        assert_eq!(r("を"), "wo");
        assert_eq!(r("えいが"), "eiga");
        assert_eq!(r("くうき"), "kuuki");
    }

    #[test]
    fn small_ya_yu_yo() {
        assert_eq!(r("きょう"), "kyou");
        assert_eq!(r("しゃしん"), "shashin");
        assert_eq!(r("ちゅう"), "chuu");
        assert_eq!(r("じゃあね"), "jaane");
        assert_eq!(r("ぢゃ"), "ja");
        assert_eq!(r("にゃ"), "nya");
        assert_eq!(r("りょこう"), "ryokou");
        assert_eq!(r("ぴゅ"), "pyu");
    }

    #[test]
    fn small_tsu_doubles() {
        assert_eq!(r("きって"), "kitte");
        assert_eq!(r("まっちゃ"), "matcha");
        assert_eq!(r("こっち"), "kotchi");
        assert_eq!(r("いっしょ"), "issho");
        assert_eq!(r("みっつ"), "mittsu");
        assert_eq!(r("あっ"), "a");
        assert_eq!(r("カップ"), "kappu");
    }

    #[test]
    fn n_and_apostrophe() {
        assert_eq!(r("しんぶん"), "shinbun");
        assert_eq!(r("れんあい"), "ren'ai");
        assert_eq!(r("こんや"), "kon'ya");
        assert_eq!(r("きんようび"), "kin'youbi");
        assert_eq!(r("みんな"), "minna");
        assert_eq!(r("ほん"), "hon");
    }

    #[test]
    fn long_vowel_mark() {
        assert_eq!(r("ラーメン"), "raamen");
        assert_eq!(r("ケーキ"), "keeki");
        assert_eq!(r("ねぇ"), "nee");
        assert_eq!(r("ンー"), "n");
    }

    #[test]
    fn katakana_extensions() {
        assert_eq!(r("ファン"), "fan");
        assert_eq!(r("フィルム"), "firumu");
        assert_eq!(r("ふぃるむ"), "firumu");
        assert_eq!(r("フェス"), "fesu");
        assert_eq!(r("フォト"), "foto");
        assert_eq!(r("パーティー"), "paatii");
        assert_eq!(r("ディスコ"), "disuko");
        assert_eq!(r("トゥ"), "tu");
        assert_eq!(r("ドゥ"), "du");
        assert_eq!(r("ウィ"), "wi");
        assert_eq!(r("ウェブ"), "webu");
        assert_eq!(r("ウォ"), "wo");
        assert_eq!(r("ヴ"), "vu");
        assert_eq!(r("ヴァ"), "va");
        assert_eq!(r("ヴィ"), "vi");
        assert_eq!(r("ヴェ"), "ve");
        assert_eq!(r("ヴォ"), "vo");
        assert_eq!(r("シェフ"), "shefu");
        assert_eq!(r("ジェット"), "jetto");
        assert_eq!(r("チェック"), "chekku");
        assert_eq!(r("ツァ"), "tsa");
        assert_eq!(r("イェ"), "ye");
    }

    #[test]
    fn half_width_and_marks() {
        assert_eq!(r("ﾗｰﾒﾝ"), "raamen");
        assert_eq!(r("ｶﾞｷﾞ"), "gagi");
        assert_eq!(r("ﾊﾟﾝ"), "pan");
        assert_eq!(r("ｳﾞｧ"), "va");
        assert_eq!(r("ｷｯﾄ"), "kitto");
        assert_eq!(r("か\u{3099}"), "ga");
        assert_eq!(r("ワ\u{3099}"), "va");
        assert_eq!(r("いすゞ"), "isuzu");
    }

    #[test]
    fn punctuation_and_spaces() {
        assert_eq!(r("ね、ねえ。"), "ne,nee.");
        assert_eq!(r("ほら！"), "hora!");
        assert_eq!(r("あ　い"), "a i");
    }
}
