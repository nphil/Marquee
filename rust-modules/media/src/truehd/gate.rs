//! Whether the next burst may carry TrueHD or must be silence: a function of the session's facts
//! and the clock, asked once per burst.
//!
//! Carrier bytes played through LG's mixer are full-scale noise, so every carrier burst needs the
//! helper's word that the route bypasses the mixer for this epoch, in an answer younger than
//! 300 ms. Anything less feeds a zero burst. The helper connects the decoder after 3 s of bypass
//! and the receiver hears the stream about 6.7 s after it, so the carrier keeps flowing while the
//! route says it bypasses; the clock on forced zeros runs only while it does not, and a route
//! that never comes up gives the carrier up.
use std::time::{Duration, Instant};

use super::contract::{Receiver, Route};

/// The helper's route answer, counting its own sample age, may be this old for a burst to carry.
const ROUTE_FRESH: Duration = Duration::from_millis(300);
/// While the helper's last word was "bypassing" and no older than this, a stale answer is not
/// taken as the route lost: on a loaded TV the helper or this process can be held up for a moment.
const BYPASS_HELD: Duration = Duration::from_secs(3);
/// From the first burst offered until the helper confirms the arm. The arm itself waits up to
/// 2 s for LG's renderer to join the audio graph and 2 s for the converter.
const ARM_WAIT: Duration = Duration::from_secs(5);
/// From the arm until the route bypasses the mixer. An answer that is late at the deadline is
/// waited for up to [`ROUTE_FRESH`] longer: only a current "not bypassing" ends the wait sooner.
/// The helper un-mixes at the sink's second buffer, or 1 s after the arm without word of it, and
/// the kernel reports `Pass-Through` after that.
const BYPASS_WAIT: Duration = Duration::from_secs(2);
/// Zeros in a row, once the carrier has run and while the route is not bypassing, before giving
/// the carrier up.
const FORCED_LIMIT: Duration = Duration::from_secs(1);
/// The longest a start, seek or resume holds the clock for the route to be audible; past it the
/// clock runs and the carrier stays, since only a route failure the helper reports
/// ([`Decision::GiveUp`]) gives it up. The helper is being cut to an `audible` within 1.5 s of the
/// carrier flowing, so the hold is the first frame for that long at most, plus whatever silence
/// is left.
pub const PLAY_HOLD_CAP: Duration = Duration::from_millis(1500);
/// The longest the held clock then waits for the receiver to report TrueHD.
pub const RECEIVER_CAP: Duration = Duration::from_millis(3500);
/// How long it holds when the `receiver` topic is missing: the Denon needs about this long to lock
/// onto a new TrueHD stream (TV soak, 2026-11-24).
pub const RECEIVER_WITHOUT_TOPIC: Duration = Duration::from_millis(2000);

/// What the session knows, each fact stamped with when it arrived.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// The epoch the helper last confirmed an arm for.
    pub armed_epoch: Option<u64>,
    pub route: Option<(Route, Instant)>,
    /// The receiver's input signal, read while the clock waits for it.
    pub receiver: Option<(Receiver, Instant)>,
}

impl Facts {
    /// The age of the route answer: the time since it arrived plus the helper's own sample age.
    fn route_age(&self, now: Instant) -> Option<Duration> {
        self.route.map(|(r, at)| now.saturating_duration_since(at) + Duration::from_millis(r.age_ms))
    }

    /// The helper's last word for `epoch` was that the mixer is bypassed, and it is no older
    /// than `limit`.
    fn bypassing_within(&self, now: Instant, epoch: u64, limit: Duration) -> bool {
        self.armed_epoch == Some(epoch)
            && self.route.is_some_and(|(r, _)| r.armed && r.bypassing && !r.mixed)
            && self.route_age(now).is_some_and(|age| age < limit)
    }

    /// The receiver is being sent the carrier for `epoch`, in an answer fresh enough to say so.
    pub fn audible(&self, now: Instant, epoch: u64) -> bool {
        self.bypassing_within(now, epoch, ROUTE_FRESH) && self.route.is_some_and(|(r, _)| r.audible)
    }

