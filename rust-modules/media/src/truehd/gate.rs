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
/// The opening's rewind waits for the receiver to report TrueHD at most this long after the route
/// is audible. The Denon reads "PCM ZERO" for the carrier's zero bursts and locks 2.3-3.4 s after
/// Play (TV run, 2026-11-24).
pub const LOCK_CAP: Duration = Duration::from_secs(4);
/// The same wait when the `receiver` topic is missing.
pub const LOCK_WITHOUT_TOPIC: Duration = Duration::from_millis(2500);
/// A route that is not audible this long after Play leaves the opening as it is.
const OPENING_GIVE_UP: Duration = Duration::from_secs(8);

/// What the session knows, each fact stamped with when it arrived.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// The epoch the helper last confirmed an arm for.
    pub armed_epoch: Option<u64>,
    pub route: Option<(Route, Instant)>,
    /// The receiver's input signal, read while the opening waits for it.
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

/// A first start whose Play came this much before the receiver locked repeats its opening: the
/// seconds before it were fed and played to a receiver that was not decoding them.
pub const REWIND_AFTER: Duration = Duration::from_millis(250);

/// What the opening's wait came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningStep {
    /// Seek back to the start position (`start_ns`, the content's time); the opening ran
    /// `silent` before the receiver locked.
    Rewind { start_ns: i64, silent: Duration },
    /// Nothing to repeat.
    Done,
}

/// The Load's first start, from Play until the receiver has locked onto the stream. The opening
/// plays while it has not; one seek back to the start position, with the receiver locked and the
/// route armed, plays it in full. Once per Load; a seek or flush before then is the viewer's and
/// ends the chance.
///
/// The receiver is locked when its topic reports TrueHD in a reading taken once the route was
/// audible. The wait is [`LOCK_CAP`] from the route being audible, or [`LOCK_WITHOUT_TOPIC`] when
/// the topic is missing.
#[derive(Debug, Default)]
pub struct OpeningRewind {
    played: Option<(Instant, i64)>,
    audible: Option<Instant>,
    done: bool,
}

impl OpeningRewind {
    /// The first Play of the Load, from `start_ns` in the content's time.
    pub fn note_played(&mut self, now: Instant, start_ns: i64) {
        if !self.done && self.played.is_none() {
            self.played = Some((now, start_ns));
        }
    }

    /// Still owed: the Load has not heard the opening's outcome, or been moved by a seek.
    pub fn pending(&self) -> bool {
        !self.done
    }

    /// The receiver's signal is wanted: the route is audible and the opening waits for the lock.
    pub fn wants_receiver(&self) -> bool {
        self.audible.is_some() && !self.done
    }

    /// The outcome, once.
    pub fn poll(&mut self, now: Instant, facts: &Facts, epoch: u64) -> Option<OpeningStep> {
        let (played, start_ns) = self.played?;
        if self.done {
            return None;
        }
        let heard = match self.audible {
            Some(at) => at,
            None if facts.audible(now, epoch) => *self.audible.insert(now),
            None if now.saturating_duration_since(played) >= OPENING_GIVE_UP => {
                self.done = true;
                return Some(OpeningStep::Done);
            }
            None => return None,
        };
        let waited = now.saturating_duration_since(heard);
        let reading = facts.receiver.and_then(|(r, at)| (at >= heard).then_some(r));
        let locked = match reading {
            Some(Receiver::TrueHd) => true,
            Some(Receiver::Missing) => waited >= LOCK_WITHOUT_TOPIC,
            Some(Receiver::Other) | None => false,
        };
        if !locked && waited < LOCK_CAP {
            return None;
        }
        self.done = true;
        let silent = now.saturating_duration_since(played);
        Some(if silent > REWIND_AFTER { OpeningStep::Rewind { start_ns, silent } } else { OpeningStep::Done })
    }

    /// A seek or flush: the viewer is somewhere else now.
    pub fn end(&mut self) {
        self.done = true;
    }
}

