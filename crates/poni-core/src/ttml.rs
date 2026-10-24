//! Apple Music's TTML lyrics → [`Lyrics`].
//!
//! The documents hold one `<p begin=… end=…>` per line, often with one
//! `<span>` per word (spaces between words are text between the spans).
//! Background vocals are `<span ttm:role="x-bg">`; translations and
//! romanisations are other `x-` roles. This is a tolerant scanner for those
//! documents, not a general XML parser.

use crate::lrc::{LyricLine, Lyrics};

/// A pause between lines at least this long gets a blank line, so the last
/// sung line doesn't stay lit through an instrumental part (LRCLIB's synced
/// lyrics do the same).
const GAP_S: f64 = 4.0;

/// Parse a TTML document. Lines keep their `begin` time when every line has
/// one; otherwise (or with `itunes:timing="None"`) the lyrics are unsynced.
pub fn parse_ttml(src: &str) -> Lyrics {
    let body = src.find("<body").map_or(src, |i| &src[i..]);
    let untimed_doc = attr(tag_at(src, "<tt").unwrap_or(""), "itunes:timing").is_some_and(|t| t.eq_ignore_ascii_case("none"));
    let mut lines: Vec<(Option<f64>, Option<f64>, String)> = Vec::new();
    let mut rest = body;
    while let Some(start) = find_tag(rest, "p") {
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else { break };
        let open = &rest[..open_end];
        let begin = attr(open, "begin").and_then(parse_time);
        let end = attr(open, "end").and_then(parse_time);
        if open.ends_with('/') {
            rest = &rest[open_end + 1..];
            continue;
        }
        let content_end = rest[open_end..].find("</p>").map_or(rest.len(), |i| open_end + i);
        let text = line_text(&rest[open_end + 1..content_end]);
        rest = &rest[content_end..];
        if !text.is_empty() {
            lines.push((begin, end, text));
        }
    }
    let synced = !untimed_doc && !lines.is_empty() && lines.iter().all(|(b, _, _)| b.is_some());
    if !synced {
        return Lyrics { lines: lines.into_iter().map(|(_, _, text)| LyricLine { time_s: 0.0, text }).collect(), synced: false };
    }
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::with_capacity(lines.len());
    for i in 0..lines.len() {
        let (begin, end, text) = &lines[i];
        out.push(LyricLine { time_s: begin.unwrap_or(0.0), text: text.clone() });
        if let (Some(end), Some((Some(next), _, _))) = (end, lines.get(i + 1)) {
            if next - end >= GAP_S {
                out.push(LyricLine { time_s: *end, text: String::new() });
            }
        }
    }
    Lyrics { lines: out, synced: true }
}

/// What a span's text belongs to.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Main,
    Background,
    Skip,
}

/// The visible text of one `<p>`: words joined as written, background vocals
/// appended in parentheses, translations and romanisations dropped.
fn line_text(content: &str) -> String {
    let (mut main, mut bg) = (String::new(), String::new());
    let mut stack: Vec<Part> = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(rest, &stack, &mut main, &mut bg);
            break;
        };
        push_text(&rest[..lt], &stack, &mut main, &mut bg);
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |i| &after[i + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            if name.trim() == "span" {
                stack.pop();
            }
        } else if tag_name(tag) == "span" {
            let inherited = stack.last().copied().unwrap_or(Part::Main);
            let part = match attr(tag, "ttm:role").or_else(|| attr(tag, "role")) {
                Some("x-bg") if inherited == Part::Main => Part::Background,
                Some(role) if role.starts_with("x-") && role != "x-bg" => Part::Skip,
                _ => inherited,
            };
            if !tag.ends_with('/') {
                stack.push(part);
            }
        } else if tag_name(tag) == "br" {
            push_text(" ", &stack, &mut main, &mut bg);
        }
    }
    let (main, bg) = (collapse(&main), collapse(&bg));
    match (main.is_empty(), bg.is_empty()) {
        (_, true) => main,
        (true, false) => bg,
        (false, false) if bg.starts_with('(') => format!("{main} {bg}"),
        (false, false) => format!("{main} ({bg})"),
    }
}

fn push_text(raw: &str, stack: &[Part], main: &mut String, bg: &mut String) {
    match stack.last().copied().unwrap_or(Part::Main) {
        Part::Main => main.push_str(&decode_entities(raw)),
        Part::Background => bg.push_str(&decode_entities(raw)),
        Part::Skip => {}
    }
}

/// Runs of whitespace (including line breaks in the document) → one space.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tag_name(tag: &str) -> &str {
    tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("")
}

/// Byte offset of the next `<name` start tag (`<p` but not `<parent`).
fn find_tag(s: &str, name: &str) -> Option<usize> {
    let pat = format!("<{name}");
    let mut from = 0;
    while let Some(i) = s[from..].find(&pat) {
        let at = from + i;
        match s[at + pat.len()..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' || c == '/' => return Some(at),
            _ => from = at + pat.len(),
        }
    }
    None
}

/// The text of the first `<name …>` start tag.
fn tag_at<'a>(s: &'a str, pat: &str) -> Option<&'a str> {
    let i = s.find(pat)?;
    let end = s[i..].find('>')?;
    Some(&s[i..i + end])
}

