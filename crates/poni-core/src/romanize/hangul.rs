//! Korean in Revised Romanization, the way Apple Music shows it: one
//! syllable at a time, separated by spaces, with the sound changes inside a
//! word (liaison, nasalization, ㄹ assimilation, 같이 -> ga chi) but no ㅎ
//! aspiration (못해 stays "mot hae").

const INITIAL: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
const VOWEL: [&str; 21] =
    ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
/// Compatibility jamo consonants (ㄱ-ㅎ, U+3131-U+314E) written alone, as in ㅋㅋ.
const LONE_CONSONANT: [&str; 30] = [
    "k", "kk", "ks", "n", "nj", "nh", "t", "tt", "l", "lk", "lm", "lb", "ls", "lt", "lp", "lh", "m", "p", "pp", "ps", "s", "ss", "ng", "j", "jj", "ch",
    "k", "t", "p", "h",
];

// Initial consonant indexes used by the rules.
const I_N: usize = 2; // ㄴ
const I_R: usize = 5; // ㄹ
const I_M: usize = 6; // ㅁ
const I_SILENT: usize = 11; // ㅇ
const V_I: usize = 20; // ㅣ

/// How a final consonant sounds before another consonant or at the end of a word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Coda {
    K,
    T,
    P,
    N,
    L,
    M,
    Ng,
}

impl Coda {
    fn roman(self) -> &'static str {
        match self {
            Coda::K => "k",
            Coda::T => "t",
            Coda::P => "p",
            Coda::N => "n",
            Coda::L => "l",
            Coda::M => "m",
            Coda::Ng => "ng",
        }
    }

    /// k/t/p before ㄴ, ㅁ (or ㄹ) become ng/n/m.
    fn nasal(self) -> Coda {
        match self {
            Coda::K => Coda::Ng,
            Coda::T => Coda::N,
            Coda::P => Coda::M,
            other => other,
        }
    }
}

/// Final consonant index (1..=27) to its sound: ㄷㅅㅆㅈㅊㅌㅎ -> t, ㄱㄲㅋ -> k, ㅂㅍ -> p;
/// double finals keep one of their two.
fn coda(t: usize) -> Coda {
    const CODA: [Coda; 28] = [
        Coda::T, // (no final)
        Coda::K, Coda::K, Coda::K, // ㄱ ㄲ ㄳ
        Coda::N, Coda::N, Coda::N, // ㄴ ㄵ ㄶ
        Coda::T, // ㄷ
        Coda::L, // ㄹ
        Coda::K, Coda::M, Coda::L, Coda::L, Coda::L, Coda::P, Coda::L, // ㄺ ㄻ ㄼ ㄽ ㄾ ㄿ ㅀ
        Coda::M, // ㅁ
        Coda::P, Coda::P, // ㅂ ㅄ
        Coda::T, Coda::T, // ㅅ ㅆ
        Coda::Ng, // ㅇ
        Coda::T, Coda::T, // ㅈ ㅊ
        Coda::K, Coda::T, Coda::P, Coda::T, // ㅋ ㅌ ㅍ ㅎ
    ];
    CODA[t]
}

/// Before a vowel (silent ㅇ): what stays as the final and which initial moves on.
/// ㅇ never moves; ㅎ disappears (좋아 -> jo a); double finals split (닭이 -> dal gi).
fn liaison(t: usize) -> (&'static str, Option<&'static str>) {
    match t {
        1 => ("", Some("g")),
        2 => ("", Some("kk")),
        3 => ("k", Some("s")),
        4 => ("", Some("n")),
        5 => ("n", Some("j")),
        6 => ("", Some("n")),
        7 => ("", Some("d")),
        8 => ("", Some("r")),
        9 => ("l", Some("g")),
        10 => ("l", Some("m")),
        11 => ("l", Some("b")),
        12 => ("l", Some("s")),
        13 => ("l", Some("t")),
        14 => ("l", Some("p")),
        15 => ("", Some("r")),
        16 => ("", Some("m")),
        17 => ("", Some("b")),
        18 => ("p", Some("s")),
        19 => ("", Some("s")),
        20 => ("", Some("ss")),
        21 => ("ng", None),
        22 => ("", Some("j")),
        23 => ("", Some("ch")),
        24 => ("", Some("k")),
        25 => ("", Some("t")),
        26 => ("", Some("p")),
        _ => ("", None),
    }
}