/// The clock's hold for a seek, a resume or the opening's rewind that plays the carrier: nothing
/// runs until the route is audible, so no audio is lost to a route that is still coming up.
#[derive(Debug, Default)]
pub struct PlayHold {
    since: Option<Instant>,
    /// The helper has armed the route for some epoch of this Load.
    armed: bool,
    released: bool,
}

impl PlayHold {
    /// Whether the clock stays stopped. Only a seek, flush or resume holds: on the Load's first
    /// start the route cannot arm before Play, because LG builds the audio renderer the arm needs
    /// when the pipeline starts (TV run, 2026-11-24: Play +3330 ms, armed +3500 ms, audible +4270
    /// ms), so waiting there only freezes the first frame. The receiver is not waited for: its
    /// lock needs the carrier's data, which only flows once the clock runs. The route has
    /// [`PLAY_HOLD_CAP`] to be audible; released, it stays released until the next restart.
    pub fn waits(&mut self, now: Instant, facts: &Facts, epoch: u64) -> bool {
        self.armed |= facts.armed_epoch.is_some();
        if self.released || !self.armed {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if facts.audible(now, epoch) || now.saturating_duration_since(since) >= PLAY_HOLD_CAP {
            self.released = true;
            return false;
        }
        true
    }

    /// A seek or flush starts a new hold.
    pub fn restart(&mut self) {
        self.since = None;
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
        let t0 = Instant::now();
        let audible = Route { audible: true, connected: true, ..BYPASSING };
        let mut hold = PlayHold::default();
        assert!(!hold.waits(t0, &Facts::default(), 1), "the first start plays: the route arms only after Play");
        // Bypassing is not enough: the receiver does not hear it yet.
        assert!(hold.waits(t0, &fresh(t0, 1), 1));
        let heard_at = t0 + ms(900);
        let heard = Facts { route: Some((audible, heard_at)), ..fresh(heard_at, 1) };
        assert!(!hold.waits(heard_at, &heard, 1), "audible releases it, receiver or not");
        // A stale audible answer, or one for another epoch, does not release.
        let mut hold = PlayHold::default();
        let old = Facts { route: Some((audible, t0)), ..fresh(t0, 1) };
        assert!(hold.waits(t0 + ms(400), &old, 1));
        assert!(hold.waits(heard_at, &heard, 2), "another epoch");
        // A route that is never audible holds for the cap, then runs, and stays released.
        let mut hold = PlayHold::default();
        let armed = Facts { route: None, ..fresh(t0, 1) };
        assert!(hold.waits(t0, &armed, 1));
        assert!(hold.waits(t0 + PLAY_HOLD_CAP - ms(1), &armed, 1));
        assert!(!hold.waits(t0 + PLAY_HOLD_CAP, &armed, 1));
        assert!(!hold.waits(t0 + PLAY_HOLD_CAP + ms(50), &armed, 1));
        // A seek starts a new hold, on the route that has armed before.
        hold.restart();
        assert!(hold.waits(t0 + PLAY_HOLD_CAP + ms(100), &Facts::default(), 2));
    }

    /// The route audible at `t0 + 900 ms`, the receiver read `after` ms later.
    fn opening_facts(t0: Instant, now: Instant, reading: Option<(Receiver, u64)>) -> Facts {
        let at = t0 + ms(900);
        let route = Route { audible: true, connected: true, ..BYPASSING };
        Facts { route: Some((route, now)), receiver: reading.map(|(r, after)| (r, at + ms(after))), ..fresh(now, 1) }
    }

    /// An opening played at `t0` whose route turned audible at `t0 + 900 ms`.
    fn opening_audible(t0: Instant) -> OpeningRewind {
        let mut r = OpeningRewind::default();
        let at = t0 + ms(900);
        assert_eq!(r.poll(at, &opening_facts(t0, at, None), 1), None, "no Play yet");
        r.note_played(t0, 5_000);
        assert_eq!(r.poll(t0 + ms(100), &fresh(t0 + ms(100), 1), 1), None, "not audible yet");
        assert!(!r.wants_receiver());
        assert_eq!(r.poll(at, &opening_facts(t0, at, None), 1), None, "audible: now the receiver is waited for");
        assert!(r.wants_receiver());
        r
    }

    #[test]
    fn the_opening_is_rewound_when_the_receiver_reports_truehd_and_once() {
        let t0 = Instant::now();
        let mut r = opening_audible(t0);
        let locked = t0 + ms(900 + 2300);
        let facts = opening_facts(t0, locked, Some((Receiver::TrueHd, 2300)));
        assert_eq!(r.poll(locked, &facts, 1), Some(OpeningStep::Rewind { start_ns: 5_000, silent: ms(3200) }));
        assert_eq!(r.poll(locked + ms(20), &facts, 1), None, "once");
        assert!(!r.pending() && !r.wants_receiver());
        r.note_played(locked, 0);
        assert_eq!(r.poll(locked + ms(40), &facts, 1), None, "not again for a later Play");

        // A reading from before the route was audible is not the lock of this start.
        let mut r = opening_audible(t0);
        let stale = Facts { receiver: Some((Receiver::TrueHd, t0)), ..opening_facts(t0, locked, None) };
        assert_eq!(r.poll(locked, &stale, 1), None);
    }

    #[test]
    fn without_a_lock_the_opening_waits_the_fixed_time_without_a_topic_or_the_cap_and_then_rewinds() {
        let t0 = Instant::now();
        let heard = t0 + ms(900);
        let rewinds = |step: Option<OpeningStep>| matches!(step, Some(OpeningStep::Rewind { start_ns: 5_000, .. }));
        // No topic: 2.5 s after audible.
        let mut r = opening_audible(t0);
        let at = heard + LOCK_WITHOUT_TOPIC;
        let missing = |now| opening_facts(t0, now, Some((Receiver::Missing, 10)));
        assert_eq!(r.poll(at - ms(1), &missing(at - ms(1)), 1), None);
        assert!(rewinds(r.poll(at, &missing(at), 1)));
        // Another signal, or no answer: the 4 s cap.
        for reading in [Some((Receiver::Other, 10)), None] {
            let mut r = opening_audible(t0);
            let at = heard + LOCK_CAP;
            assert_eq!(r.poll(at - ms(1), &opening_facts(t0, at - ms(1), reading), 1), None);
            assert!(rewinds(r.poll(at, &opening_facts(t0, at, reading), 1)));
        }
    }

    #[test]
    fn the_opening_is_left_when_the_lock_came_at_once_the_route_never_sounds_or_a_seek_came() {
        let t0 = Instant::now();
        // Locked within the threshold of Play: nothing was missed.
        let mut r = OpeningRewind::default();
        r.note_played(t0, 0);
        let now = t0 + REWIND_AFTER;
        let route = Route { audible: true, ..BYPASSING };
        let facts = Facts { route: Some((route, now)), receiver: Some((Receiver::TrueHd, now)), ..fresh(now, 1) };
        assert_eq!(r.poll(now, &facts, 1), Some(OpeningStep::Done));
        assert_eq!(r.poll(now + ms(1), &facts, 1), None, "and no later chance");
        // A route that is never audible: left after the give-up.
        let mut r = OpeningRewind::default();
        r.note_played(t0, 0);
        assert_eq!(r.poll(t0 + OPENING_GIVE_UP - ms(1), &fresh(t0, 1), 1), None);
        assert_eq!(r.poll(t0 + OPENING_GIVE_UP, &fresh(t0, 1), 1), Some(OpeningStep::Done));
        // A seek before the lock ends it.
        let mut r = opening_audible(t0);
        r.end();
        let locked = t0 + ms(3000);
        assert_eq!(r.poll(locked, &opening_facts(t0, locked, Some((Receiver::TrueHd, 2000))), 1), None);
        assert!(!r.pending());
    }
}