/// Value of attribute `name` in a start tag (`name="v"` or `name='v'`).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(i) = tag[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        // Whole attribute names only: "begin" must not match "xbegin".
        let before_ok = tag[..at].chars().next_back().is_some_and(char::is_whitespace);
        let after = tag[from..].trim_start();
        if !before_ok {
            continue;
        }
        let Some(after_eq) = after.strip_prefix('=') else { continue };
        let after_eq = after_eq.trim_start();
        let quote = after_eq.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let value = &after_eq[1..];
        return value.find(quote).map(|end| &value[..end]);
    }
    None
}

/// TTML times: `h:mm:ss.fff`, `mm:ss.fff`, `ss.fff`, `12.5s`, `750ms`.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return number(ms).map(|v| v / 1000.0);
    }
    let s = s.strip_suffix('s').unwrap_or(s);
    let mut total = 0.0;
    let parts: Vec<&str> = s.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (i, p) in parts.iter().enumerate() {
        let v = number(p)?;
        // Only the last part may have a fraction.
        if i + 1 < parts.len() && p.contains('.') {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') || s.matches('.').count() > 1 {
        return None;
    }
    s.parse().ok()
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`, `&#N;` and `&#xH;`.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| name.strip_prefix('#').map(|d| d.parse::<u32>()))
                    .and_then(|r| r.ok())
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are original placeholder text, not real lyrics.
    const WORD_TIMED: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word" xml:lang="en"><head><metadata><iTunesMetadata><songwriters><songwriter>Somebody</songwriter></songwriters></iTunesMetadata></metadata></head><body dur="3:21.000"><div begin="0:10.500" end="0:30.000"><p begin="0:10.500" end="0:13.000" itunes:key="L1"><span begin="0:10.500" end="0:11.000">Paper</span> <span begin="0:11.000" end="0:12.000">boats</span> <span begin="0:12.000" end="0:13.000">drift</span></p><p begin="0:13.250" end="0:16.000" itunes:key="L2"><span begin="0:13.250" end="0:14.000">Lanterns</span> <span begin="0:14.000" end="0:15.000">on</span> <span begin="0:15.000" end="0:16.000">the &amp; tide</span><span ttm:role="x-bg" begin="0:15.500" end="0:16.000"><span begin="0:15.500" end="0:16.000">(ooh)</span></span><span ttm:role="x-translation" xml:lang="id">terjemahan</span></p></div><div begin="1:02.000" end="1:10.000"><p begin="1:02.000" end="1:05.000" itunes:key="L3"><span begin="1:02.000" end="1:05.000">Harbor</span> <span begin="1:03.000" end="1:05.000">lights</span><span ttm:role="x-bg"><span>stay</span> <span>close</span></span></p></div></body></tt>"#;

    #[test]
    fn word_timed_lines() {
        let l = parse_ttml(WORD_TIMED);
        assert!(l.synced);
        let rows: Vec<(f64, &str)> = l.lines.iter().map(|x| (x.time_s, x.text.as_str())).collect();
        assert_eq!(
            rows,
            vec![
                (10.5, "Paper boats drift"),
                (13.25, "Lanterns on the & tide (ooh)"),
                // 46 s of music after the second line: a blank row at its end.
                (16.0, ""),
                (62.0, "Harbor lights (stay close)"),
            ]
        );
    }

    #[test]
    fn line_timed_and_untimed() {
        let line = r#"<tt itunes:timing="Line"><body><div><p begin="1.5" end="3">First row</p>
            <p begin="00:00:04.250" end="00:00:06.000">Second
               row</p><p begin="7s" end="8000ms">Third &#x2019;row&#39;</p></div></body></tt>"#;
        let l = parse_ttml(line);
        assert!(l.synced);
        let rows: Vec<(f64, &str)> = l.lines.iter().map(|x| (x.time_s, x.text.as_str())).collect();
        assert_eq!(rows, vec![(1.5, "First row"), (4.25, "Second row"), (7.0, "Third \u{2019}row'")]);

        let none = r#"<tt itunes:timing="None"><body><div><p>First row</p><p>Second row</p></div></body></tt>"#;
        let l = parse_ttml(none);
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
        // Timing on only some lines can't be trusted either.
        let partial = r#"<tt><body><p begin="1.0">First row</p><p>Second row</p></body></tt>"#;
        assert!(!parse_ttml(partial).synced);
    }

    #[test]
    fn tolerates_odd_documents() {
        assert!(parse_ttml("").is_empty());
        assert!(parse_ttml("<tt><body></body></tt>").is_empty());
        // Unclosed tags, comments, <br/>, <parent> that only starts like <p>.
        let odd = r#"<tt><body><parent/><p begin="2.0"><!-- note -->One<br/>row & more</p><p begin="3.0">Two"#;
        let l = parse_ttml(odd);
        let rows: Vec<&str> = l.lines.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(rows, vec!["One row & more", "Two"]);
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("1:02.5"), Some(62.5));
        assert_eq!(parse_time("01:00:00.000"), Some(3600.0));
        assert_eq!(parse_time("12.75"), Some(12.75));
        assert_eq!(parse_time("12.75s"), Some(12.75));
        assert_eq!(parse_time("750ms"), Some(0.75));
        assert_eq!(parse_time("1.5:00"), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("1:2:3:4"), None);
    }

    #[test]
    fn attributes_match_whole_names() {
        assert_eq!(attr(r#"p xbegin="9" begin="1.0""#, "begin"), Some("1.0"));
        assert_eq!(attr(r#"p begin = '2.0'"#, "begin"), Some("2.0"));
        assert_eq!(attr(r#"p end="3""#, "begin"), None);
    }
}
