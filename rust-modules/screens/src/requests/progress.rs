//! Live progress on the viewer's own requests: which of them are in an app's download queue, and
//! how far along. One list call per kind, one outstanding at a time, and only while the page is
//! on show and a poster of that kind is approved or downloading. What a poll cannot confirm is
//! dropped after [`STALE_MS`], so a poster never keeps a percentage nobody has reported lately.
use plx_plex::plex::progress::{Fail, Live, Stage};
use plx_plex::plex::seerr::{Badge, Kind};
use std::borrow::Cow;

/// Between lists while something is in the queue.
const BUSY_EVERY_MS: u32 = 4_000;
/// Between lists while the queue holds none of the titles.
const IDLE_EVERY_MS: u32 = 10_000;
/// The driver had a call out: ask again shortly.
const BUSY_MS: u32 = 500;
/// Not linked or turned away: look again now and then, in case Settings changed.
const NOT_SET_MS: u32 = 60_000;
/// After a failed list: wait longer each time.
const RETRY_MS: [u32; 5] = [5_000, 10_000, 20_000, 40_000, 60_000];
/// A call with no answer this long is written off.
const LOST_MS: u32 = 40_000;
/// A chip older than this is dropped.
const STALE_MS: u32 = 20_000;

/// What a poster's chip says of a title in a download queue.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Chip {
    stage: Stage,
    percent: Option<u8>,
    pub word: Cow<'static, str>,
}

/// The Seerr states a live queue can refine, never overriding a ready or declined title.
pub(super) fn follows(badge: Badge) -> bool {
    matches!(badge, Badge::Approved | Badge::Downloading | Badge::PartlyAvailable)
}

fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Movie => 0,
        Kind::Tv => 1,
    }
}

const KINDS: [Kind; 2] = [Kind::Movie, Kind::Tv];

/// The list poll's state. Time comes in through [`Poll::set_now`] (the page's tick), so a test
/// moves a fake clock instead of sleeping.
#[derive(Default)]
pub(super) struct Poll {
    now: u32,
    visible: bool,
    /// Every kind is due the moment the page shows again.
    wake: bool,
    /// The kind asked for and when.
    out: Option<(Kind, u32)>,
    gen: u32,
    due: [u32; 2],
    /// When each kind's last good answer came.
    heard: [Option<u32>; 2],
    fails: [usize; 2],
    /// The kind asked first next, so neither starves the other.
    turn: usize,
    chips: [Vec<(u32, Chip)>; 2],
}

impl Poll {
    pub fn set_now(&mut self, ms: u32) {
        self.now = ms;
    }

    pub fn show(&mut self) {
        if !self.visible {
            self.visible = true;
            self.wake = true;
            // Language is set on a page that covers this one.
            for (_, chip) in self.chips.iter_mut().flatten() {
                chip.word = super::progress_text::short(chip.stage, chip.percent);
            }
        }
    }

    /// Stop asking and retire any unanswered list while the page is covered.
    pub fn hide(&mut self) {
        self.visible = false;
        self.out = None;
    }

    /// The chip of `kind`'s title `tmdb`, if its app's queue holds it.
    pub fn chip(&self, kind: Kind, tmdb: u32) -> Option<&Chip> {
        self.chips[slot(kind)].iter().find(|(id, _)| *id == tmdb).map(|(_, c)| c)
    }

    fn due(&self, i: usize) -> bool {
        (self.now.wrapping_sub(self.due[i]) as i32) >= 0
    }

    /// The kind to list now, if any; `wanted` says whether a poster of that kind is worth a call.
    pub fn advance(&mut self, wanted: impl Fn(Kind) -> bool) -> Option<(Kind, u32)> {
        if self.wake {
            self.wake = false;
            self.due = [self.now; 2];
        }
        if !self.visible {
            return None;
        }
        if let Some((kind, at)) = self.out {
            if self.now.wrapping_sub(at) < LOST_MS {
                return None;
            }
            self.out = None;
            self.back_off(slot(kind));
        }
        let kind = (0..2).map(|n| (self.turn + n) % 2).find(|&i| self.due(i) && wanted(KINDS[i])).map(|i| KINDS[i])?;
        self.out = Some((kind, self.now));
        self.turn = 1 - slot(kind);
        self.gen = self.gen.wrapping_add(1);
        Some((kind, self.gen))
    }

