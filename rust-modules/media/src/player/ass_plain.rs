//! **Styled subtitles too slow to draw give their styling up** for the rest of the playback. A
//! file's own styling (`Origin::Authored`) can ask more of libass than the TV draws in time
//! (blurred, moving signs; karaoke layers). [`SlowRenders`] watches the worker's render times; once
//! it trips, the worker hands the renderer [`plain_source`] in place of each authored source: the
//! same lines in the viewer's text look. They keep their words, times, line breaks, italics, bold
//! and an explicit top or bottom placement (`text_ass::convert`'s whitelist); positions, motion,
//! colours, blur and vector drawings go.
use super::ass::{Content, Event, Origin, Source};
use super::ass_script::ScriptIndex;
use super::text_ass::{self, Converted, TextDialect};
use super::text_style::TextStyle;
use std::collections::HashSet;
use std::sync::Arc;

/// A styled render slower than this misses frames.
pub const SLOW_RENDER_MS: u64 = 100;
/// Renders over budget for this long straight give the styling up.
pub const SLOW_FOR_MS: u64 = 3_000;
/// A new renderer (an open, a seek, a new size) fills its glyph and bitmap caches first: renders
/// this soon after one do not count.
pub const WARM_MS: u64 = 5_000;
/// Slow renders further apart than this are a line now and then, not a load that keeps the worker
/// busy: the streak starts again.
const STEADY_GAP_MS: u64 = 1_000;
/// Sets a plain source's identity apart from the authored one it was made from (ids count up from 1).
const PLAIN_ID: u64 = 1 << 63;

/// The renderer a render came from: the job's `(epoch, source id, width, height)`.
pub(super) type Renderer = (u64, u64, i32, i32);

/// Styled render times, watched for a load the TV cannot keep up with.
#[derive(Default)]
pub(super) struct SlowRenders {
    renderer: Option<Renderer>,
    warm_until_ms: u64,
    /// When the over-budget streak began, and when its last render ended.
    streak: Option<(u64, u64)>,
}

impl SlowRenders {
    /// One styled render by `renderer` that took `took_ms` and ended at `now_ms` (any steady
    /// clock). True when renders have stayed over [`SLOW_RENDER_MS`] for [`SLOW_FOR_MS`].
    pub(super) fn step(&mut self, now_ms: u64, renderer: Renderer, took_ms: u64) -> bool {
        if self.renderer != Some(renderer) {
            self.renderer = Some(renderer);
            self.warm_until_ms = now_ms.saturating_add(WARM_MS);
            self.streak = None;
        }
        if now_ms < self.warm_until_ms || took_ms <= SLOW_RENDER_MS {
            self.streak = None;
            return false;
        }
        let started = now_ms.saturating_sub(took_ms);
        let since = match self.streak {
            Some((since, last_end)) if started.saturating_sub(last_end) <= STEADY_GAP_MS => since,
            _ => started,
        };
        self.streak = Some((since, now_ms));
        now_ms.saturating_sub(since) >= SLOW_FOR_MS
    }
}

