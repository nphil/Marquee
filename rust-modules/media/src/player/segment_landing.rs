//! When a Segment seek that Play accepted has reached the picture.
//!
//! A Segment seek (webOS 4: a flush, `setTimeToDecode`, a private `sendSegmentEvent`) can be
//! accepted all the way through Play while the video sink stays on the retired timeline. The clock
//! then reports positions from before the seek, or none, and no new picture shows; nothing else in
//! the pipeline says so. Play's acceptance is therefore not the landing. The seek stays unresolved
//! until the clock has moved on the new AUs and, on a session that sends a displayed-frame counter,
//! that counter has counted a fresh frame. If that does not happen within
//! [`SEGMENT_PRESENTATION_MS`] the pump reloads once at the target, and a fresh Load installs its
//! segment by construction.
//!
//! Pure state: the pump hands in a [`Probe`] of the sink's evidence, so every rule is a unit test.

use std::ops::RangeInclusive;

/// How long the sink gets to show the new timeline once Play was accepted. Separate from the
/// reopen watchdog's budget, which measures the demux and keyframe phase that is over by then.
pub(super) const SEGMENT_PRESENTATION_MS: u32 = 4_000;

/// What the pump read from the sink and the feed this tick.
pub(super) struct Probe {
    /// The frame's tick, in milliseconds.
    pub now: u32,
    /// `SHARED.frames`: the clock's position callbacks counted since the seek was applied.
    pub clock_ticks: i32,
    /// `SHARED.pres_fed`: the position the clock last reported, in fed time.
    pub position: i64,
    /// The video accepted for this seek, in fed time: the landing keyframe to the newest AU fed.
    pub runway: RangeInclusive<i64>,
    /// `SHARED.sink_displayed_sequence`.
    pub displayed: u32,
    /// `SHARED.sink_counter_seen`: the session sends a displayed-frame counter at all.
    pub counter_seen: bool,
    /// The deadline may run: the feed gate is open and no route flight is out. A seek the pump
    /// itself is holding back is not a seek the sink is failing to show.
    pub running: bool,
}

pub(super) enum Step {
    /// Nothing to decide: Play is not accepted yet, the evidence is still coming, or the deadline
    /// is paused.
    Waiting,
    /// The sink shows the new timeline.
    Landed,
    /// The budget ran out. The event-log line names the evidence the landing was missing.
    Overdue(String),
}

/// An in-place Segment seek between its apply and its landing.
pub(super) struct Landing {
    play: Option<Play>,
}

/// What the sink had shown when Play was accepted, and since when the pump has been waiting.
struct Play {
    clock_ticks: i32,
    displayed: u32,
    waiting_since: Option<u32>,
}

impl Landing {
    pub(super) fn new() -> Landing {
        Landing { play: None }
    }

    /// Play was accepted. The evidence is whatever the sink says AFTER this: a callback queued
    /// behind the flush speaks for the retired timeline, so `clock_ticks` and `displayed` are the
    /// values read before the call. Only the first Play counts; a later one (a viewer's Resume)
    /// does not move the baseline.
    pub(super) fn played(&mut self, clock_ticks: i32, displayed: u32) {
        self.play.get_or_insert(Play { clock_ticks, displayed, waiting_since: None });
    }

    pub(super) fn step(&mut self, probe: &Probe) -> Step {
        let Some(play) = self.play.as_mut() else { return Step::Waiting };
        let facts = Facts {
            clock_advanced: probe.clock_ticks != play.clock_ticks,
            position: probe.position,
            runway: probe.runway.clone(),
            displayed: probe.displayed != play.displayed,
            counter_present: probe.counter_seen,
        };
        if facts.landed() {
            return Step::Landed;
        }
        if !probe.running {
            play.waiting_since = None;
            return Step::Waiting;
        }
        let since = *play.waiting_since.get_or_insert(probe.now);
        if probe.now.wrapping_sub(since) >= SEGMENT_PRESENTATION_MS {
            Step::Overdue(facts.deadline_line())
        } else {
            Step::Waiting
        }
    }
}

/// What the landing reads after Play, as one value so the rule that lands the seek and the line the
/// deadline writes cannot name different facts.
struct Facts {
    /// The sink's clock ticked since Play.
    clock_advanced: bool,
    /// The clock's position, in fed time.
    position: i64,
    /// The accepted video, in fed time. The requested position can be several seconds beyond the
    /// landing keyframe, so the reported position is compared to this, not to the target.
    runway: RangeInclusive<i64>,
    /// A positive displayed-frame count arrived since Play.
    displayed: bool,
    /// This session has received a displayed-frame callback, a zero count included. A set that
    /// never sends the counter (an OLED C8 on webOS 4.4.3 sends none of callbacks 46 to 49) cannot
    /// show a fresh count, so waiting for one would reload every seek that is already playing.
    counter_present: bool,
}

impl Facts {
    fn in_runway(&self) -> bool {
        self.runway.contains(&self.position)
    }

