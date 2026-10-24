//! Windows' Japanese reading analyzer against Oyen Poni's own dictionary, on
//! real songs. The "Japanese readings compared" CI job
//! (`.github/workflows/ja-compare.yml`) fetches the lyrics of Apple Music
//! Japan's chart at run time (`tools/ja_compare_fetch.py`; song lyrics never
//! go in the repo) and runs this test on a Windows runner, which has the
//! analyzer's data:
//!
//! ```text
//! PONI_JA_COMPARE=<folder of .lrc files> PONI_JA_COMPARE_OUT=<report.md>
//! cargo test -p poni compare_readings -- --ignored --nocapture
//! ```
//!
//! The report counts the lines both read the same way and lists the words
//! they read differently, most frequent first. Neither side is taken as
//! right: a person judges the list. It shows words, never whole lines.

use super::romanize::{japanese, on_sta};
use poni_core::lrc::parse_lrc;
use poni_core::romanize::{self, JaDict, Ruby, Script};
use std::collections::{HashMap, HashSet};

/// How many differences the report lists, most frequent first.
const LIST_MAX: usize = 200;
/// Longer stretches are cut in the report (it lists words, not lyrics).
const TEXT_MAX: usize = 12;

#[derive(Default)]
struct Stats {
    songs: usize,
    lines: usize,
    same: usize,
    unaligned: usize,
    failed: usize,
}

#[test]
#[ignore = "needs PONI_JA_COMPARE (songs fetched at run time) and Windows' Japanese reading data"]
fn compare_readings() {
    let dir = std::env::var("PONI_JA_COMPARE").expect("PONI_JA_COMPARE: a folder of .lrc files");
    let dict = JaDict::embedded().expect("Oyen Poni's dictionary loads");
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("the folder reads").filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "lrc")).collect();
    files.sort();

    let mut stats = Stats::default();
    let mut seen = HashSet::new();
    let mut diffs: HashMap<(String, String, String), usize> = HashMap::new();
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        let lyrics = parse_lrc(&src);
        let all: Vec<&str> = lyrics.lines.iter().map(|l| l.text.trim()).collect();
        if romanize::song_script(&all) != Some(Script::Japanese) {
            continue;
        }
        stats.songs += 1;
        // Each different line once: choruses would count many times otherwise.
        let texts: Vec<&str> = all.into_iter().filter(|t| romanize::needs_roman(t) && seen.insert(t.to_string())).collect();
        let windows = on_sta(|| texts.iter().map(|t| japanese(t).map(|(pieces, _)| pieces)).collect::<Vec<_>>()).expect("the analyzer's thread starts");
        for (text, win) in texts.iter().zip(windows) {
            stats.lines += 1;
            let win = match win {
                Ok(pieces) => pieces,
                Err(e) => {
                    stats.failed += 1;
                    eprintln!("{}: {e}", path.display());
                    continue;
                }
            };
            let own = dict.ruby(text);
            if roman(&win) == roman(&own) {
                stats.same += 1;
                continue;
            }
            match differences(text, &win, &own) {
                Some(found) => found.into_iter().for_each(|d| *diffs.entry(d).or_default() += 1),
                None => stats.unaligned += 1,
            }
        }
    }

    let report = report(&stats, diffs);
    println!("{report}");
    if let Ok(out) = std::env::var("PONI_JA_COMPARE_OUT") {
        std::fs::write(&out, &report).expect("the report writes");
    }
    assert!(stats.songs > 0, "no Japanese songs in {dir}");
    assert_eq!(stats.failed, 0, "Windows' analyzer failed on {} lines (no Japanese reading data?)", stats.failed);
}

/// The romaji of a line as one lowercase run: spaces between words differ
/// between the readers without changing how the line reads.
fn roman(pieces: &[Ruby]) -> String {
    pieces.iter().flat_map(|p| p.roman.chars()).filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// The stretches of `line` the two readers read differently, as (text,
/// Windows' romaji, the dictionary's romaji), cut where both start a new
/// piece. None when a reader's pieces don't add up to the line.
fn differences(line: &str, windows: &[Ruby], own: &[Ruby]) -> Option<Vec<(String, String, String)>> {
    let len = line.chars().count();
    let ends = |pieces: &[Ruby]| {
        let mut at = 0;
        let mut out = vec![0];
        for p in pieces {
            at += p.text.chars().count();
            out.push(at);
        }
        (at == len).then_some(out)
    };
    let (w, o) = (ends(windows)?, ends(own)?);
    let cuts: Vec<usize> = w.iter().copied().filter(|c| o.contains(c)).collect();
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    for span in cuts.windows(2) {
        let (a, b) = (span[0], span[1]);
        let (rw, ro) = (roman_between(windows, &w, a, b), roman_between(own, &o, a, b));
        if rw != ro {
            let text: String = chars[a..b].iter().collect::<String>().trim().to_string();
            let text = if text.chars().count() > TEXT_MAX { text.chars().take(TEXT_MAX).chain(['…']).collect() } else { text };
            out.push((text, rw, ro));
        }
    }
    Some(out)
}

/// The romaji of the pieces that lie between characters `a` and `b`.
fn roman_between(pieces: &[Ruby], ends: &[usize], a: usize, b: usize) -> String {
    let inside: Vec<Ruby> = pieces.iter().zip(ends.windows(2)).filter(|(_, e)| e[0] >= a && e[1] <= b).map(|(p, _)| p.clone()).collect();
    roman(&inside)
}

fn report(s: &Stats, diffs: HashMap<(String, String, String), usize>) -> String {
    let pct = |n: usize| if s.lines == 0 { 0.0 } else { 100.0 * n as f64 / s.lines as f64 };
    let total: usize = diffs.values().sum();
    let mut rows: Vec<_> = diffs.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let cell = |t: &str| if t.is_empty() { "(none)".to_string() } else { t.replace('|', "\\|") };
    let mut out = String::from("### Japanese readings: Windows' analyzer and Oyen Poni's dictionary\n\n");
    out += &format!(
        "{} Japanese songs from Apple Music Japan's chart, lyrics from LRCLIB (fetched for this run, not stored).\n\n\
         - {} different lines; both read {} of them the same way ({:.1}%).\n\
         - {} words read differently, {} distinct ones; {} lines couldn't be lined up.\n\n",
        s.songs, s.lines, s.same, pct(s.same), total, rows.len(), s.unaligned
    );
    if rows.is_empty() {
        return out;
    }
    out += &format!("The {} most frequent differences:\n\n| Times | Text | Windows | Oyen Poni |\n|---:|---|---|---|\n", rows.len().min(LIST_MAX));
    for ((text, win, own), n) in rows.iter().take(LIST_MAX) {
        out += &format!("| {n} | {} | {} | {} |\n", cell(text), cell(win), cell(own));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb(text: &str, roman: &str) -> Ruby {
        Ruby { text: text.into(), roman: roman.into() }
    }

    #[test]
    fn differences_are_cut_where_both_readers_split() {
        let windows = [rb("二人で", "futari de"), rb("歩く", "aruku")];
        let own = [rb("二人", "ninin"), rb("で", "de"), rb("歩く", "aruku")];
        assert_eq!(differences("二人で歩く", &windows, &own), Some(vec![("二人で".into(), "futaride".into(), "nininde".into())]));
        // Spacing alone is no difference.
        assert_eq!(roman(&[rb("呼ん", "yon"), rb("だ", "da")]), roman(&[rb("呼んだ", "yonda")]));
        // Pieces that don't add up to the line can't be lined up.
        assert_eq!(differences("夜", &[rb("夜", "yoru")], &[rb("夜x", "yoru")]), None);
    }
}