    fn back_off(&mut self, i: usize) {
        let wait = RETRY_MS[self.fails[i].min(RETRY_MS.len() - 1)];
        self.fails[i] += 1;
        self.due[i] = self.now.wrapping_add(wait);
    }

    /// Take one list. `true` when a chip changed.
    pub fn land(&mut self, gen: u32, kind: Kind, result: Result<Vec<Live>, Fail>) -> bool {
        if gen != self.gen || self.out.is_none_or(|(k, _)| k != kind) {
            return false;
        }
        self.out = None;
        let i = slot(kind);
        match result {
            Ok(list) => {
                self.fails[i] = 0;
                self.heard[i] = Some(self.now);
                self.due[i] = self.now.wrapping_add(if list.is_empty() { IDLE_EVERY_MS } else { BUSY_EVERY_MS });
                self.replace(i, &list)
            }
            Err(Fail::Busy) => {
                self.due[i] = self.now.wrapping_add(BUSY_MS);
                false
            }
            Err(Fail::NotSet | Fail::Refused) => {
                self.due[i] = self.now.wrapping_add(NOT_SET_MS);
                self.heard[i] = None;
                self.replace(i, &[])
            }
            Err(Fail::Failed) => {
                self.back_off(i);
                false
            }
        }
    }

    /// Drop the chips of a kind whose last good answer is older than [`STALE_MS`]. `true` when any went.
    pub fn expire(&mut self) -> bool {
        let mut changed = false;
        for i in 0..KINDS.len() {
            if self.heard[i].is_some_and(|at| self.now.wrapping_sub(at) >= STALE_MS) {
                self.heard[i] = None;
                changed |= self.replace(i, &[]);
            }
        }
        changed
    }

