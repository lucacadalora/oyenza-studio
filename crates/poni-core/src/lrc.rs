//! LRC (timed lyrics) parsing and lookup.
//!
//! Supports `[mm:ss.xx]`, `[mm:ss.xxx]`, `[mm:ss:xx]`, `[mm:ss]`, several
//! timestamps on one line, `[offset:±ms]`, and ignores metadata tags
//! (`[ar:…]`, `[ti:…]`, …) and enhanced word timings (`<mm:ss.xx>`).

#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub time_s: f64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    /// `true` when lines carry real timestamps.
    pub synced: bool,
}

impl Lyrics {
    /// Unsynced lyrics from plain text (one line per row, all at t=0).
    pub fn plain(text: &str) -> Lyrics {
        let lines = text
            .lines()
            .map(|l| LyricLine { time_s: 0.0, text: l.trim().to_string() })
            .collect();
        Lyrics { lines, synced: false }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.text.trim().is_empty())
    }

    /// Number of lines with text (blank stanza breaks don't count).
    pub fn text_lines(&self) -> usize {
        self.lines.iter().filter(|l| !l.text.trim().is_empty()).count()
    }

    /// Index of the line active at `t` seconds: the last line whose time is
    /// `<= t`. `None` before the first line or for unsynced lyrics.
    pub fn line_index_at(&self, t: f64) -> Option<usize> {
        if !self.synced || self.lines.is_empty() {
            return None;
        }
        // partition_point: first index whose time > t
        let idx = self.lines.partition_point(|l| l.time_s <= t);
        if idx == 0 {
            None
        } else {
            Some(idx - 1)
        }
    }

    /// Progress 0..=1 through line `i` at time `t` (used for smooth scrolling).
    pub fn line_progress(&self, i: usize, t: f64) -> f32 {
        let Some(line) = self.lines.get(i) else { return 0.0 };
        let next = self.lines.get(i + 1).map(|l| l.time_s).unwrap_or(line.time_s + 4.0);
        let span = (next - line.time_s).max(0.05);
        ((t - line.time_s) / span).clamp(0.0, 1.0) as f32
    }
}

/// Parse LRC text. Lines without timestamps are ignored when at least one
/// timestamped line exists; otherwise the text is treated as plain lyrics.
pub fn parse_lrc(src: &str) -> Lyrics {
    let mut offset_ms: f64 = 0.0;
    let mut out: Vec<LyricLine> = Vec::new();
    let mut plain_rows: Vec<String> = Vec::new();

    for raw in src.lines() {
        let mut rest = raw.trim_start_matches('\u{feff}').trim();
        let mut stamps: Vec<f64> = Vec::new();
        let mut is_tag = false;
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let inner = &stripped[..end];
            if let Some(t) = parse_stamp(inner) {
                stamps.push(t);
            } else if let Some((key, val)) = inner.split_once(':') {
                is_tag = true;
                if key.trim().eq_ignore_ascii_case("offset") {
                    offset_ms = val.trim().parse::<f64>().unwrap_or(0.0);
                }
            } else {
                break;
            }
            rest = stripped[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            if !is_tag && !rest.is_empty() {
                plain_rows.push(rest.to_string());
            }
            continue;
        }
        let text = strip_word_timings(rest).trim().to_string();
        for t in stamps {
            out.push(LyricLine { time_s: t, text: text.clone() });
        }
    }

    if out.is_empty() {
        return Lyrics::plain(&plain_rows.join("\n"));
    }
    // Positive offset means lyrics appear earlier (LRC convention).
    let shift = offset_ms / 1000.0;
    for l in &mut out {
        l.time_s = (l.time_s - shift).max(0.0);
    }
    out.sort_by(|a, b| a.time_s.partial_cmp(&b.time_s).unwrap_or(std::cmp::Ordering::Equal));
    Lyrics { lines: out, synced: true }
}

/// `mm:ss`, `mm:ss.xx`, `mm:ss.xxx`, `mm:ss:xx` → seconds.
fn parse_stamp(s: &str) -> Option<f64> {
    let s = s.trim();
    let (min, rest) = s.split_once(':')?;
    let min: u32 = min.trim().parse().ok()?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    if sec.is_empty() || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sec: u32 = sec.parse().ok()?;
    let frac_s = match frac {
        Some(f) if !f.is_empty() => {
            if !f.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let v: f64 = f.parse().ok()?;
            v / 10f64.powi(f.len() as i32)
        }
        _ => 0.0,
    };
    Some(min as f64 * 60.0 + sec as f64 + frac_s)
}

/// Remove enhanced-LRC word timings like `<00:12.34>`.
fn strip_word_timings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut buf = String::new();
            let mut closed = false;
            for d in chars.by_ref() {
                if d == '>' {
                    closed = true;
                    break;
                }
                buf.push(d);
            }
            if closed && parse_stamp(&buf).is_some() {
                continue;
            }
            out.push('<');
            out.push_str(&buf);
            if closed {
                out.push('>');
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixtures are original placeholder text, not real song lyrics.
    const SAMPLE: &str = "[ti:Test]\n[ar:Nobody]\n[00:01.00]first line\n[00:05.50][00:20.00]repeat line\n[00:10.250]third <00:10.50>word <00:11.00>timed\n[00:15:00]colon fraction\nno stamp here\n";

    #[test]
    fn parses_multiple_formats() {
        let l = parse_lrc(SAMPLE);
        assert!(l.synced);
        let times: Vec<f64> = l.lines.iter().map(|x| x.time_s).collect();
        assert_eq!(times, vec![1.0, 5.5, 10.25, 15.0, 20.0]);
        assert_eq!(l.lines[2].text, "third word timed");
        assert_eq!(l.lines[1].text, "repeat line");
        assert_eq!(l.lines[4].text, "repeat line");
    }

    #[test]
    fn index_lookup() {
        let l = parse_lrc(SAMPLE);
        assert_eq!(l.line_index_at(0.5), None);
        assert_eq!(l.line_index_at(1.0), Some(0));
        assert_eq!(l.line_index_at(7.0), Some(1));
        assert_eq!(l.line_index_at(999.0), Some(4));
        let p = l.line_progress(0, 3.25);
        assert!((p - 0.5).abs() < 1e-6);
    }

    #[test]
    fn offset_applies() {
        let l = parse_lrc("[offset:+500]\n[00:02.00]a\n");
        assert!((l.lines[0].time_s - 1.5).abs() < 1e-9);
    }

    #[test]
    fn plain_text_fallback() {
        let l = parse_lrc("hello\nworld\n");
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.line_index_at(5.0), None);
    }

    #[test]
    fn rejects_bad_stamps() {
        assert_eq!(parse_stamp("ab:cd"), None);
        assert_eq!(parse_stamp("01:2x"), None);
        assert_eq!(parse_stamp("01:02"), Some(62.0));
    }

    #[test]
    fn keeps_non_timing_angle_brackets() {
        assert_eq!(strip_word_timings("a <b> c"), "a <b> c");
    }
}
