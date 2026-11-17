//! **The freeze safety net**: what the player does while it waits for media.
//!
//! The pump hands [`StallWatch::step`] one [`Observation`] per tick and runs the [`Action`] it
//! answers; the [`Notice`] is what the screen shows. A wait is a tick where the player holds for
//! media on purpose (a seek landing, a prime, a rebuffer) or where the picture has stood still for
//! [`CLOCK_STILL_MS`] while its clock should be running. The source decides what kind of wait it is:
//!
//! * **Stuck.** The source is parked on a full lane and has queued nothing for
//!   [`STALL_PATIENCE_MS`]. Player and source are each waiting on the other and nothing changes by
//!   itself, so the pump restarts quietly at the playhead ([`Action::Restart`]).
//! * **Network.** The source is still working: reading, or waiting on the server (a transcode
//!   that is starting is this kind, never the first). The screen says so after
//!   [`NETWORK_NOTICE_MS`] and offers a lower quality after [`LOWER_QUALITY_OFFER_MS`].
//!
//! Stuck again within [`SAME_PLACE_NS`] of the last restart gives up restarting there
//! ([`Action::GaveUp`]): the wait is then shown as a network wait at once, with the offer.
//!
//! Every wait that has lasted [`STALL_PATIENCE_MS`] is [`Notice::Frozen`] at least, which is what
//! keeps a key press on a still picture from skipping somewhere else.

/// How long a parked source may queue nothing while the player waits before the pair counts as
/// stuck. The same second the iPhone build waits.
pub const STALL_PATIENCE_MS: u32 = 1_000;
/// How long a wait lasts before the screen says the network is the hold-up.
pub const NETWORK_NOTICE_MS: u32 = 2_000;
/// How long a wait lasts before the screen offers a lower quality.
pub const LOWER_QUALITY_OFFER_MS: u32 = 10_000;
/// How long a picture whose clock should be running may stand still before it counts as a wait:
/// five of the pipeline's position reports, which arrive 201 ms apart at 5 Hz.
pub const CLOCK_STILL_MS: u32 = 1_005;
/// A stall this close to the last restart's position is the same stall.
pub const SAME_PLACE_NS: i64 = 1_000_000_000;
/// Ticks further apart than this restart the clocks: the pump was not looking in between.
const OBSERVATION_GAP_MS: u32 = 500;

/// What the screen shows for the wait, published by the pump as a `u8`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Notice {
    /// No wait, or one too short to mention.
    #[default]
    None = 0,
    /// The picture has been still for a while: key presses show the controls instead of skipping.
    Frozen = 1,
    /// "Waiting for the network".
    Network = 2,
    /// "Waiting for the network", and OK plays at a lower quality.
    Offer = 3,
}

impl Notice {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Notice::Frozen,
            2 => Notice::Network,
            3 => Notice::Offer,
            _ => Notice::None,
        }
    }
}

/// One pump tick, as the watch needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The playback this tick belongs to. A new one forgets everything, the last restart included.
    pub playback: u32,
    /// The watch applies: an Engine past its open, no viewer pause, no rebuild in flight, not at
    /// the end of the film. Anything else forgets the current wait.
    pub covered: bool,
    /// This Engine has shown a picture. A playback that has not shown its first one is the open's,
    /// and its own watchdogs time it.
    pub seen_frame: bool,
    /// The player is holding for media on purpose: a seek landing, a prime or a rebuffer.
    pub holding: bool,
    /// The playhead, ns.
    pub position_ns: i64,
    /// Access units the source has queued since the Engine began, both lanes together.
    pub queued: u64,
    /// The source is parked on a full lane right now.
    pub parked: bool,
    /// A lower quality exists for this playback.
    pub can_lower: bool,
}

/// What the pump does this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    /// Reopen at this position, keeping the playback's choices.
    Restart { at_ns: i64 },
    /// Stuck again where the last restart was: stop restarting here.
    GaveUp { at_ns: i64 },
}

#[derive(Clone, Copy, Debug)]
struct Wait {
    since: u32,
    /// Since when the source has been parked with nothing queued.
    quiet_since: u32,
    queued: u64,
}

