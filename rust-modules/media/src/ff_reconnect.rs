//! Reconnecting a direct-play transfer that died mid-body.
//!
//! A media server that stalls for a while (a slow database query, a transcode dying beside the
//! film) resets or times out the transfer, and its next few requests fail too. Reopening at the
//! byte reached, immediately and three times, spent the whole budget inside a second and ended the
//! film; the same reopen a few seconds later succeeds. [`heal`] reopens with a growing wait
//! between attempts for about 25 s, and stops at once on a refusal that waiting cannot change.
//!
//! The reopen itself is the demuxer's existing one (`seek_cb`, a ranged request at the byte); this
//! module only decides when to try again and when to stop. While the demuxer waits here the queues
//! drain and the player's own progressive-stall hold shows Buffering.

use crate::curlio::OpenErr;
use plx_net::stream::redirect::FollowError;
use plx_net::stream::HttpOpenError;
use std::time::{Duration, Instant};

/// The wait unit. Tests shrink it so a real source can be driven through the whole schedule.
#[cfg(not(test))]
pub(super) const TICK: Duration = Duration::from_millis(500);
#[cfg(test)]
pub(super) const TICK: Duration = Duration::from_millis(1);

/// Ticks waited before each attempt at one byte: the first is immediate. 51 ticks, 25.5 s.
pub(super) const WAITS: [u32; 9] = [0, 1, 2, 4, 6, 8, 10, 10, 10];

/// No attempt starts after this much time since the first failure, whatever its own duration
/// (a dial can take its whole 15 s connect timeout).
const WALL: Duration = Duration::from_secs(30);

/// Reopens spent at one byte of a cut body, across calls: libavformat retries a failed read with a
/// fresh call, so a budget per call let a cut no reopen crosses be reopened round after round and
/// then read as the end of the file. A byte read refills it.
#[derive(Clone, Copy, Default)]
pub(super) struct HealBudget {
    at: i64,
    spent: u8,
    first: Option<Instant>,
    /// Set by the reopen when its refusal is one waiting cannot change (see [`status_is_permanent`]).
    pub(super) permanent: bool,
}

impl HealBudget {
    /// Charge one attempt at byte `at` made at `now`: how long to wait before it, or `None` when
    /// the budget at that byte is spent.
    fn next(&mut self, at: i64, now: Instant) -> Option<Duration> {
        if self.at != at {
            *self = HealBudget { at, ..HealBudget::default() };
        }
        let first = *self.first.get_or_insert(now);
        let left = WALL.checked_sub(now.saturating_duration_since(first)).filter(|left| !left.is_zero())?;
        let wait = TICK * *WAITS.get(usize::from(self.spent))?;
        self.spent += 1;
        Some(wait.min(left))
    }

    fn give_up(&mut self) {
        self.spent = WAITS.len() as u8;
    }

    /// Attempts charged so far.
    pub(super) fn attempts(&self) -> u8 {
        self.spent
    }
}

/// How one reopen ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reopen {
    /// The stream is back at the byte asked for.
    Back,
    /// A failure the next attempt may not repeat: a reset, a timeout, a 5xx.
    Transient,
    /// One it will: a 4xx (the file is gone, the sign-in refused), a server that ignores ranges,
    /// teardown.
    Permanent,
}

/// What [`heal`] needs of the stream it heals.
pub(super) trait Link {
    fn budget(&mut self) -> &mut HealBudget;
    /// One ranged reopen at `at`.
    fn reopen(&mut self, at: i64) -> Reopen;
    /// Log one attempt (`wait` is what was waited before it).
    fn note_attempt(&mut self, at: i64, wait: Duration);
    fn now(&self) -> Instant;
    /// Wait `d`; `false` when teardown cut the wait short.
    fn sleep(&mut self, d: Duration) -> bool;
}

/// Reopen the stream at byte `at`, retrying transient failures on the schedule. `true` when it is
/// back (the caller reads on), `false` when the budget ran out or the refusal is permanent (the
/// caller fails the read with the existing transport error).
pub(super) fn heal(link: &mut impl Link, at: i64) -> bool {
    loop {
        let now = link.now();
        let Some(wait) = link.budget().next(at, now) else {
            return false;
        };
        if !wait.is_zero() && !link.sleep(wait) {
            return false;
        }
        link.note_attempt(at, wait);
        match link.reopen(at) {
            Reopen::Back => return true,
            Reopen::Permanent => {
                link.budget().give_up();
                return false;
            }
            Reopen::Transient => {}
        }
    }
}

/// An HTTP status no later attempt changes: every 4xx but the ones that name a passing condition
/// (request timeout, too early, too many requests).
pub(super) fn status_is_permanent(status: i32) -> bool {
    (400..500).contains(&status) && !matches!(status, 408 | 425 | 429)
}

pub(super) fn open_err_is_permanent(e: &OpenErr) -> bool {
    match e {
        OpenErr::Status(s) => status_is_permanent(*s),
        OpenErr::RangeIgnored | OpenErr::Aborted | OpenErr::Unavailable | OpenErr::Local => true,
        _ => false,
    }
}

pub(super) fn follow_err_is_permanent(e: &FollowError) -> bool {
    match e {
        FollowError::Open(HttpOpenError::Status(s)) => status_is_permanent(*s),
        FollowError::Open(HttpOpenError::Aborted) => true,
        FollowError::Open(_) => false,
        _ => true,
    }
}