#[derive(Clone, Copy)]
enum Unit {
    /// Initial, vowel and final indexes of a syllable (final 0 = none).
    Syllable(usize, usize, usize),
    /// A lone jamo (ㅋ, ㅠ) or one we can't read: written as is, no sound changes.
    Lone(&'static str),
}

/// Romanize one word (a run of Hangul): syllables separated by single spaces.
pub(super) fn word_roman(word: &str) -> String {
    let units = units(word);
    let mut parts: Vec<String> = Vec::with_capacity(units.len());
    // The initial a final consonant moved onto (or changed) for the next syllable.
    let mut carried: Option<&'static str> = None;
    for (i, unit) in units.iter().enumerate() {
        let (l, v, t) = match *unit {
            Unit::Syllable(l, v, t) => (l, v, t),
            Unit::Lone(s) => {
                carried = None;
                if !s.is_empty() {
                    parts.push(s.to_string());
                }
                continue;
            }
        };
        let initial = carried.take().unwrap_or(INITIAL[l]);
        let next = match units.get(i + 1) {
            Some(&Unit::Syllable(nl, nv, _)) => Some((nl, nv)),
            _ => None,
        };
        let fin = match (t, next) {
            (0, _) => "",
            (_, None) => coda(t).roman(),
            (_, Some((I_SILENT, nv))) => {
                let (stay, moved) = liaison(t);
                // ㄷ, ㅌ before 이 become j, ch (같이 -> ga chi).
                carried = match moved {
                    Some("d") if nv == V_I => Some("j"),
                    Some("t") if nv == V_I => Some("ch"),
                    other => other,
                };
                stay
            }
            (_, Some((nl, _))) => {
                let mut c = coda(t);
                if t == 9 && nl == 0 {
                    c = Coda::L; // ㄺ before ㄱ: 읽고 -> il go
                }
                match (c, nl) {
                    (_, I_N | I_M) => {
                        c = c.nasal();
                        if c == Coda::L && nl == I_N {
                            carried = Some("l"); // 설날 -> seol lal
                        }
                    }
                    (Coda::L, I_R) => carried = Some("l"), // 빨리 -> ppal li
                    (Coda::N, I_R) => {
                        c = Coda::L; // 신라 -> sil la
                        carried = Some("l");
                    }
                    (_, I_R) => {
                        c = c.nasal(); // 종로 -> jong no, 국립 -> gung nip
                        carried = Some("n");
                    }
                    _ => {}
                }
                c.roman()
            }
        };
        parts.push(format!("{initial}{}{fin}", VOWEL[v]));
    }
    parts.join(" ")
}

/// Syllables (precomposed, or composed from conjoining jamo) and lone jamo.
fn units(word: &str) -> Vec<Unit> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i] as usize;
        i += 1;
        match c {
            0xAC00..=0xD7A3 => {
                let s = c - 0xAC00;
                out.push(Unit::Syllable(s / 588, s % 588 / 28, s % 28));
            }
            0x1100..=0x1112 if matches!(chars.get(i), Some(&v) if ('\u{1161}'..='\u{1175}').contains(&v)) => {
                let v = chars[i] as usize - 0x1161;
                i += 1;
                let mut t = 0;
                if let Some(&f) = chars.get(i).filter(|f| ('\u{11A8}'..='\u{11C2}').contains(*f)) {
                    t = f as usize - 0x11A7;
                    i += 1;
                }
                out.push(Unit::Syllable(c - 0x1100, v, t));
            }
            0x3131..=0x314E => out.push(Unit::Lone(LONE_CONSONANT[c - 0x3131])),
            0x314F..=0x3163 => out.push(Unit::Lone(VOWEL[c - 0x314F])),
            _ => out.push(Unit::Lone("")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(pairs: &[(&str, &str)]) {
        for (word, roman) in pairs {
            assert_eq!(word_roman(word), *roman, "{word}");
        }
    }

    // Single words, grouped by the rule they show. The first ones are pairs
    // read off Apple Music's screen.
    #[test]
    fn apple_music_pairs() {
        check(&[
            ("이", "i"),
            ("잘", "jal"),
            ("둬", "dwo"),
            ("나를", "na reul"),
            ("순간", "sun gan"),
            ("머리가", "meo ri ga"),
            ("내버려", "nae beo ryeo"),
            ("아름다워", "a reum da wo"),
            ("나른해진", "na reun hae jin"),
            ("빗은", "bi seun"),
            ("벗어나진", "beo seo na jin"),
            ("헝클어질수록", "heong keu reo jil su rok"),
            ("못해", "mot hae"),
        ]);
    }

    #[test]
    fn liaison() {
        check(&[
            ("읽어", "il geo"),
            ("없어", "eop seo"),
            ("닭이", "dal gi"),
            ("좋아", "jo a"),
            ("많아", "ma na"),
            ("싫어", "si reo"),
            ("강아지", "gang a ji"),
            ("있어", "i sseo"),
            ("같이", "ga chi"),
            ("굳이", "gu ji"),
            ("밭을", "ba teul"),
        ]);
    }

    #[test]
    fn nasalization_and_l() {
        check(&[
            ("감사합니다", "gam sa ham ni da"),
            ("국물", "gung mul"),
            ("있는", "in neun"),
            ("좋네", "jon ne"),
            ("사랑해", "sa rang hae"),
            ("빨리", "ppal li"),
            ("신라", "sil la"),
            ("설날", "seol lal"),
            ("종로", "jong no"),
            ("국립", "gung nip"),
            ("읽고", "il go"),
        ]);
    }

    #[test]
    fn word_endings_and_lone_jamo() {
        check(&[("닭", "dak"), ("값", "gap"), ("부엌", "bu eok"), ("꽃", "kkot"), ("ㅋㅋ", "k k"), ("ㅠㅠ", "yu yu")]);
        // Conjoining jamo (decomposed text) read the same as precomposed syllables.
        assert_eq!(word_roman("\u{1109}\u{1161}\u{1105}\u{1161}\u{11BC}"), "sa rang");
    }
}