/// The watch's memory across ticks and across the restarts it asks for.
#[derive(Debug, Default)]
pub struct StallWatch {
    playback: u32,
    /// This playback has shown a picture on some Engine, so a reload's prime is a wait the watch
    /// covers even before the new Engine's first picture.
    had_picture: bool,
    last_tick: Option<u32>,
    /// When the playhead was last seen moving, and where it was.
    moved: Option<(u32, i64)>,
    wait: Option<Wait>,
    last_restart_ns: Option<i64>,
    gave_up: bool,
    restarts: u32,
}

impl StallWatch {
    pub fn step(&mut self, now: u32, obs: Observation) -> (Notice, Action) {
        if obs.playback != self.playback {
            *self = StallWatch { playback: obs.playback, ..StallWatch::default() };
        }
        self.had_picture |= obs.seen_frame;
        if self.last_tick.is_some_and(|t| now.wrapping_sub(t) > OBSERVATION_GAP_MS) {
            self.wait = None;
            self.moved = None;
        }
        self.last_tick = Some(now);
        if !obs.covered || !self.had_picture {
            self.wait = None;
            self.moved = Some((now, obs.position_ns));
            return (Notice::None, Action::None);
        }
        // A hold stops the clock on purpose, so stillness is only counted from the moment it ends.
        if obs.holding || self.moved.is_none_or(|(_, at)| at != obs.position_ns) {
            self.moved = Some((now, obs.position_ns));
        }
        let moved_at = self.moved.map_or(now, |(t, _)| t);
        let still = now.wrapping_sub(moved_at);
        if !obs.holding && still < CLOCK_STILL_MS {
            self.wait = None;
            if self.last_restart_ns.is_some_and(|at| (obs.position_ns - at).abs() > SAME_PLACE_NS) {
                self.last_restart_ns = None;
                self.gave_up = false;
            }
            return (Notice::None, Action::None);
        }
        let since = if obs.holding { now } else { moved_at };
        let wait = self.wait.get_or_insert(Wait { since, quiet_since: now, queued: obs.queued });
        if obs.queued != wait.queued || !obs.parked {
            wait.queued = obs.queued;
            wait.quiet_since = now;
        }
        let waited = now.wrapping_sub(wait.since);
        if !self.gave_up && now.wrapping_sub(wait.quiet_since) >= STALL_PATIENCE_MS {
            let at_ns = obs.position_ns;
            if self.last_restart_ns.is_some_and(|last| (at_ns - last).abs() <= SAME_PLACE_NS) {
                self.gave_up = true;
                return (self.notice(waited, obs.can_lower), Action::GaveUp { at_ns });
            }
            self.last_restart_ns = Some(at_ns);
            self.restarts += 1;
            self.wait = None;
            return (Notice::Frozen, Action::Restart { at_ns });
        }
        (self.notice(waited, obs.can_lower), Action::None)
    }

    fn notice(&self, waited: u32, can_lower: bool) -> Notice {
        if self.gave_up || waited >= LOWER_QUALITY_OFFER_MS {
            if can_lower { Notice::Offer } else { Notice::Network }
        } else if waited >= NETWORK_NOTICE_MS {
            Notice::Network
        } else if waited >= STALL_PATIENCE_MS {
            Notice::Frozen
        } else {
            Notice::None
        }
    }

    /// Restarts this playback has had, for Stats.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: u32 = 100;

    fn playing(position_ns: i64) -> Observation {
        Observation { playback: 1, covered: true, seen_frame: true, position_ns, can_lower: true, ..Observation::default() }
    }

    /// Steps `obs` every [`TICK`] from `from` until `until` (exclusive) and returns the last answer
    /// and every action taken on the way.
    fn run(w: &mut StallWatch, from: u32, until: u32, obs: Observation) -> ((Notice, Action), Vec<Action>) {
        let mut last = (Notice::None, Action::None);
        let mut actions = Vec::new();
        let mut now = from;
        while now < until {
            last = w.step(now, obs);
            if last.1 != Action::None {
                actions.push(last.1);
            }
            now += TICK;
        }
        (last, actions)
    }