    /// The count is evidence only where the session sends one.
    fn displayed_or_unsent(&self) -> bool {
        self.displayed || !self.counter_present
    }

    fn landed(&self) -> bool {
        self.clock_advanced && self.in_runway() && self.displayed_or_unsent()
    }

    fn deadline_line(&self) -> String {
        let missing = [
            (!self.clock_advanced, "clock"),
            (!self.in_runway(), "runway"),
            (!self.displayed_or_unsent(), "displayed"),
        ]
        .iter()
        .filter_map(|&(absent, name)| absent.then_some(name))
        .collect::<Vec<_>>()
        .join(",");
        format!(
            "seek: Segment presentation deadline: missing={missing} clock_advanced={} \
             position_ms={} runway_ms={}..={} in_runway={} displayed={} counter_present={}",
            self.clock_advanced,
            self.position / 1_000_000,
            self.runway.start() / 1_000_000,
            self.runway.end() / 1_000_000,
            self.in_runway(),
            self.displayed,
            self.counter_present,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNWAY: RangeInclusive<i64> = 10_000_000_000..=14_000_000_000;

    fn probe(now: u32, clock_ticks: i32, position: i64, displayed: u32, counter_seen: bool) -> Probe {
        Probe { now, clock_ticks, position, runway: RUNWAY, displayed, counter_seen, running: true }
    }

    fn played() -> Landing {
        let mut landing = Landing::new();
        landing.played(5, 2);
        landing
    }

    fn lands(ticks: i32, position: i64, displayed: u32, counter: bool) -> bool {
        matches!(played().step(&probe(0, ticks, position, displayed, counter)), Step::Landed)
    }

    /// The line a landing that never gets its evidence writes when the budget runs out.
    fn deadline(ticks: i32, position: i64, displayed: u32, counter: bool) -> String {
        let mut landing = played();
        assert!(matches!(landing.step(&probe(0, ticks, position, displayed, counter)), Step::Waiting));
        match landing.step(&probe(SEGMENT_PRESENTATION_MS, ticks, position, displayed, counter)) {
            Step::Overdue(line) => line,
            _ => panic!("the budget ran out without a deadline"),
        }
    }

    /// The landing rule: the clock and the runway always, a fresh positive count only where the
    /// session sends one. The deadline's line names exactly what was missing.
    #[test]
    fn a_landing_needs_the_count_only_where_the_session_sends_one() {
        const ON_VIDEO: i64 = 11_000_000_000;
        assert!(lands(6, ON_VIDEO, 3, true), "a counting set with a fresh count");
        assert!(!lands(6, ON_VIDEO, 2, true), "a counting set without one");
        assert!(lands(6, ON_VIDEO, 2, false), "a set that never sends the counter");
        assert!(!lands(5, ON_VIDEO, 2, false), "the clock is always required");
        assert!(!lands(6, 9_000_000_000, 3, false), "the runway is always required");
        let line = deadline(6, ON_VIDEO, 2, true);
        assert!(line.contains("missing=displayed ") && line.contains("counter_present=true"), "{line}");
        let line = deadline(5, 9_000_000_000, 2, false);
        assert!(line.contains("missing=clock,runway ") && line.contains("counter_present=false"), "{line}");
    }

    /// Evidence from before Play is the retired timeline's: the baseline excludes it, and a landing
    /// that has not been played has nothing to decide however long it waits.
    #[test]
    fn only_what_the_sink_says_after_play_counts() {
        let mut landing = Landing::new();
        let stale = probe(60_000, 5, 11_000_000_000, 2, true);
        assert!(matches!(landing.step(&stale), Step::Waiting), "not played yet");
        landing.played(5, 2);
        landing.played(9, 9); // a later Play keeps the first baseline
        assert!(matches!(landing.step(&stale), Step::Waiting), "the same clock and count as at Play");
    }

    /// The budget starts at the first poll that can fail the seek and restarts whenever the pump
    /// itself is holding the seek back.
    #[test]
    fn the_deadline_runs_only_while_the_seek_is_not_held_back() {
        let mut landing = Landing::new();
        landing.played(5, 2);
        let at = |now, running| Probe { running, ..probe(now, 5, 9_000_000_000, 2, true) };
        assert!(matches!(landing.step(&at(1_000, true)), Step::Waiting));
        assert!(matches!(landing.step(&at(1_000 + SEGMENT_PRESENTATION_MS - 1, true)), Step::Waiting));
        assert!(matches!(landing.step(&at(1_000 + SEGMENT_PRESENTATION_MS, false)), Step::Waiting), "held back");
        assert!(matches!(landing.step(&at(1_000 + SEGMENT_PRESENTATION_MS, true)), Step::Waiting), "restarted");
        assert!(matches!(
            landing.step(&at(1_000 + 2 * SEGMENT_PRESENTATION_MS, true)),
            Step::Overdue(_)
        ));
    }
}
