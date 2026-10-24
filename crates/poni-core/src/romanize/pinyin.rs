//! Chinese characters to pinyin with tone marks, one syllable per character,
//! from Unihan's kMandarin readings (see `pinyin_data.rs`).

use super::pinyin_data::{BLOCKS, INDEX, SYLLABLES};

/// Pinyin for one Han character, if Unihan lists a reading.
pub(super) fn pinyin(c: char) -> Option<&'static str> {
    if let Some(p) = particle(c) {
        return Some(p);
    }
    if c == '〇' {
        return Some("líng");
    }
    let cp = c as u32;
    let &(first, _, offset) = BLOCKS.iter().find(|(first, last, _)| (*first..=*last).contains(&cp))?;
    match INDEX[offset + (cp - first) as usize] {
        0 => None,
        i => Some(SYLLABLES[i as usize - 1]),
    }
}

/// Common particles read in the neutral tone (traditional forms too).
fn particle(c: char) -> Option<&'static str> {
    Some(match c {
        '的' => "de",
        '了' => "le",
        '着' | '著' => "zhe",
        '么' | '麼' => "me",
        '吗' | '嗎' => "ma",
        '呢' => "ne",
        '吧' => "ba",
        '们' | '們' => "men",
        _ => return None,
    })
}

/// Pinyin for a run of Han characters, separated by single spaces
/// (characters without a reading are skipped).
pub(super) fn run_roman(run: &str) -> String {
    let syllables: Vec<&str> = run.chars().filter_map(pinyin).collect();
    syllables.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings() {
        assert_eq!(run_roman("我爱你"), "wǒ ài nǐ");
        assert_eq!(run_roman("中国"), "zhōng guó");
        assert_eq!(run_roman("绿色"), "lǜ sè");
        assert_eq!(run_roman("愛國"), "ài guó");
        assert_eq!(run_roman("二〇二四"), "èr líng èr sì");
    }

    #[test]
    fn neutral_particles() {
        assert_eq!(run_roman("我的"), "wǒ de");
        assert_eq!(run_roman("来了"), "lái le");
        assert_eq!(run_roman("看着"), "kàn zhe");
        assert_eq!(run_roman("看著"), "kàn zhe");
        assert_eq!(run_roman("什么"), "shén me");
        assert_eq!(run_roman("好吗"), "hǎo ma");
        assert_eq!(run_roman("你呢"), "nǐ ne");
        assert_eq!(run_roman("走吧"), "zǒu ba");
        assert_eq!(run_roman("我们"), "wǒ men");
        assert_eq!(run_roman("他們"), "tā men");
    }

    #[test]
    fn table_edges() {
        // Extension A, a compatibility ideograph (read as U+8C48), an unassigned slot, outside the table.
        assert_eq!(pinyin('\u{3400}'), Some("qiū"));
        assert_eq!(pinyin('\u{F900}'), pinyin('\u{8C48}'));
        assert!(pinyin('\u{F900}').is_some());
        assert_eq!(pinyin('\u{FAFF}'), None);
        assert_eq!(pinyin('a'), None);
        assert_eq!(INDEX.len(), BLOCKS.iter().map(|(a, b, _)| (b - a + 1) as usize).sum::<usize>());
        assert!(INDEX.iter().all(|&i| (i as usize) <= SYLLABLES.len()));
    }
}