    /// A moving picture is never a wait, however quiet or parked its source is.
    #[test]
    fn a_moving_picture_is_left_alone() {
        let mut w = StallWatch::default();
        for i in 0..100u32 {
            let obs = Observation { parked: true, ..playing(i64::from(i) * 100_000_000) };
            assert_eq!(w.step(i * TICK, obs), (Notice::None, Action::None));
        }
    }

    /// A parked source that queues nothing while the player holds is a restart at the playhead
    /// after one second, and not before.
    #[test]
    fn a_parked_source_under_a_hold_restarts_after_the_patience() {
        let mut w = StallWatch::default();
        let stuck = Observation { holding: true, parked: true, ..playing(40_000_000_000) };
        let (last, actions) = run(&mut w, 0, STALL_PATIENCE_MS, stuck);
        assert_eq!(last, (Notice::None, Action::None), "inside the patience");
        assert!(actions.is_empty());
        assert_eq!(w.step(STALL_PATIENCE_MS, stuck).1, Action::Restart { at_ns: 40_000_000_000 });
    }

    /// The picture freezing while its clock should run counts as a wait from the moment it stood
    /// still: the restart comes once the source has also been parked for the patience.
    #[test]
    fn a_still_picture_with_a_parked_source_restarts() {
        let mut w = StallWatch::default();
        w.step(0, playing(10_000_000_000));
        let frozen = Observation { parked: true, ..playing(10_000_000_000) };
        let (last, actions) = run(&mut w, TICK, CLOCK_STILL_MS, frozen);
        assert_eq!(last, (Notice::None, Action::None), "still under the clock's own allowance");
        assert!(actions.is_empty());
        let (_, actions) = run(&mut w, CLOCK_STILL_MS, CLOCK_STILL_MS + STALL_PATIENCE_MS + TICK, frozen);
        assert_eq!(actions, vec![Action::Restart { at_ns: 10_000_000_000 }]);
    }

    /// A source that keeps queuing, or is not parked at all (reading, or waiting on a transcode
    /// that is starting), is a network wait: never a restart, the notice at two seconds and the
    /// offer at ten.
    #[test]
    fn a_working_source_is_a_network_wait_with_its_notice_and_offer() {
        for (label, mut obs) in [
            ("reading", Observation { holding: true, parked: false, ..playing(5_000_000_000) }),
            ("queuing", Observation { holding: true, parked: true, ..playing(5_000_000_000) }),
        ] {
            let mut w = StallWatch::default();
            let mut actions = Vec::new();
            let mut at = |w: &mut StallWatch, now: u32, obs: Observation| {
                let (notice, action) = w.step(now, obs);
                if action != Action::None {
                    actions.push(action);
                }
                notice
            };
            let mut notices = Vec::new();
            let mut now = 0;
            while now <= LOWER_QUALITY_OFFER_MS {
                if label == "queuing" {
                    obs.queued += 1;
                }
                notices.push((now, at(&mut w, now, obs)));
                now += TICK;
            }
            assert!(actions.is_empty(), "{label}: a working source is never restarted");
            let notice_at = |t: u32| notices.iter().find(|(n, _)| *n == t).unwrap().1;
            assert_eq!(notice_at(STALL_PATIENCE_MS - TICK), Notice::None, "{label}");
            assert_eq!(notice_at(STALL_PATIENCE_MS), Notice::Frozen, "{label}");
            assert_eq!(notice_at(NETWORK_NOTICE_MS - TICK), Notice::Frozen, "{label}");
            assert_eq!(notice_at(NETWORK_NOTICE_MS), Notice::Network, "{label}");
            assert_eq!(notice_at(LOWER_QUALITY_OFFER_MS - TICK), Notice::Network, "{label}");
            assert_eq!(notice_at(LOWER_QUALITY_OFFER_MS), Notice::Offer, "{label}");
        }
    }