/// `CURLcode`s of a transfer that ended on the network rather than on the request: could not
/// resolve (6) or connect (7), HTTP/2 framing (16), partial file (18), operation timed out (28),
/// TLS connect (35), empty reply (52), send (55) and receive (56) failures, TLS close (92).
pub(super) fn curl_rc_is_transient(rc: i32) -> bool {
    matches!(rc, 6 | 7 | 16 | 18 | 28 | 35 | 52 | 55 | 56 | 92)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that answers the reopens in `answers` (then `Back`), on a virtual clock that a
    /// sleep advances; records each reopen's byte and the virtual time it was made at.
    struct Fake {
        budget: HealBudget,
        answers: Vec<Reopen>,
        tried: Vec<(i64, Duration)>,
        origin: Instant,
        elapsed: Duration,
    }

    impl Link for Fake {
        fn budget(&mut self) -> &mut HealBudget {
            &mut self.budget
        }
        fn reopen(&mut self, at: i64) -> Reopen {
            self.tried.push((at, self.elapsed));
            if self.answers.is_empty() {
                Reopen::Back
            } else {
                self.answers.remove(0)
            }
        }
        fn note_attempt(&mut self, _: i64, _: Duration) {}
        fn now(&self) -> Instant {
            self.origin + self.elapsed
        }
        fn sleep(&mut self, d: Duration) -> bool {
            self.elapsed += d;
            true
        }
    }

    fn rig(answers: Vec<Reopen>) -> Fake {
        Fake { budget: HealBudget::default(), answers, tried: vec![], origin: Instant::now(), elapsed: Duration::ZERO }
    }

    #[test]
    fn a_transient_failure_is_retried_with_growing_waits_and_resumes_at_the_same_byte() {
        let mut r = rig(vec![Reopen::Transient, Reopen::Transient, Reopen::Transient]);
        assert!(heal(&mut r, 343_827_223));
        let t = &r.tried;
        assert_eq!(t.iter().map(|(at, _)| *at).collect::<Vec<_>>(), [343_827_223; 4], "every attempt asks for the byte reached");
        assert_eq!(
            t.iter().map(|(_, at)| *at).collect::<Vec<_>>(),
            [0, 1, 3, 7].map(|n| TICK * n),
            "immediate, then 1, 2 and 4 ticks later"
        );
    }

    #[test]
    fn a_permanent_refusal_fails_at_once_and_stays_failed() {
        let mut r = rig(vec![Reopen::Permanent]);
        assert!(!heal(&mut r, 100));
        assert_eq!(r.tried.len(), 1, "no second attempt for a 404");
        assert!(!heal(&mut r, 100), "a later read at the same byte does not retry either");
        assert_eq!(r.tried.len(), 1);
    }

    #[test]
    fn a_server_that_never_answers_is_given_about_25_seconds_and_then_the_read_fails() {
        let mut r = rig(vec![Reopen::Transient; 100]);
        assert!(!heal(&mut r, 5));
        assert_eq!(r.tried.len(), WAITS.len());
        assert_eq!(r.elapsed, TICK * WAITS.iter().sum::<u32>());
        assert_eq!(r.elapsed.as_millis() * 500 / TICK.as_millis(), 25_500);
        assert!(!heal(&mut r, 5), "the spent budget holds across read calls");
        assert_eq!(r.tried.len(), WAITS.len());
    }

    #[test]
    fn slow_attempts_end_the_budget_by_the_clock_not_the_count() {
        let mut b = HealBudget::default();
        let t0 = Instant::now();
        assert!(b.next(7, t0).is_some());
        assert_eq!(b.next(7, t0 + WALL - TICK).map(|w| w <= TICK), Some(true), "the wait is cut to the time left");
        assert_eq!(b.next(7, t0 + WALL), None);
    }

    #[test]
    fn a_new_byte_starts_a_new_budget() {
        let mut b = HealBudget::default();
        let t0 = Instant::now();
        for _ in 0..WAITS.len() {
            assert!(b.next(7, t0).is_some());
        }
        assert_eq!(b.next(7, t0), None);
        assert_eq!(b.next(8, t0), Some(Duration::ZERO));
    }

    #[test]
    fn only_a_refusal_that_waiting_cannot_change_is_permanent() {
        for s in [400, 401, 403, 404, 410, 416] {
            assert!(status_is_permanent(s), "{s}");
        }
        for s in [408, 425, 429, 500, 502, 503, 504, 200, 206] {
            assert!(!status_is_permanent(s), "{s}");
        }
        assert!(open_err_is_permanent(&OpenErr::Status(404)));
        assert!(open_err_is_permanent(&OpenErr::RangeIgnored));
        assert!(!open_err_is_permanent(&OpenErr::Status(503)));
        assert!(!open_err_is_permanent(&OpenErr::Transport(56)));
        assert!(follow_err_is_permanent(&FollowError::Open(HttpOpenError::Status(403))));
        assert!(!follow_err_is_permanent(&FollowError::Open(HttpOpenError::Status(502))));
        assert!(!follow_err_is_permanent(&FollowError::Open(HttpOpenError::Transport)));
        assert!(curl_rc_is_transient(56) && curl_rc_is_transient(28) && !curl_rc_is_transient(23));
    }
}
