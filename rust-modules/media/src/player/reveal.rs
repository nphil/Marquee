//! **"Reveal as spoken"**: show a text cue a word at a time (or a line at a time) instead of whole,
//! so the next words are not read before they are heard, and a following line never shows early.
//!
//! A subtitle file times whole cues, not words, so the time of each word is ESTIMATED from the
//! cue's own span: words weigh by their syllables (characters for scripts without them), a comma
//! adds half a word's pause and a full stop one, the last word starts by 90 % of the cue and the
//! rest of the cue is the tail where everything stays up. Exact word timing needs the audio
//! (server-side forced alignment, for instance WhisperX); that is a later option, not this module.
//!
//! The reveal is written into the cue itself as ASS override tags, so libass lays the line out
//! whole (nothing shifts as words appear) and the player's existing "animated cue" handling
//! re-renders it while it plays: each unit after the first opens invisible (`\1a`, `\3a`, `\4a`
//! at `FF`) and fades in over [`FADE_MS`] to the style's own alphas ([`Params`]).
//!
//! Cues that already carry animation (a `\t`, `\fad` or a karaoke tag) are left exactly as they
//! are: their timing is exact. Authored ASS/SSA is never passed through here at all; its karaoke
//! plays as the file wrote it.

use super::text_style::Layout;
use plx_plex::plex::session::Reveal;

/// How long one unit takes to fade in.
pub const FADE_MS: i64 = 160;
/// The share of the cue in which the last unit starts; the rest is the tail.
const SPEAKING_SHARE: f64 = 0.9;
/// Cues shorter than this show whole: there is no time to reveal anything in.
const MIN_CUE_MS: i64 = 700;

/// What the transform needs besides the mode: the alphas the style's outline and shadow have, so a
/// unit fades in TO them and not to fully opaque (`&H26` for today's 85 % outline).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub mode: Reveal,
    pub outline_alpha: u8,
    pub back_alpha: u8,
}

impl Params {
    pub fn of(layout: &Layout) -> Self {
        let edge = layout.edge_fields();
        Self {
            mode: layout.reveal,
            outline_alpha: (edge.outline_colour >> 24) as u8,
            back_alpha: (edge.back_colour >> 24) as u8,
        }
    }
}

/// One piece of an event body: an override block, a hard line break, or visible text.
#[derive(Debug, PartialEq, Eq)]
enum Seg<'a> {
    Tag(&'a str),
    Break(&'a str),
    Text(&'a str),
}

fn segments(body: &str) -> Vec<Seg<'_>> {
    fn flush<'a>(out: &mut Vec<Seg<'a>>, body: &'a str, from: usize, to: usize) {
        if to > from {
            out.push(Seg::Text(&body[from..to]));
        }
    }
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                // an unterminated brace is text, as libass reads it
                if let Some(close) = body[i..].find('}') {
                    flush(&mut out, body, start, i);
                    out.push(Seg::Tag(&body[i..i + close + 1]));
                    i += close + 1;
                    start = i;
                    continue;
                }
                i += 1;
            }
            b'\\' if matches!(bytes.get(i + 1), Some(b'N' | b'n')) => {
                flush(&mut out, body, start, i);
                out.push(Seg::Break(&body[i..i + 2]));
                i += 2;
                start = i;
            }
            // an escape (`\h`, `\{`, `\\`) skips its character whole, whatever its width
            b'\\' => i += 1 + body[i + 1..].chars().next().map_or(0, char::len_utf8),
            _ => i += 1,
        }
    }
    flush(&mut out, body, start, bytes.len());
    out
}

