//! Whether the next burst may carry TrueHD or must be silence: a function of the session's facts
//! and the clock, asked once per burst.
//!
//! Carrier bytes played through LG's mixer are full-scale noise, so every carrier burst needs the
//! helper's word that the route bypasses the mixer for this epoch, an answer younger than 300 ms,
//! and the output's own ALSA controls saying unmixed eARC. Anything less feeds a zero burst; a
//! second of zeros in a row, or a route that never comes up, gives the carrier up.
use std::time::{Duration, Instant};

use super::alsa::Earc;
use super::contract::Route;

/// The helper's route answer, counting its own sample age, may be this old.
const ROUTE_FRESH: Duration = Duration::from_millis(300);
/// The session reads the controls every burst; a read this old means it has stopped.
const ALSA_FRESH: Duration = Duration::from_millis(100);
/// From the first burst offered until the helper confirms the arm.
const ARM_WAIT: Duration = Duration::from_secs(2);
/// From the arm until the route bypasses the mixer.
const BYPASS_WAIT: Duration = Duration::from_secs(1);
/// Zeros in a row, once the carrier has run, before giving it up.
const FORCED_LIMIT: Duration = Duration::from_secs(1);

/// What the session knows, each fact stamped with when it arrived.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    /// The epoch the helper last confirmed an arm for.
    pub armed_epoch: Option<u64>,
    pub route: Option<(Route, Instant)>,
    pub alsa: Option<(Earc, Instant)>,
}

impl Facts {
    /// Every fact a carrier burst needs, fresh, for `epoch`. The feeder asks before it feeds a
    /// burst and the sink probe again as the burst reaches LG's sink.
    pub fn safe(&self, now: Instant, epoch: u64) -> bool {
        let route = self.route.is_some_and(|(r, at)| {
            r.armed && r.bypassing && !r.mixed && now.saturating_duration_since(at) + Duration::from_millis(r.age_ms) < ROUTE_FRESH
        });
        let alsa = self.alsa.is_some_and(|(e, at)| e.passes() && now.saturating_duration_since(at) < ALSA_FRESH);
        self.armed_epoch == Some(epoch) && route && alsa
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Carrier,
    Zeros,
    /// Release the route and play the title's audio the normal way.
    GiveUp(&'static str),
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
                match armed {
                    None if now - offered >= ARM_WAIT => Phase::GaveUp("the helper did not arm the route"),
                    Some(at) if now - at >= BYPASS_WAIT => Phase::GaveUp("the route did not bypass the mixer"),
                    _ => Phase::Arming { offered: Some(offered), armed },
                }
            }
            Phase::Carrier => Phase::Forced { since: now },
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

    const BYPASSING: Route = Route { armed: true, bypassing: true, mixed: false, age_ms: 20 };
    const UNMIXED: Earc = Earc { on_off: 1, output_type: [2, 7, 0, 0, 0] };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Facts as the session would have them at `now`: armed for `epoch`, both reads just in.
    fn fresh(now: Instant, epoch: u64) -> Facts {
        Facts { armed_epoch: Some(epoch), route: Some((BYPASSING, now)), alsa: Some((UNMIXED, now)) }
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
        assert_eq!(g.decide(t0 + ms(1999), &Facts::default()), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(2000), &Facts::default()), Decision::GiveUp(_)), "never armed");

        let mut g = Gate::new();
        let armed = Facts { armed_epoch: Some(g.epoch()), ..Facts::default() };
        g.decide(t0, &armed);
        assert_eq!(g.decide(t0 + ms(999), &armed), Decision::Zeros);
        assert!(matches!(g.decide(t0 + ms(1000), &armed), Decision::GiveUp(_)), "armed, never bypassing");
        g.restart();
        assert!(matches!(g.decide(t0 + ms(1100), &fresh(t0 + ms(1100), g.epoch())), Decision::GiveUp(_)), "for good");
    }

    #[test]
    fn each_stale_or_failed_fact_forces_zeros_and_a_second_of_them_gives_up() {
        let t0 = Instant::now();
        let mut g = Gate::new();
        assert_eq!(g.decide(t0, &fresh(t0, 1)), Decision::Carrier);
        let now = t0 + ms(100);
        let ok = fresh(now, 1);
        let failed = [
            Facts { armed_epoch: Some(0), ..ok },
            Facts { route: Some((BYPASSING, now - ms(281))), ..ok },
            Facts { route: Some((Route { mixed: true, ..BYPASSING }, now)), ..ok },
            Facts { alsa: Some((UNMIXED, now - ms(100))), ..ok },
            Facts { alsa: Some((Earc { output_type: [2, 7, 0, 0, 1], ..UNMIXED }, now)), ..ok },
            Facts { alsa: None, ..ok },
        ];
        for facts in failed {
            assert_eq!(g.decide(now, &facts), Decision::Zeros, "{facts:?}");
            assert_eq!(g.decide(now, &ok), Decision::Carrier, "back as soon as it holds again");
        }
        let lost = Facts { alsa: None, ..ok };
        assert_eq!(g.decide(now, &lost), Decision::Zeros);
        assert_eq!(g.decide(now + ms(999), &lost), Decision::Zeros);
        assert!(matches!(g.decide(now + ms(1000), &lost), Decision::GiveUp(_)));
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
}