    /// Every fact a carrier burst needs, fresh, for `epoch`. The feeder asks before it feeds a
    /// burst and the sink probe again as the burst reaches LG's sink.
    pub fn safe(&self, now: Instant, epoch: u64) -> bool {
        self.bypassing_within(now, epoch, ROUTE_FRESH)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Carrier,
    Zeros,
    /// Release the route and play the title's audio the normal way.
    GiveUp(&'static str),
}

/// A first start whose Play came this much before the route was audible repeats its opening: the
/// seconds before it were fed and played silent.
pub const REWIND_AFTER: Duration = Duration::from_millis(250);

/// The Load's first start, from Play to the route being audible: when that took longer than
/// [`REWIND_AFTER`] the opening was played silent, and one seek back to the start position, on a
/// route that is armed and so stays audible, plays it in full. Once per Load; a seek or flush
/// before then is the viewer's and ends the chance.
#[derive(Debug, Default)]
pub struct OpeningRewind {
    played: Option<(Instant, i64)>,
    done: bool,
}

impl OpeningRewind {
    /// The first Play of the Load, from `start_ns` in the content's time.
    pub fn note_played(&mut self, now: Instant, start_ns: i64) {
        if !self.done && self.played.is_none() {
            self.played = Some((now, start_ns));
        }
    }

    /// The route is audible: where to seek back to and how long the opening ran silent, once.
    pub fn poll(&mut self, now: Instant, facts: &Facts, epoch: u64) -> Option<(i64, Duration)> {
        let (at, start_ns) = self.played?;
        if self.done || !facts.audible(now, epoch) {
            return None;
        }
        self.done = true;
        let silent = now.saturating_duration_since(at);
        (silent > REWIND_AFTER).then_some((start_ns, silent))
    }

    /// A seek or flush: the viewer is somewhere else now.
    pub fn end(&mut self) {
        self.done = true;
    }
}

/// Where a held clock is waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldStage {
    /// For the route to be audible.
    Arming,
    /// For the receiver to report that it has locked onto TrueHD.
    Receiver,
}

/// The clock's hold for a seek, a resume or the opening's rewind that plays the carrier: nothing
/// runs until the route is audible and the receiver has locked onto the stream, so no audio is
/// lost to a route or a receiver that is still coming up.
#[derive(Debug, Default)]
pub struct PlayHold {
    since: Option<Instant>,
    /// The helper has armed the route for some epoch of this Load.
    armed: bool,
    /// When the route became audible: the receiver wait runs from here.
    heard: Option<Instant>,
    released: bool,
}

impl PlayHold {
    /// Where the clock waits, or `None` once it runs. Only a seek, flush or resume holds: on the
    /// Load's first start the route cannot arm before Play, because LG builds the audio renderer
    /// the arm needs when the pipeline starts (TV run, 2026-11-24: Play +3330 ms, armed +3500 ms,
    /// audible +4270 ms), so waiting there only freezes the first frame.
    ///
    /// The route has [`PLAY_HOLD_CAP`] to be audible. The receiver then has until
    /// [`RECEIVER_CAP`] to report TrueHD in a reading taken since; a missing `receiver` topic
    /// holds [`RECEIVER_WITHOUT_TOPIC`] instead. Released, it stays released until the next
    /// restart.
    pub fn stage(&mut self, now: Instant, facts: &Facts, epoch: u64) -> Option<HoldStage> {
        self.armed |= facts.armed_epoch.is_some();
        if self.released || !self.armed {
            return None;
        }
        let since = *self.since.get_or_insert(now);
        let heard = match self.heard {
            Some(at) => at,
            None if facts.audible(now, epoch) => *self.heard.insert(now),
            None if now.saturating_duration_since(since) >= PLAY_HOLD_CAP => {
                self.released = true;
                return None;
            }
            None => return Some(HoldStage::Arming),
        };
        let waited = now.saturating_duration_since(heard);
        let reading = facts.receiver.and_then(|(r, at)| (at >= heard).then_some(r));
        let locked = match reading {
            Some(Receiver::TrueHd) => true,
            Some(Receiver::Missing) => waited >= RECEIVER_WITHOUT_TOPIC,
            Some(Receiver::Other) | None => waited >= RECEIVER_CAP,
        };
        if locked || waited >= RECEIVER_CAP {
            self.released = true;
            return None;
        }
        Some(HoldStage::Receiver)
    }