    /// With no lower quality to offer, the long wait keeps saying only that it is waiting.
    #[test]
    fn no_lower_quality_keeps_the_plain_notice() {
        let mut w = StallWatch::default();
        let obs = Observation { holding: true, can_lower: false, ..playing(0) };
        let (last, _) = run(&mut w, 0, LOWER_QUALITY_OFFER_MS + TICK, obs);
        assert_eq!(last.0, Notice::Network);
    }

    /// Stuck again at the place the last restart reopened gives up restarting there and shows the
    /// offer at once; once the picture has moved on, a stall somewhere else restarts again.
    #[test]
    fn stuck_again_in_the_same_place_gives_up_until_the_picture_moves_on() {
        let mut w = StallWatch::default();
        let stuck = Observation { holding: true, parked: true, ..playing(60_000_000_000) };
        let (_, actions) = run(&mut w, 0, STALL_PATIENCE_MS + TICK, stuck);
        assert_eq!(actions, vec![Action::Restart { at_ns: 60_000_000_000 }]);
        // the reload is not covered while it opens; its prime is, and sticks half a second on
        run(&mut w, 1_200, 1_600, Observation { covered: false, seen_frame: false, ..stuck });
        let again = Observation { seen_frame: false, position_ns: 60_500_000_000, ..stuck };
        let (last, actions) = run(&mut w, 1_600, 1_600 + STALL_PATIENCE_MS + TICK, again);
        assert_eq!(actions, vec![Action::GaveUp { at_ns: 60_500_000_000 }]);
        assert_eq!(last.0, Notice::Offer, "gave up: the offer is up at once");
        let (_, actions) = run(&mut w, 3_000, 6_000, again);
        assert!(actions.is_empty(), "no restart loop at the same place");
        // the picture moves on past the place, then sticks elsewhere: a fresh restart
        let mut now = 6_000;
        for i in 0..30 {
            w.step(now, playing(60_500_000_000 + i * 100_000_000));
            now += TICK;
        }
        let elsewhere = Observation { position_ns: 70_000_000_000, ..stuck };
        let (_, actions) = run(&mut w, now, now + STALL_PATIENCE_MS + TICK, elsewhere);
        assert_eq!(actions, vec![Action::Restart { at_ns: 70_000_000_000 }]);
        assert_eq!(w.restarts(), 2, "the give-up was not a restart");
    }

    /// Before this playback's first picture the open's own watchdogs own the wait, and a viewer
    /// pause, a flight or the end of the film is no wait at all.
    #[test]
    fn uncovered_ticks_and_the_open_are_never_waits() {
        let stuck = Observation { holding: true, parked: true, ..playing(0) };
        for (label, obs) in [
            ("opening", Observation { seen_frame: false, ..stuck }),
            ("uncovered", Observation { covered: false, ..stuck }),
        ] {
            let mut w = StallWatch::default();
            let (last, actions) = run(&mut w, 0, LOWER_QUALITY_OFFER_MS + TICK, obs);
            assert_eq!(last, (Notice::None, Action::None), "{label}");
            assert!(actions.is_empty(), "{label}");
        }
    }

    /// A new playback forgets the last one's restart, so its first stall restarts.
    #[test]
    fn a_new_playback_forgets_the_last_restart() {
        let mut w = StallWatch::default();
        let stuck = Observation { holding: true, parked: true, ..playing(0) };
        run(&mut w, 0, STALL_PATIENCE_MS + TICK, stuck);
        let next = Observation { playback: 2, ..stuck };
        let (_, actions) = run(&mut w, 2_000, 2_000 + STALL_PATIENCE_MS + TICK, next);
        assert_eq!(actions, vec![Action::Restart { at_ns: 0 }]);
        assert_eq!(w.restarts(), 1, "the count is this playback's");
    }

    /// A gap in the pump's ticks restarts the clocks rather than counting the gap as a wait.
    #[test]
    fn a_gap_between_ticks_is_not_counted() {
        let mut w = StallWatch::default();
        let stuck = Observation { holding: true, parked: true, ..playing(0) };
        w.step(0, stuck);
        assert_eq!(w.step(5_000, stuck), (Notice::None, Action::None));
        assert_eq!(w.step(5_000 + TICK, stuck), (Notice::None, Action::None));
    }
}