/// `authored`'s lines as plain ones in `style`, the viewer's text look: what the renderer draws in
/// its place once styling is given up. `revision` is the plain track's own, new whenever the
/// authored track or the look changes, so the renderer takes the change in. Lines that come out
/// the same at the same time (an outline layer under its text) are drawn once.
pub(super) fn plain_source(authored: &Source, style: TextStyle, revision: u64) -> Source {
    let header = text_ass::header(&style);
    let mut seen = HashSet::new();
    let mut first = |start_ms: i64, duration_ms: i64, plain: &Converted| {
        seen.insert((start_ms, duration_ms, plain.align, plain.body.clone()))
    };
    let content = match &authored.content {
        Content::Embedded { events, fonts, .. } => Content::Embedded {
            header: header.into_bytes().into(),
            events: events
                .iter()
                .filter_map(|event| {
                    // ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text
                    let packet = std::str::from_utf8(&event.payload).ok()?;
                    let mut fields = packet.splitn(9, ',');
                    let read_order = fields.next()?.trim().parse().ok()?;
                    let plain = plain_line(fields.nth(7)?)?;
                    first(event.start_ms, event.duration_ms, &plain).then(|| Event {
                        start_ms: event.start_ms,
                        duration_ms: event.duration_ms,
                        payload: text_ass::event_payload(read_order, plain.align, &plain.body).into_bytes().into(),
                    })
                })
                .collect(),
            fonts: fonts.clone(),
        },
        Content::Script { bytes, fonts } => {
            let mut script = header;
            let index = ScriptIndex::parse(bytes);
            for (start_ms, end_ms, text) in index.iter().flat_map(ScriptIndex::dialogues) {
                let Some(plain) = plain_line(&String::from_utf8_lossy(text)) else { continue };
                if end_ms > start_ms && first(start_ms, end_ms - start_ms, &plain) {
                    script.push_str(&format!(
                        "Dialogue: 0,{},{},{},,0,0,0,,{}\n",
                        text_ass::centis(start_ms / 10),
                        text_ass::centis(end_ms / 10),
                        text_ass::style_name(plain.align),
                        plain.body
                    ));
                }
            }
            Content::Script { bytes: script.into_bytes().into(), fonts: fonts.clone() }
        }
    };
    Source {
        id: authored.id | PLAIN_ID,
        revision,
        alignment_epoch: None,
        cue_timings: Arc::from([]),
        origin: Origin::Text(style),
        content,
    }
}

/// One authored line's Text column, plain. `None` for a vector drawing (`\p1` and up), which has
/// no words, and for a line with nothing left to show.
fn plain_line(text: &str) -> Option<Converted> {
    if draws(text) {
        return None;
    }
    text_ass::convert(text, TextDialect::Plain)
}