    fn replace(&mut self, i: usize, list: &[Live]) -> bool {
        let chips = &mut self.chips[i];
        let incoming = list.iter().map(|l| (l.tmdb, l.sum.stage, l.sum.percent));
        if chips.iter().map(|(tmdb, c)| (*tmdb, c.stage, c.percent)).eq(incoming.clone()) {
            return false;
        }
        chips.clear();
        chips.extend(incoming.map(|(tmdb, stage, percent)| {
            (tmdb, Chip { stage, percent, word: super::progress_text::short(stage, percent) })
        }));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_plex::plex::progress::Sum;

    fn live(tmdb: u32, percent: u8) -> Live {
        Live { tmdb, sum: Sum { stage: Stage::Downloading, percent: Some(percent), eta_s: None } }
    }

    fn polling() -> Poll {
        let mut poll = Poll::default();
        poll.show();
        poll
    }

    fn ask(poll: &mut Poll, wanted: impl Fn(Kind) -> bool) -> Option<Kind> {
        poll.advance(wanted).map(|(kind, _)| kind)
    }

    #[test]
    fn one_list_is_out_at_a_time_and_the_kinds_take_turns() {
        let mut poll = polling();
        let (kind, gen) = poll.advance(|_| true).unwrap();
        assert_eq!(kind, Kind::Movie);
        assert_eq!(poll.advance(|_| true), None, "the first is still out");
        poll.land(gen, kind, Ok(vec![live(7, 40)]));
        assert_eq!(ask(&mut poll, |_| true), Some(Kind::Tv), "the other kind is due and next");
    }

    #[test]
    fn nothing_is_asked_while_hidden_or_while_no_poster_wants_it() {
        let mut poll = polling();
        let (kind, gen) = poll.advance(|k| k == Kind::Tv).unwrap();
        assert_eq!(kind, Kind::Tv);
        poll.land(gen, kind, Ok(Vec::new()));
        poll.hide();
        poll.set_now(60_000);
        assert_eq!(poll.advance(|_| true), None);
        poll.show();
        assert_eq!(poll.advance(|_| false), None);
    }

    #[test]
    fn old_replies_cannot_overwrite_a_new_list_on_return() {
        let mut poll = polling();
        let (kind, old) = poll.advance(|k| k == Kind::Movie).unwrap();
        poll.hide();
        poll.show();
        let (_, current) = poll.advance(|k| k == Kind::Movie).unwrap();
        assert!(!poll.land(old, kind, Ok(vec![live(7, 10)])));
        assert!(poll.land(current, kind, Ok(vec![live(7, 40)])));
        assert_eq!(poll.chip(kind, 7).unwrap().percent, Some(40));
    }

    #[test]
    fn a_chip_follows_the_latest_list_and_a_gone_title_loses_it() {
        let mut poll = polling();
        let (_, gen) = poll.advance(|_| true).unwrap();
        assert!(poll.land(gen, Kind::Movie, Ok(vec![live(7, 40)])));
        assert_eq!(poll.chip(Kind::Movie, 7).unwrap().percent, Some(40));
        assert_eq!(poll.chip(Kind::Tv, 7), None, "a movie and a show with one id are two titles");
        poll.set_now(4_000);
        let (_, gen) = poll.advance(|k| k == Kind::Movie).unwrap();
        assert!(!poll.land(gen, Kind::Movie, Ok(vec![live(7, 40)])), "the same list changes nothing");
        poll.set_now(8_000);
        let (_, gen) = poll.advance(|k| k == Kind::Movie).unwrap();
        assert!(poll.land(gen, Kind::Movie, Ok(Vec::new())));
        assert_eq!(poll.chip(Kind::Movie, 7), None);
    }

    #[test]
    fn a_list_comes_every_few_seconds_while_the_queue_holds_something() {
        let mut poll = polling();
        poll.advance(|k| k == Kind::Movie);
        let gen = poll.gen;
        poll.land(gen, Kind::Movie, Ok(vec![live(7, 40)]));
        poll.set_now(BUSY_EVERY_MS - 1);
        assert_eq!(poll.advance(|k| k == Kind::Movie), None);
        poll.set_now(BUSY_EVERY_MS);
        assert_eq!(ask(&mut poll, |k| k == Kind::Movie), Some(Kind::Movie));
    }

    #[test]
    fn a_busy_driver_is_asked_again_shortly_and_a_failure_backs_off() {
        let mut poll = polling();
        poll.advance(|k| k == Kind::Movie);
        let gen = poll.gen;
        poll.land(gen, Kind::Movie, Err(Fail::Busy));
        poll.set_now(BUSY_MS);
        assert_eq!(ask(&mut poll, |k| k == Kind::Movie), Some(Kind::Movie));
        let gen = poll.gen;
        poll.land(gen, Kind::Movie, Err(Fail::Failed));
        poll.set_now(BUSY_MS + RETRY_MS[0] - 1);
        assert_eq!(poll.advance(|k| k == Kind::Movie), None);
        poll.set_now(BUSY_MS + RETRY_MS[0]);
        assert_eq!(ask(&mut poll, |k| k == Kind::Movie), Some(Kind::Movie));
        let gen = poll.gen;
        poll.land(gen, Kind::Movie, Err(Fail::Failed));
        poll.set_now(BUSY_MS + RETRY_MS[0] + RETRY_MS[1] - 1);
        assert_eq!(poll.advance(|k| k == Kind::Movie), None, "the second wait is longer");
    }

    #[test]
    fn a_call_that_never_answers_is_written_off_and_retried_later() {
        let mut poll = polling();
        poll.advance(|k| k == Kind::Movie);
        poll.set_now(LOST_MS - 1);
        assert_eq!(poll.advance(|k| k == Kind::Movie), None);
        poll.set_now(LOST_MS);
        assert_eq!(poll.advance(|k| k == Kind::Movie), None, "backed off first");
        poll.set_now(LOST_MS + RETRY_MS[0]);
        assert_eq!(ask(&mut poll, |k| k == Kind::Movie), Some(Kind::Movie));
    }

    #[test]
    fn a_chip_nobody_has_confirmed_lately_is_dropped() {
        let mut poll = polling();
        poll.advance(|_| true);
        let gen = poll.gen;
        poll.land(gen, Kind::Movie, Ok(vec![live(7, 40)]));
        poll.set_now(STALE_MS - 1);
        assert!(!poll.expire());
        poll.set_now(STALE_MS);
        assert!(poll.expire());
        assert_eq!(poll.chip(Kind::Movie, 7), None);
        assert!(!poll.expire(), "dropped once");
    }

    #[test]
    fn an_app_that_is_not_linked_shows_nothing_and_is_looked_at_again_in_a_minute() {
        let mut poll = polling();
        poll.advance(|k| k == Kind::Tv);
        let gen = poll.gen;
        assert!(!poll.land(gen, Kind::Tv, Err(Fail::NotSet)));
        poll.set_now(NOT_SET_MS - 1);
        assert_eq!(poll.advance(|k| k == Kind::Tv), None);
        poll.set_now(NOT_SET_MS);
        assert_eq!(ask(&mut poll, |k| k == Kind::Tv), Some(Kind::Tv));
    }
}