    /// Whether the route has armed in this Load, so a hold is possible at all.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// A seek or flush starts a new hold.
    pub fn restart(&mut self) {
        self.since = None;
        self.heard = None;
        self.released = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Zeros while the route comes up. `offered`: the first burst of this epoch; `armed`: the
    /// helper's confirmation.
    Arming { offered: Option<Instant>, armed: Option<Instant> },
    Carrier,
    Forced { since: Instant },
    GaveUp(&'static str),
}

pub struct Gate {
    epoch: u64,
    phase: Phase,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate { epoch: 1, phase: Phase::Arming { offered: None, armed: None } }
    }

    /// The epoch the session must arm for.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// A flush, seek, resume or track switch: zeros until the route is confirmed again.
    pub fn restart(&mut self) {
        if !matches!(self.phase, Phase::GaveUp(_)) {
            self.epoch += 1;
            self.phase = Phase::Arming { offered: None, armed: None };
        }
    }

    pub fn decide(&mut self, now: Instant, facts: &Facts) -> Decision {
        let open = facts.safe(now, self.epoch);
        self.phase = match self.phase {
            Phase::GaveUp(why) => Phase::GaveUp(why),
            _ if open => Phase::Carrier,
            Phase::Arming { offered, armed } => {
                let offered = offered.unwrap_or(now);
                let armed = armed.or((facts.armed_epoch == Some(self.epoch)).then_some(now));
                let answered = facts.route_age(now).is_some_and(|age| age < ROUTE_FRESH);
                match armed {
                    None if now - offered >= ARM_WAIT => Phase::GaveUp("the helper did not arm the route"),
                    Some(at) if now - at >= BYPASS_WAIT && (answered || now - at >= BYPASS_WAIT + ROUTE_FRESH) => {
                        Phase::GaveUp("the route did not bypass the mixer")
                    }
                    _ => Phase::Arming { offered: Some(offered), armed },
                }
            }
            Phase::Carrier => Phase::Forced { since: now },
            // The route still says bypassing: the answer is only late, not lost.
            Phase::Forced { .. } if facts.bypassing_within(now, self.epoch, BYPASS_HELD) => Phase::Forced { since: now },
            Phase::Forced { since } if now - since >= FORCED_LIMIT => Phase::GaveUp("a second of forced silence"),
            forced @ Phase::Forced { .. } => forced,
        };
        match self.phase {
            Phase::Carrier => Decision::Carrier,
            Phase::GaveUp(why) => Decision::GiveUp(why),
            _ => Decision::Zeros,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYPASSING: Route =
        Route { armed: true, bypassing: true, mixed: false, connected: false, audible: false, age_ms: 20 };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Facts as the session would have them at `now`: armed for `epoch`, the route just in.
    fn fresh(now: Instant, epoch: u64) -> Facts {
        Facts { armed_epoch: Some(epoch), route: Some((BYPASSING, now)), receiver: None }
    }

    /// Armed for epoch 1, the route answered at `at` and not bypassing yet.
    fn waiting(at: Instant) -> Facts {
        Facts { route: Some((Route { bypassing: false, ..BYPASSING }, at)), ..fresh(at, 1) }
    }

    #[test]
    fn zeros_until_the_route_is_confirmed_then_carrier() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &Facts::default()), Decision::Zeros);
        let not_yet = Facts { route: Some((Route { bypassing: false, ..BYPASSING }, t0)), ..fresh(t0, g.epoch()) };
        assert_eq!(g.decide(t0 + ms(500), &not_yet), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(600), &fresh(t0 + ms(600), g.epoch())), Decision::Carrier);
    }