/// Whether an override block in `text` switches to drawing mode: `\p` with a non-zero scale.
fn draws(text: &str) -> bool {
    text.split('{').skip(1).filter_map(|block| block.split_once('}')).any(|(tags, _)| {
        tags.split('\\').any(|tag| {
            tag.strip_prefix('p').map(str::trim).is_some_and(|scale| {
                !scale.is_empty() && scale.bytes().all(|b| b.is_ascii_digit()) && scale.bytes().any(|b| b != b'0')
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Back-to-back renders of `took_ms` from `from_ms` until `until_ms`: when the streak tripped.
    fn renders(slow: &mut SlowRenders, renderer: Renderer, from_ms: u64, until_ms: u64, took_ms: u64) -> Option<u64> {
        let mut now = from_ms;
        while now < until_ms {
            now += took_ms;
            if slow.step(now, renderer, took_ms) {
                return Some(now);
            }
        }
        None
    }

    const A: Renderer = (1, 7, 1920, 1080);

    #[test]
    fn renders_over_budget_for_three_seconds_straight_give_the_styling_up() {
        let mut slow = SlowRenders::default();
        assert_eq!(renders(&mut slow, A, 0, WARM_MS, 150), None, "a new renderer warms up first");
        let tripped = renders(&mut slow, A, WARM_MS, WARM_MS + 10_000, 150).expect("a steady slow load trips");
        assert!((WARM_MS + SLOW_FOR_MS..WARM_MS + SLOW_FOR_MS + 150).contains(&tripped), "tripped at {tripped}");
        assert_eq!(renders(&mut SlowRenders::default(), A, 0, 60_000, SLOW_RENDER_MS), None, "on budget");
    }

    #[test]
    fn a_fast_render_a_pause_or_a_new_renderer_starts_the_count_again() {
        let mut slow = SlowRenders::default();
        assert!(!slow.step(0, A, 10));
        let mut now = WARM_MS;
        assert_eq!(renders(&mut slow, A, now, now + 2_700, 150), None);
        now += 2_750;
        assert!(!slow.step(now, A, 20), "a render on budget");
        assert_eq!(renders(&mut slow, A, now, now + 2_700, 150), None);
        now += 2_700;
        for _ in 0..10 {
            now += 1_200;
            assert!(!slow.step(now, A, 150), "a slow line now and then");
        }
        let b = (1, 8, 1920, 1080);
        assert_eq!(renders(&mut slow, b, now, now + WARM_MS - 150, 150), None, "a seek's new timeline warms up");
    }

    fn style() -> TextStyle {
        TextStyle::default()
    }

    fn embedded(texts: &[(i64, &str)]) -> Source {
        let events: Vec<Event> = texts
            .iter()
            .enumerate()
            .map(|(i, (start_ms, text))| Event {
                start_ms: *start_ms,
                duration_ms: 2_000,
                payload: format!("{i},0,Sign,,0,0,0,,{text}").into_bytes().into(),
            })
            .collect();
        Source {
            id: 5,
            revision: 3,
            alignment_epoch: None,
            cue_timings: Arc::from([]),
            origin: Origin::Authored,
            content: Content::Embedded {
                header: Arc::from(&b"[Script Info]\n"[..]),
                events: events.into(),
                fonts: Arc::from([]),
            },
        }
    }

    #[test]
    fn plain_lines_keep_words_times_and_placement_and_lose_the_styling() {
        let authored = embedded(&[
            (1_000, r"{\pos(320,40)\blur3\i1}Hello\Nthere"),
            (1_000, r"{\bord6\3c&H000000&\i1}Hello\Nthere"),
            (1_000, r"{\an8\fad(200,200)}A sign, up top"),
            (4_000, r"{\p1}m 0 0 l 100 0 100 100{\p0}"),
            (4_000, r"{\k20}Ka{\k30}ra{\k25}o{\k40}ke"),
        ]);
        let plain = plain_source(&authored, style(), 9);
        assert_eq!((plain.id, plain.revision, plain.origin), (5 | PLAIN_ID, 9, Origin::Text(style())));
        let Content::Embedded { header, events, .. } = &plain.content else { panic!("embedded stays embedded") };
        assert_eq!(&header[..], text_ass::header(&style()).as_bytes());
        let lines: Vec<(i64, String)> =
            events.iter().map(|e| (e.start_ms, String::from_utf8(e.payload.to_vec()).unwrap())).collect();
        let hello = text_ass::convert(r"{\i1}Hello\Nthere", TextDialect::Plain).unwrap().body;
        assert_eq!(lines, vec![
            (1_000, text_ass::event_payload(0, 2, &hello)),
            (1_000, text_ass::event_payload(2, 8, "A sign, up top")),
            (4_000, text_ass::event_payload(4, 2, "Karaoke")),
        ]);
    }

    #[test]
    fn a_sidecar_script_becomes_a_plain_script_on_its_own_times() {
        let script = "[Script Info]\nScriptType: v4.00+\n\n[Events]\n\
            Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
            Dialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,{\\fad(200,200)}Hi, there\r\n\
            Comment: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,not shown\n\
            Dialogue: 0,0:01:03.20,0:01:05.00,Top,,0,0,0,,{\\an8\\move(0,0,9,9)}Later\n";
        let authored = Source {
            id: 6,
            revision: 0,
            alignment_epoch: None,
            cue_timings: Arc::from([]),
            origin: Origin::Authored,
            content: Content::Script { bytes: script.as_bytes().into(), fonts: Arc::from([]) },
        };
        let Content::Script { bytes, .. } = plain_source(&authored, style(), 1).content else {
            panic!("a script stays a script")
        };
        let expected = format!(
            "{}Dialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,Hi, there\n\
             Dialogue: 0,0:01:03.20,0:01:05.00,An8,,0,0,0,,Later\n",
            text_ass::header(&style())
        );
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), expected);
    }
}