fn is_ideograph(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

fn is_thai_like(c: char) -> bool {
    matches!(c as u32, 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

/// Syllables of a word, estimated: vowel groups for Latin-script words, characters for the rest.
fn syllables(word: &str) -> f64 {
    let mut vowel_groups = 0u32;
    let mut in_group = false;
    let (mut letters, mut other_script, mut digits) = (0u32, 0u32, 0u32);
    for c in word.chars() {
        if c.is_ascii_digit() {
            digits += 1;
            in_group = false;
        } else if c.is_alphabetic() {
            if c.is_ascii() || matches!(c as u32, 0x00C0..=0x024F) {
                letters += 1;
                let l = c.to_lowercase().next().unwrap_or(c);
                let vowel = "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýÿœ".contains(l);
                if vowel && !in_group {
                    vowel_groups += 1;
                }
                in_group = vowel;
            } else {
                other_script += 1;
                in_group = false;
            }
        } else {
            in_group = false;
        }
    }
    let latin = if letters > 0 { f64::from(vowel_groups.max(1)) } else { 0.0 };
    (latin + f64::from(other_script) / 2.2 + f64::from(digits) / 2.0).max(1.0)
}

/// The pause a word's last character asks for, in word weights.
fn pause_after(word: &str) -> f64 {
    let end = word.trim_end_matches(['"', '\'', '”', '’', ')', ']', '»', '」', '』']).chars().last();
    match end {
        Some('.' | '!' | '?' | '…' | '。' | '！' | '？') => 1.0,
        Some(',' | ';' | ':' | '—' | '–' | '、' | '，' | '；' | '：') => 0.5,
        _ => 0.0,
    }
}

/// One thing that appears: where it starts in the body, its weight, and the pause after it.
struct Unit {
    at: usize,
    weight: f64,
    pause: f64,
}

/// The reveal units of a body: its words, or its lines.
fn units(body: &str, segs: &[Seg<'_>], mode: Reveal) -> Vec<Unit> {
    let base = body.as_ptr() as usize;
    let at_of = |s: &str| s.as_ptr() as usize - base;
    let mut out = Vec::new();
    match mode {
        Reveal::Off => {}
        Reveal::Word => {
            for seg in segs {
                let Seg::Text(text) = seg else { continue };
                let mut offset = 0;
                let mut chars = text.char_indices().peekable();
                let mut word_start: Option<usize> = None;
                let flush = |from: usize, to: usize, out: &mut Vec<Unit>| {
                    let word = &text[from..to];
                    if !word.trim().is_empty() {
                        out.push(Unit { at: at_of(word), weight: syllables(word), pause: pause_after(word) });
                    }
                };
                let mut thai_run = 0;
                while let Some((i, c)) = chars.next() {
                    offset = i + c.len_utf8();
                    if c == ' ' || c == '\t' {
                        if let Some(s) = word_start.take() {
                            flush(s, i, &mut out);
                        }
                        thai_run = 0;
                    } else if is_ideograph(c) {
                        if let Some(s) = word_start.take() {
                            flush(s, i, &mut out);
                        }
                        flush(i, offset, &mut out);
                        thai_run = 0;
                    } else if is_thai_like(c) {
                        if word_start.is_none() {
                            word_start = Some(i);
                            thai_run = 0;
                        }
                        thai_run += 1;
                        if thai_run == 4 && chars.peek().is_some_and(|(_, n)| !n.is_whitespace()) {
                            if let Some(s) = word_start.take() {
                                flush(s, offset, &mut out);
                            }
                            thai_run = 0;
                        }
                    } else if word_start.is_none() {
                        word_start = Some(i);
                    }
                }
                if let Some(s) = word_start.take() {
                    flush(s, offset, &mut out);
                }
            }
        }
        Reveal::Line => {
            let mut line_start: Option<&str> = None;
            let mut line_chars = 0usize;
            let mut line_end = "";
            let close = |start: &mut Option<&str>, chars: &mut usize, end: &str, out: &mut Vec<Unit>| {
                if let Some(s) = start.take() {
                    out.push(Unit { at: at_of(s), weight: (*chars as f64 / 3.0).max(1.0), pause: pause_after(end) });
                }
                *chars = 0;
            };
            for seg in segs {
                match seg {
                    Seg::Text(text) => {
                        let visible = text.trim_start();
                        if line_start.is_none() && !visible.is_empty() {
                            line_start = Some(visible);
                        }
                        line_chars += text.chars().filter(|c| !c.is_whitespace()).count();
                        if !text.trim().is_empty() {
                            line_end = text.trim_end();
                        }
                    }
                    Seg::Break(_) => close(&mut line_start, &mut line_chars, line_end, &mut out),
                    Seg::Tag(_) => {}
                }
            }
            close(&mut line_start, &mut line_chars, line_end, &mut out);
        }
    }
    out
}

fn hex(a: u8) -> String {
    format!("&H{a:02X}&")
}

/// The body of an event shown over `duration_ms`, with every unit after the first faded in at its
/// estimated time. A body with fewer than two units, a cue shorter than [`MIN_CUE_MS`] and a body
/// that already animates come back unchanged.
pub fn reveal_body(body: &str, duration_ms: i64, params: Params) -> String {
    if params.mode == Reveal::Off || duration_ms < MIN_CUE_MS || super::ass_timing::text_animated(body.as_bytes()) {
        return body.to_string();
    }
    let segs = segments(body);
    let units = units(body, &segs, params.mode);
    if units.len() < 2 {
        return body.to_string();
    }
    let speaking = duration_ms as f64 * SPEAKING_SHARE;
    let total: f64 = units.iter().map(|u| u.weight).sum::<f64>() + units[..units.len() - 1].iter().map(|u| u.pause).sum::<f64>();
    let fade = FADE_MS.min((speaking / units.len() as f64).max(60.0) as i64);
    let closed = format!("\\1a{}\\3a{}\\4a{}", hex(0), hex(params.outline_alpha), hex(params.back_alpha));
    let open = format!("\\1a{}\\3a{}\\4a{}", hex(0xFF), hex(0xFF), hex(0xFF));
    let mut out = String::with_capacity(body.len() + units.len() * 64);
    let mut cursor = 0;
    let mut elapsed = 0.0;
    for (i, unit) in units.iter().enumerate() {
        out.push_str(&body[cursor..unit.at]);
        cursor = unit.at;
        if i > 0 {
            let start = ((elapsed / total) * speaking).round() as i64;
            let start = start.min(duration_ms - fade);
            out.push_str(&format!("{{{open}\\t({start},{},{closed})}}", start + fade));
        }
        elapsed += unit.weight + unit.pause;
    }
    out.push_str(&body[cursor..]);
    out
}

/// A Matroska ASS packet (`ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`) with
/// its text revealed over `duration_ms`.
pub fn reveal_packet(packet: &[u8], duration_ms: i64, params: Params) -> Vec<u8> {
    reveal_after(packet, 8, |_| Some(duration_ms), params)
}

/// `Dialogue:` lines (`Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text`) with each
/// line's text revealed over its own span.
pub fn reveal_events(events: &str, params: Params) -> String {
    let mut out = String::with_capacity(events.len() + events.len() / 2);
    for line in events.split_inclusive('\n') {
        match line.strip_prefix("Dialogue: ") {
            Some(rest) => {
                out.push_str("Dialogue: ");
                let (body, newline) = rest.strip_suffix('\n').map_or((rest, ""), |b| (b, "\n"));
                let revealed = reveal_after(
                    body.as_bytes(),
                    9,
                    |head| {
                        let mut fields = head.split(',').skip(1);
                        let (start, end) = (centis_of(fields.next()?)?, centis_of(fields.next()?)?);
                        Some((end - start) * 10)
                    },
                    params,
                );
                out.push_str(&String::from_utf8_lossy(&revealed));
                out.push_str(newline);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// `H:MM:SS.cc` as centiseconds.
fn centis_of(t: &str) -> Option<i64> {
    let (clock, cs) = t.trim().split_once('.')?;
    let mut parts = clock.split(':');
    let (h, m, s): (i64, i64, i64) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    Some(((h * 60 + m) * 60 + s) * 100 + cs.parse::<i64>().ok()?)
}

/// Split `line` after its `commas`-th comma, give the head to `duration` for the span in ms, and
/// reveal the text that follows.
fn reveal_after(line: &[u8], commas: usize, duration: impl FnOnce(&str) -> Option<i64>, params: Params) -> Vec<u8> {
    let mut seen = 0;
    let Some(cut) = line.iter().position(|&b| {
        if b == b',' {
            seen += 1;
        }
        seen == commas
    }) else {
        return line.to_vec();
    };
    let (head, text) = line.split_at(cut + 1);
    let (Ok(head_str), Ok(text_str)) = (std::str::from_utf8(head), std::str::from_utf8(text)) else {
        return line.to_vec();
    };
    let Some(ms) = duration(head_str) else { return line.to_vec() };
    let mut out = head.to_vec();
    out.extend_from_slice(reveal_body(text_str, ms, params).as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: Params = Params { mode: Reveal::Word, outline_alpha: 0x26, back_alpha: 0xFF };
    const LINES: Params = Params { mode: Reveal::Line, outline_alpha: 0x26, back_alpha: 0xFF };

    fn starts(s: &str) -> Vec<i64> {
        s.split("\\t(").skip(1).filter_map(|t| t.split(',').next()?.parse().ok()).collect()
    }

    #[test]
    fn off_and_short_and_single_unit_cues_are_untouched() {
        assert_eq!(reveal_body("Hello there", 3000, Params { mode: Reveal::Off, ..WORDS }), "Hello there");
        assert_eq!(reveal_body("Hello there", 600, WORDS), "Hello there", "too short to reveal in");
        assert_eq!(reveal_body("Hello", 3000, WORDS), "Hello", "one word is whole");
        assert_eq!(reveal_body("{\\an8}Hi", 3000, WORDS), "{\\an8}Hi");
    }

    #[test]
    fn words_appear_in_order_by_weight_and_the_last_starts_inside_the_cue() {
        let out = reveal_body("I really enjoyed everything", 4000, WORDS);
        let at = starts(&out);
        assert_eq!(at.len(), 3, "every word after the first has a start: {out}");
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{at:?}");
        assert!(at.iter().all(|&t| t > 0 && t + FADE_MS <= 4000), "{at:?}");
        assert!(*at.last().unwrap() <= (4000.0 * SPEAKING_SHARE) as i64, "the tail stays whole: {at:?}");
        assert!(out.starts_with("I {"), "the first word is simply there: {out}");
        // the longer a word, the longer the next one waits for it
        assert!(at[1] - at[0] > at[0], "'really' (2 syllables) takes longer than 'I'");
    }

    #[test]
    fn units_fade_in_to_the_styles_own_alphas() {
        let out = reveal_body("one two", 2000, WORDS);
        assert!(out.contains("{\\1a&HFF&\\3a&HFF&\\4a&HFF&\\t("), "{out}");
        assert!(out.contains("\\1a&H00&\\3a&H26&\\4a&HFF&)"), "fills to the outline's own 26: {out}");
        let hard = reveal_body("one two", 2000, Params { outline_alpha: 0x00, back_alpha: 0x40, ..WORDS });
        assert!(hard.contains("\\3a&H00&\\4a&H40&)"), "{hard}");
    }

    #[test]
    fn punctuation_adds_a_pause_before_the_next_word() {
        let flat = starts(&reveal_body("one two three four", 4000, WORDS));
        let stops = starts(&reveal_body("one. two three four", 4000, WORDS));
        assert!(stops[0] > flat[0], "after a full stop the next word waits longer: {flat:?} vs {stops:?}");
    }

    #[test]
    fn tags_and_line_breaks_are_kept_and_never_split() {
        let out = reveal_body("{\\i1}Don't go{\\i0}\\Nout there", 4000, WORDS);
        assert!(out.starts_with("{\\i1}Don't "), "{out}");
        assert!(out.contains("{\\i0}\\N"), "{out}");
        assert!(out.ends_with("there"), "{out}");
        let plain: String = {
            let mut s = String::new();
            let mut skip = false;
            for c in out.chars() {
                match c {
                    '{' => skip = true,
                    '}' => skip = false,
                    c if !skip => s.push(c),
                    _ => {}
                }
            }
            s
        };
        assert_eq!(plain, "Don't go\\Nout there", "stripping the tags gives the cue back");
    }

    #[test]
    fn line_mode_reveals_a_line_at_a_time_and_never_early() {
        let out = reveal_body("First line here\\NSecond line follows", 4000, LINES);
        let at = starts(&out);
        assert_eq!(at.len(), 1, "two lines: the second has one start: {out}");
        assert!(at[0] >= 1500 && at[0] <= 2500, "about half way: {at:?}");
        assert!(out.starts_with("First line here\\N{"), "the second line's reveal opens right after the break: {out}");
        assert_eq!(reveal_body("One line only", 4000, LINES), "One line only");
    }

    #[test]
    fn cjk_reveals_per_character_and_thai_in_chunks() {
        let out = reveal_body("今日はいい天気", 4000, WORDS);
        assert_eq!(starts(&out).len(), 6, "one start per character after the first: {out}");
        let thai = reveal_body("สวัสดีครับทุกคนวันนี้", 4000, WORDS);
        assert!(starts(&thai).len() >= 2, "{thai}");
    }

    #[test]
    fn a_cue_that_already_animates_keeps_its_own_timing() {
        let karaoke = "{\\k50}one {\\k50}two {\\k50}three";
        assert_eq!(reveal_body(karaoke, 4000, WORDS), karaoke);
        let fade = "{\\fad(200,200)}one two three";
        assert_eq!(reveal_body(fade, 4000, WORDS), fade);
    }

    #[test]
    fn rtl_text_reveals_in_reading_order() {
        let out = reveal_body("مرحبا بالعالم كيف حالك", 4000, WORDS);
        let at = starts(&out);
        assert_eq!(at.len(), 3);
        assert!(out.find("بالعالم").unwrap() < out.find("كيف").unwrap(), "logical order is kept for libass to lay out: {out}");
    }

    #[test]
    fn packets_and_dialogue_lines_take_the_span_from_their_own_fields() {
        let packet = b"7,0,Default,,0,0,0,,one two three four";
        let out = String::from_utf8(reveal_packet(packet, 4000, WORDS)).unwrap();
        assert!(out.starts_with("7,0,Default,,0,0,0,,one {"), "{out}");
        assert_eq!(starts(&out).len(), 3);
        let events = "Dialogue: 0,0:00:01.00,0:00:05.00,Default,,0,0,0,,one two three four\nDialogue: 0,0:00:06.00,0:00:06.30,Default,,0,0,0,,too short to reveal\n";
        let revealed = reveal_events(events, WORDS);
        let mut lines = revealed.lines();
        let first = lines.next().unwrap();
        assert!(first.starts_with("Dialogue: 0,0:00:01.00,0:00:05.00,Default,,0,0,0,,one {"), "{first}");
        assert_eq!(starts(first).len(), 3);
        assert_eq!(lines.next().unwrap(), "Dialogue: 0,0:00:06.00,0:00:06.30,Default,,0,0,0,,too short to reveal");
        assert!(revealed.ends_with('\n'));
        assert_eq!(reveal_events(events, Params { mode: Reveal::Off, ..WORDS }), events.to_string().replace("one two", "one two"));
    }

    #[test]
    fn a_text_that_is_not_utf8_or_has_no_text_field_is_returned_as_is() {
        assert_eq!(reveal_packet(b"7,0,Default", 4000, WORDS), b"7,0,Default".to_vec());
        assert_eq!(reveal_packet(&[b'1', b',', 0xFF, 0xFE], 4000, WORDS), vec![b'1', b',', 0xFF, 0xFE]);
    }

    #[test]
    fn params_take_the_alphas_of_the_edge() {
        let layout = Layout::default();
        assert_eq!(Params::of(&layout), Params { mode: Reveal::Off, outline_alpha: 0x26, back_alpha: 0xFF });
    }
}