    #[test]
    fn a_route_that_never_comes_up_is_given_up() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &Facts::default()), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(4999), &Facts::default()), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(5000), &Facts::default()), Decision::GiveUp(_)), "never armed");

        let mut g = Gate::new();
        g.decide(t0, &waiting(t0));
        assert_eq!(g.decide(t0 + ms(1999), &waiting(t0 + ms(1999))), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(2000), &waiting(t0 + ms(2000))), Decision::GiveUp(_)), "armed, never bypassing");
        g.restart();
        assert!(matches!(g.decide(t0 + ms(2100), &fresh(t0 + ms(2100), g.epoch())), Decision::GiveUp(_)), "for good");
    }

    #[test]
    fn a_route_answer_late_at_the_bypass_deadline_is_waited_for() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        g.decide(t0, &waiting(t0));
        // At the deadline the last answer is 400 ms old: one sample is late.
        let late = waiting(t0 + ms(1600));
        assert_eq!(g.decide(t0 + ms(2000), &late), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(2100), &fresh(t0 + ms(2100), 1)), Decision::Carrier);

        let mut g = Gate::new();
        g.decide(t0, &waiting(t0));
        assert_eq!(g.decide(t0 + ms(2299), &late), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(2300), &late), Decision::GiveUp(_)), "an answer that stays late is a no");
    }

    #[test]
    fn each_stale_or_failed_fact_forces_zeros() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &fresh(t0, 1)), Decision::Carrier);
        let now = t0 + ms(100);
        let ok = fresh(now, 1);
        let failed = [
            Facts { armed_epoch: Some(0), ..ok },
            Facts { route: Some((BYPASSING, now - ms(281))), ..ok },
            Facts { route: Some((Route { mixed: true, ..BYPASSING }, now)), ..ok },
            Facts { route: Some((Route { bypassing: false, ..BYPASSING }, now)), ..ok },
            Facts { route: None, ..ok },
        ];
        for facts in failed {
            assert_eq!(g.decide(now, &facts), Decision::Zeros, "{facts:?}");
            assert_eq!(g.decide(now, &ok), Decision::Carrier, "back as soon as it holds again");
        }
    }

    #[test]
    fn forced_zeros_give_up_a_second_after_the_route_stops_bypassing() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &fresh(t0, 1)), Decision::Carrier);
        let lost = Facts { route: Some((Route { bypassing: false, ..BYPASSING }, t0)), ..fresh(t0, 1) };
        assert_eq!(g.decide(t0 + ms(10), &lost), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(1009), &lost), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(1010), &lost), Decision::GiveUp(_)));
    }

    #[test]
    fn a_late_route_answer_that_said_bypassing_is_waited_for_not_given_up_on() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &fresh(t0, 1)), Decision::Carrier);
        // The helper is busy with LG's connect and mute calls: no new answer for 2.5 s.
        let silent = fresh(t0, 1);
        for at in [400, 1500, 2500] {
            assert_eq!(g.decide(t0 + ms(at), &silent), Decision::Zeros, "{at} ms");
        }
        assert_eq!(g.decide(t0 + ms(2600), &fresh(t0 + ms(2600), 1)), Decision::Carrier);
        // An answer that never comes is the route lost, a second after the last said so.
        assert_eq!(g.decide(t0 + ms(3000), &silent), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(3999), &silent), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(4000), &silent), Decision::GiveUp(_)));
    }

    #[test]
    fn a_restart_needs_an_arm_for_the_new_epoch() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &fresh(t0, g.epoch())), Decision::Carrier);
        let old = g.epoch();
        g.restart();
        assert_eq!(g.decide(t0 + ms(20), &fresh(t0 + ms(20), old)), Decision::Zeros);
        assert_eq!(g.decide(t0 + ms(40), &fresh(t0 + ms(40), g.epoch())), Decision::Carrier);
    }

    #[test]
    fn the_clock_waits_for_an_audible_route_for_the_cap_and_then_runs() {
        use HoldStage::Arming;
        let t0 = Instant::now();
        let audible = Route { audible: true, connected: true, ..BYPASSING };
        let mut hold = PlayHold::default();
        assert_eq!(hold.stage(t0, &Facts::default(), 1), None, "the first start plays: the route arms only after Play");
        // Bypassing is not enough: the receiver does not hear it yet.
        assert_eq!(hold.stage(t0, &fresh(t0, 1), 1), Some(Arming));
        let heard_at = t0 + ms(900);
        let heard = Facts { route: Some((audible, heard_at)), receiver: Some((Receiver::TrueHd, heard_at)), ..fresh(heard_at, 1) };
        assert_eq!(hold.stage(heard_at, &heard, 1), None, "audible and locked releases it");
        // A stale audible answer, or one for another epoch, does not release.
        let mut hold = PlayHold::default();
        let old = Facts { route: Some((audible, t0)), ..fresh(t0, 1) };
        assert_eq!(hold.stage(t0 + ms(400), &old, 1), Some(Arming));
        assert_eq!(hold.stage(heard_at, &heard, 2), Some(Arming), "another epoch");
        // A route that is never audible holds for the cap, then runs, and stays released.
        let mut hold = PlayHold::default();
        let armed = Facts { route: None, ..fresh(t0, 1) };
        assert_eq!(hold.stage(t0, &armed, 1), Some(Arming));
        assert_eq!(hold.stage(t0 + PLAY_HOLD_CAP - ms(1), &armed, 1), Some(Arming));
        assert_eq!(hold.stage(t0 + PLAY_HOLD_CAP, &armed, 1), None);
        assert_eq!(hold.stage(t0 + PLAY_HOLD_CAP + ms(50), &armed, 1), None);
        // A seek starts a new hold, on the route that has armed before.
        hold.restart();
        assert_eq!(hold.stage(t0 + PLAY_HOLD_CAP + ms(100), &Facts::default(), 2), Some(Arming));
    }

    /// Audible at `t0 + 100 ms`; `reading` is the receiver's, taken `at` ms after that.
    fn heard_with(t0: Instant, reading: Option<(Receiver, u64)>) -> Facts {
        let at = t0 + ms(100);
        let route = Route { audible: true, connected: true, ..BYPASSING };
        Facts { route: Some((route, at)), receiver: reading.map(|(r, after)| (r, at + ms(after))), ..fresh(at, 1) }
    }

    #[test]
    fn the_receiver_wait_ends_on_truehd_after_the_fixed_time_without_a_topic_or_at_the_cap() {
        use HoldStage::Receiver as Wait;
        let t0 = Instant::now();
        let heard_at = t0 + ms(100);
        let start = |reading: Option<(Receiver, u64)>| {
            let mut hold = PlayHold::default();
            assert_eq!(hold.stage(t0, &fresh(t0, 1), 1), Some(HoldStage::Arming));
            assert_eq!(hold.stage(heard_at, &heard_with(t0, reading), 1), Some(Wait), "audible: the receiver wait starts");
            hold
        };
        // TrueHD, read after the route was audible, releases at once.
        let mut hold = start(None);
        assert_eq!(hold.stage(heard_at + ms(50), &heard_with(t0, Some((Receiver::TrueHd, 20))), 1), None);
        // A reading from before the route was audible is not this start's.
        let mut hold = start(None);
        let stale = Facts { receiver: Some((Receiver::TrueHd, t0)), ..heard_with(t0, None) };
        assert_eq!(hold.stage(heard_at + ms(50), &stale, 1), Some(Wait));
        // Another signal holds to the cap.
        let mut hold = start(None);
        let other = heard_with(t0, Some((Receiver::Other, 10)));
        assert_eq!(hold.stage(heard_at + RECEIVER_CAP - ms(1), &other, 1), Some(Wait));
        assert_eq!(hold.stage(heard_at + RECEIVER_CAP, &other, 1), None);
        // No topic: 2.0 s.
        let mut hold = start(None);
        let missing = heard_with(t0, Some((Receiver::Missing, 10)));
        assert_eq!(hold.stage(heard_at + RECEIVER_WITHOUT_TOPIC - ms(1), &missing, 1), Some(Wait));
        assert_eq!(hold.stage(heard_at + RECEIVER_WITHOUT_TOPIC, &missing, 1), None);
        // No answer yet: the cap.
        let mut hold = start(None);
        assert_eq!(hold.stage(heard_at + RECEIVER_CAP - ms(1), &heard_with(t0, None), 1), Some(Wait));
        assert_eq!(hold.stage(heard_at + RECEIVER_CAP, &heard_with(t0, None), 1), None);
    }

    #[test]
    fn the_opening_is_rewound_once_and_only_past_the_threshold() {
        let t0 = Instant::now();
        let audible = |at: Instant| Facts { route: Some((Route { audible: true, ..BYPASSING }, at)), ..fresh(at, 1) };
        let mut r = OpeningRewind::default();
        let at = t0 + ms(940);
        assert_eq!(r.poll(at, &audible(at), 1), None, "no Play yet");
        r.note_played(t0, 5_000);
        assert_eq!(r.poll(t0 + ms(100), &fresh(t0 + ms(100), 1), 1), None, "not audible yet");
        assert_eq!(r.poll(at, &audible(at), 1), Some((5_000, ms(940))));
        assert_eq!(r.poll(at + ms(20), &audible(at + ms(20)), 1), None, "once");

        // Heard within the threshold: nothing to repeat, and no later chance.
        let mut r = OpeningRewind::default();
        r.note_played(t0, 0);
        assert_eq!(r.poll(t0 + REWIND_AFTER, &audible(t0 + REWIND_AFTER), 1), None);
        r.note_played(t0 + ms(1), 0);
        assert_eq!(r.poll(at, &audible(at), 1), None);

        // A seek before audible ends it.
        let mut r = OpeningRewind::default();
        r.note_played(t0, 0);
        r.end();
        assert_eq!(r.poll(at, &audible(at), 1), None);
    }
}
