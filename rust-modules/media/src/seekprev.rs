//! **Seek previews made on the TV**, for a playback whose server keeps no preview thumbnails: the
//! scrub card (`appkit::scrub_preview`) finds every server still missing, and this module decodes
//! stills from the film itself instead.
//!
//! - **Which films**: H.264 or HEVC at 1080p or below ([`eligible`]), from a server on the local
//!   network (`route::seek_preview_eligible`). A 4K picture costs several times a 1080p one to
//!   decode on the TV's CPU, so 4K keeps the server's stills only.
//! - **When**: once the playback's scrub card has found no server stills (the HUD's memo, which the
//!   app hands to [`pump`] each frame), and only while the playback's buffer is healthy ([`Gate`]).
//!   A server that refuses the reads is asked again after a growing pause, and not at all for that
//!   film after [`REFUSALS_TO_STOP`] in a row ([`Backoff`]).
//! - **What**: a ring of [`SLOTS`] stills ([`Plan`]): a coarse spread over the whole film first,
//!   then points around the live scrub target. A batch opens one demuxer and one software decoder
//!   on a thread of its own (`ff::seekprev`), decodes the keyframe at or before each point, shrinks
//!   it to the card's size and closes both; one batch runs at a time. The pictures reach the HUD's
//!   textures through [`take`].
//! - **Memory**: [`rest_for_memory`] stops the work and drops every still while memory is short.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::ff::seekprev::{BatchEnd, Reader, Still};
use crate::ff::subside::SideStop;

/// Stills held at once: the HUD keeps one texture of up to 320×180 per slot (5.5 MB for the ring).
pub const SLOTS: usize = 24;
/// Every third point of the coarse spread is an anchor: asked first and never displaced, so a scrub
/// anywhere in the film has a still within an eighth of the film.
const ANCHOR_EVERY: usize = 3;
/// The lattice stills around a scrub target snap to. A still within half a step of a point stands
/// for it.
const STEP_MS: i64 = 10_000;
/// A scrub target with no still this near is asked before anything else: the card's own tolerance
/// (`appkit::scrub_preview::STILL_TOLERANCE_MS`).
const NEAR_MS: i64 = 10_000;
/// Pictures one batch makes before its decoder and demuxer are closed.
const BATCH_JOBS: usize = 8;

/// The reserve under which the gate shuts. A ≤1080p film's queues hold at least ~3 s when full (the
/// 10 MiB video lane at Blu-ray's 48 Mbps ceiling, the 1 MiB audio lane at a lossless 6 Mbps), so a
/// reserve under this is draining, not full at a high rate.
const RESERVE_FLOOR_MS: i64 = 2_000;
/// No work this long after the playback settles (a start, a seek, a rebuffer): it refills first.
const SETTLE_QUIET_MS: u32 = 3_000;
/// No work this long after the reserve fell under the floor while it had been steady.
const DIP_QUIET_MS: u32 = 10_000;
/// How long the reserve must hold before work starts.
const STEADY_MS: u32 = 2_000;

/// The pause after a refusal, doubled by each further refusal in a row up to the ceiling.
const BACKOFF_FIRST_MS: u32 = 2_000;
const BACKOFF_CEIL_MS: u32 = 30_000;
/// Refusals in a row after which the film gets no more generated stills.
const REFUSALS_TO_STOP: u32 = 5;

/// Is a source of this size and codec decoded on the TV? H.264 and HEVC only, the decoders the
/// bundled FFmpeg carries for this, at 1080p or below (1088 is 1080p's coded height). An unknown
/// size (0) qualifies for nothing.
pub fn eligible(w: i64, h: i64, vcodec: &str) -> bool {
    (1..=1920).contains(&w)
        && (1..=1088).contains(&h)
        && (vcodec.eq_ignore_ascii_case("h264") || vcodec.eq_ignore_ascii_case("hevc"))
}

// ---- the ring's schedule ---------------------------------------------------------------------

/// A point to decode and the ring slot its picture goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Job {
    slot: u8,
    want_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    /// Being decoded.
    Flight,
    /// In the HUD's ring, decoded from the keyframe at this movie ms.
    Have(i64),
    /// No picture came of it; kept so the point is not asked again.
    Failed,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    slot: u8,
    want_ms: i64,
    held: Held,
    /// The coarse spread's point this was asked for, if any.
    coarse: Option<u8>,
}

impl Entry {
    fn anchor(&self) -> bool {
        self.coarse.is_some_and(|j| j as usize % ANCHOR_EVERY == 1)
    }

    /// How far the still is from `ms`: where its keyframe landed, else the point asked.
    fn dist(&self, ms: i64) -> i64 {
        match self.held {
            Held::Have(at) => (at - ms).abs(),
            _ => (self.want_ms - ms).abs(),
        }
    }
}

/// **Which stills to make, in what order, into which slot.** Pure; the batch thread and the pump
/// share it under [`GEN`]'s lock.
///
/// The coarse spread is [`SLOTS`] points at the middles of equal spans of the film, anchors first
/// ([`coarse_point`]); a point that finds the ring full is skipped. A live scrub target with no
/// still within [`NEAR_MS`] is asked before the spread, snapped to [`STEP_MS`]; once the spread is
/// done, the lattice points around the target are. A point near the target that finds the ring full
/// takes the slot of the still farthest from the target, if that still is farther than the point,
/// is not in flight and is not an anchor.
#[derive(Clone, Debug, Default)]
struct Plan {
    dur_ms: i64,
    entries: Vec<Entry>,
    /// Bit `j`: the spread's point `j` has been asked (cleared again when a stop lost it).
    asked: u32,
}

/// The `i`th coarse point to ask: the anchors, then the point before each, then the one after.
fn coarse_point(i: usize) -> usize {
    let per = SLOTS / ANCHOR_EVERY;
    [1, 0, 2][i / per] + (i % per) * ANCHOR_EVERY
}

fn snap(ms: i64) -> i64 {
    (ms + STEP_MS / 2).div_euclid(STEP_MS) * STEP_MS
}

impl Plan {
    const fn new() -> Plan {
        Plan { dur_ms: 0, entries: Vec::new(), asked: 0 }
    }

    /// The film's length, which the spread is laid over. The first known length stands.
    fn set_duration(&mut self, dur_ms: i64) {
        if self.dur_ms <= 0 {
            self.dur_ms = dur_ms.max(0);
        }
    }

    fn grid_ms(&self, j: usize) -> i64 {
        self.dur_ms * (2 * j as i64 + 1) / (2 * SLOTS as i64)
    }

    fn covered(&self, ms: i64, within: i64) -> bool {
        self.entries.iter().any(|e| (e.want_ms - ms).abs() <= within || e.dist(ms) <= within)
    }

    fn free_slot(&self) -> Option<u8> {
        (0..SLOTS as u8).find(|s| self.entries.iter().all(|e| e.slot != *s))
    }

    /// The next still to make while the scrub target is `focus` (movie ms, `None` with no live
    /// scrub), recorded as in flight; `None` when nothing is worth making now.
    fn next(&mut self, focus: Option<i64>) -> Option<Job> {
        if self.dur_ms <= 0 {
            return None;
        }
        let last = self.dur_ms - 1;
        if let Some(f) = focus {
            if !self.covered(f, NEAR_MS) {
                if let Some(job) = self.near(snap(f).clamp(0, last), f) {
                    return Some(job);
                }
            }
        }
        for i in 0..SLOTS {
            let j = coarse_point(i);
            if self.asked & (1 << j) != 0 {
                continue;
            }
            self.asked |= 1 << j;
            let want_ms = self.grid_ms(j);
            if self.covered(want_ms, STEP_MS / 2) {
                continue;
            }
            if let Some(slot) = self.free_slot() {
                self.entries.push(Entry { slot, want_ms, held: Held::Flight, coarse: Some(j as u8) });
                return Some(Job { slot, want_ms });
            }
        }
        let f = focus?;
        for k in [0, 1, -1, 2, -2] {
            let ms = snap(f) + k * STEP_MS;
            if (0..=last).contains(&ms) && !self.covered(ms, STEP_MS / 2) {
                if let Some(job) = self.near(ms, f) {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Ask `want_ms`, for the scrub target `focus`, in a free slot or the slot of the still that
    /// may go farthest from the target.
    fn near(&mut self, want_ms: i64, focus: i64) -> Option<Job> {
        let slot = match self.free_slot() {
            Some(s) => s,
            None => {
                let (i, far) = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.held != Held::Flight && !e.anchor())
                    .map(|(i, e)| (i, e.dist(focus)))
                    .max_by_key(|(_, d)| *d)?;
                if far <= (want_ms - focus).abs() {
                    return None;
                }
                self.entries.swap_remove(i).slot
            }
        };
        self.entries.push(Entry { slot, want_ms, held: Held::Flight, coarse: None });
        Some(Job { slot, want_ms })
    }

    fn flight(&mut self, slot: u8) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.slot == slot && e.held == Held::Flight)
    }

    /// `slot`'s picture arrived, decoded at `at_ms`. `false` when the ring already holds that
    /// keyframe (two points in one group of pictures): the point then counts as tried.
    fn land(&mut self, slot: u8, at_ms: i64) -> bool {
        let fresh = self.entries.iter().all(|e| e.held != Held::Have(at_ms));
        if let Some(e) = self.flight(slot) {
            e.held = if fresh { Held::Have(at_ms) } else { Held::Failed };
        }
        fresh
    }

    /// No picture came of `slot`'s point.
    fn fail(&mut self, slot: u8) {
        if let Some(e) = self.flight(slot) {
            e.held = Held::Failed;
        }
    }

    /// `slot`'s point was never decoded (its batch ended first): forget it, so it is asked again.
    fn cancel(&mut self, slot: u8) {
        if let Some(i) = self.entries.iter().position(|e| e.slot == slot && e.held == Held::Flight) {
            if let Some(j) = self.entries.swap_remove(i).coarse {
                self.asked &= !(1 << j);
            }
        }
    }
}

// ---- the buffer gate and the back-off ----------------------------------------------------------

/// What the gate reads each frame.
#[derive(Clone, Copy, Debug)]
struct Health {
    /// Playing: not starting, seeking or rebuffering.
    settled: bool,
    /// The playable reserve ahead of the playhead, `None` while it is not knowable.
    reserve_ms: Option<i64>,
}

/// `now` is at or past `t` on the wrapping millisecond clock.
fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

/// **May a batch run now?** Only on a settled playback whose reserve has stood at
/// [`RESERVE_FLOOR_MS`] or more for [`STEADY_MS`], [`SETTLE_QUIET_MS`] after it settled and
/// [`DIP_QUIET_MS`] after the reserve last fell under the floor.
#[derive(Clone, Copy, Debug, Default)]
struct Gate {
    quiet_until: Option<u32>,
    steady_since: Option<u32>,
}

impl Gate {
    const fn new() -> Gate {
        Gate { quiet_until: None, steady_since: None }
    }

    fn step(&mut self, h: Health, now: u32) -> bool {
        if !h.settled {
            self.steady_since = None;
            self.quiet_until = Some(now.wrapping_add(SETTLE_QUIET_MS));
            return false;
        }
        if !h.reserve_ms.is_some_and(|r| r >= RESERVE_FLOOR_MS) {
            if self.steady_since.take().is_some() {
                self.quiet_until = Some(now.wrapping_add(DIP_QUIET_MS));
            }
            return false;
        }
        if self.quiet_until.is_some_and(|q| !reached(now, q)) {
            return false;
        }
        self.quiet_until = None;
        let since = *self.steady_since.get_or_insert(now);
        reached(now, since.wrapping_add(STEADY_MS))
    }
}

/// **The pause after the server refused a batch's read**: [`BACKOFF_FIRST_MS`], doubling with each
/// refusal in a row up to [`BACKOFF_CEIL_MS`]; [`REFUSALS_TO_STOP`] in a row end the film's stills.
/// A batch the server served resets it.
#[derive(Clone, Copy, Debug, Default)]
struct Backoff {
    refusals: u32,
    until: Option<u32>,
}

impl Backoff {
    const fn new() -> Backoff {
        Backoff { refusals: 0, until: None }
    }

    fn refused(&mut self, now: u32) {
        self.refusals += 1;
        let wait = BACKOFF_FIRST_MS.saturating_mul(1 << (self.refusals - 1).min(15)).min(BACKOFF_CEIL_MS);
        self.until = Some(now.wrapping_add(wait));
    }

    fn served(&mut self) {
        *self = Backoff::new();
    }

    fn gave_up(&self) -> bool {
        self.refusals >= REFUSALS_TO_STOP
    }

    fn ready(&self, now: u32) -> bool {
        !self.gave_up() && self.until.is_none_or(|u| reached(now, u))
    }
}

// ---- the generator ---------------------------------------------------------------------------

/// What the HUD reads of the generator each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feed {
    /// Changes whenever every still made so far is void (another playback, memory short): the HUD
    /// then drops its textures.
    pub epoch: u32,
    /// This playback gets stills made here when its server has none.
    pub on: bool,
}

/// One still for the HUD's ring.
#[derive(Debug)]
pub struct Picture {
    /// The [`Feed::epoch`] it was made in.
    pub epoch: u32,
    pub slot: u8,
    /// The movie time of the keyframe it was decoded from.
    pub at_ms: i64,
    pub w: i32,
    pub h: i32,
    /// `w`×`h` RGBA, tightly packed.
    pub rgba: Vec<u8>,
}

struct Gen {
    epoch: u32,
    /// The playback this describes: (Part id, playback generation).
    owner: Option<(i64, String)>,
    /// The playback qualifies (`route::seek_preview_eligible`), as of the last pump.
    eligible: bool,
    /// The Part's URL, built when work first starts. Carries the token; never logged.
    url: Option<Arc<str>>,
    plan: Plan,
    gate: Gate,
    backoff: Backoff,
    /// The film cannot be decoded here (no such decoder, a picture format the shrink does not read).
    unusable: bool,
    /// Memory is short ([`rest_for_memory`]).
    short: bool,
    /// The stop of this epoch's running batch.
    running: Option<Arc<SideStop>>,
    /// A batch thread of any epoch is alive; the next batch waits for it to end.
    live: bool,
    /// How this epoch's last batch ended, for the next pump to account.
    ended: Option<BatchEnd>,
    /// Pictures not yet taken by the HUD, at most one per slot.
    outbox: Vec<Picture>,
}

impl Gen {
    const fn new() -> Gen {
        Gen {
            epoch: 0,
            owner: None,
            eligible: false,
            url: None,
            plan: Plan::new(),
            gate: Gate::new(),
            backoff: Backoff::new(),
            unusable: false,
            short: false,
            running: None,
            live: false,
            ended: None,
            outbox: Vec::new(),
        }
    }

    fn on(&self) -> bool {
        self.eligible && !self.unusable && !self.backoff.gave_up() && !self.short
    }

    /// Void everything made or asked so far. `short` and `live` describe the process, not the
    /// playback, and stay.
    fn reset(&mut self) {
        if let Some(stop) = self.running.take() {
            stop.trigger();
        }
        *self = Gen { epoch: self.epoch.wrapping_add(1), short: self.short, live: self.live, ..Gen::new() };
    }

    fn account(&mut self, end: BatchEnd, now: u32) {
        match end {
            BatchEnd::Done => self.backoff.served(),
            BatchEnd::Stopped => {}
            BatchEnd::Refused => {
                self.backoff.refused(now);
                if self.backoff.gave_up() {
                    plx_base::eventlog::log(&format!("seekprev: {REFUSALS_TO_STOP} refusals in a row; no more stills for this film"));
                }
            }
            BatchEnd::Unusable => {
                self.unusable = true;
                plx_base::eventlog::log("seekprev: the film's video cannot be decoded here; no stills");
            }
        }
    }
}

static GEN: Mutex<Gen> = Mutex::new(Gen::new());

fn lock() -> MutexGuard<'static, Gen> {
    GEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The live scrub's target in movie ms, `None` while no scrub is live.
fn focus_ms() -> Option<i64> {
    let ns = crate::player::TX.scrub_ns.load(Ordering::Relaxed);
    (ns >= 0).then_some(ns / 1_000_000)
}

/// **The generator's frame step**, on the main thread. `absent`: the player's scrub card has found
/// that this playback's server keeps no stills. Starts a batch when one is due; stops the running
/// one when the gate shuts or the stills are no longer wanted.
pub fn pump(ps: &crate::route::PlaybackSession, absent: bool, now: u32) {
    let part = crate::route::cur_part_id(ps);
    let sess = crate::route::sess_str(ps);
    let mut g = lock();
    if part <= 0 || sess.is_empty() {
        if g.owner.is_some() {
            g.reset();
        }
        return;
    }
    if g.owner.as_ref().is_none_or(|(p, s)| *p != part || s != sess) {
        g.reset();
        g.owner = Some((part, sess.to_owned()));
    }
    g.eligible = crate::route::seek_preview_eligible(ps);
    if let Some(end) = g.ended.take() {
        g.account(end, now);
    }
    let settled = crate::player::state(ps) == crate::player::PlaybackState::Playing && !crate::player::rebuffering();
    let open = g.gate.step(Health { settled, reserve_ms: crate::ff::playable_reserve_ms() }, now);
    if !absent || !g.on() || !open {
        if let Some(stop) = g.running.take() {
            stop.trigger();
        }
        return;
    }
    if g.live || !g.backoff.ready(now) {
        return;
    }
    if g.url.is_none() {
        let Some(url) = crate::route::seek_preview_url(ps) else {
            g.unusable = true;
            return;
        };
        g.url = Some(Arc::from(url));
        plx_base::eventlog::log("seekprev: the server keeps no preview thumbnails; making stills here");
    }
    g.plan.set_duration(crate::player::duration_ns() / 1_000_000);
    if let Some(first) = g.plan.next(focus_ms()) {
        start(&mut g, first);
    }
}

fn start(g: &mut Gen, first: Job) {
    let Some(url) = g.url.clone() else { return };
    let epoch = g.epoch;
    let stop = SideStop::new();
    let thread_stop = Arc::clone(&stop);
    g.live = true;
    if plx_base::task::spawn("seekprev", move || run(epoch, &url, first, &thread_stop)).is_none() {
        g.live = false;
        g.plan.cancel(first.slot);
        g.ended = Some(BatchEnd::Refused);
        return;
    }
    g.running = Some(stop);
}

/// Clears [`Gen::live`] when the batch thread ends, by return or by unwinding.
struct Alive;

impl Drop for Alive {
    fn drop(&mut self) {
        lock().live = false;
    }
}

fn run(epoch: u32, url: &str, first: Job, stop: &Arc<SideStop>) {
    let _alive = Alive;
    let end = batch(epoch, url, first, stop);
    let mut g = lock();
    if g.epoch == epoch {
        g.running = None;
        g.ended = Some(end);
    }
}

/// One batch: the demuxer and decoder are opened for it alone and closed (the reader's drop)
/// before it reports.
fn batch(epoch: u32, url: &str, first: Job, stop: &Arc<SideStop>) -> BatchEnd {
    // SAFETY: the reader is opened, used and dropped on this thread alone.
    let mut reader = match unsafe { Reader::open(url, stop) } {
        Ok(r) => r,
        Err(end) => {
            settle(epoch, first, None, false);
            return end;
        }
    };
    let mut job = first;
    for made in 1..=BATCH_JOBS {
        // SAFETY: as above.
        match unsafe { reader.grab(job.want_ms) } {
            Ok(still) => settle(epoch, job, still, true),
            Err(end) => {
                settle(epoch, job, None, false);
                return end;
            }
        }
        if made == BATCH_JOBS || stop.is_set() {
            break;
        }
        let mut g = lock();
        if g.epoch != epoch || g.short {
            break;
        }
        match g.plan.next(focus_ms()) {
            Some(next) => job = next,
            None => break,
        }
    }
    BatchEnd::Done
}

/// Record how `job` went: a picture (`still`), no picture (`tried`), or not tried at all.
fn settle(epoch: u32, job: Job, still: Option<Still>, tried: bool) {
    let mut g = lock();
    if g.epoch != epoch {
        return;
    }
    match still {
        Some(s) => {
            if g.plan.land(job.slot, s.at_ms) {
                g.outbox.retain(|p| p.slot != job.slot);
                g.outbox.push(Picture { epoch, slot: job.slot, at_ms: s.at_ms, w: s.w, h: s.h, rgba: s.rgba });
                drop(g);
                plx_machine::idle::invalidate();
            }
        }
        None if tried => g.plan.fail(job.slot),
        None => g.plan.cancel(job.slot),
    }
}

/// The generator as the HUD sees it this frame.
pub fn feed() -> Feed {
    let g = lock();
    Feed { epoch: g.epoch, on: g.on() }
}

/// One picture made since the HUD last took one, `None` when there is none.
pub fn take() -> Option<Picture> {
    lock().outbox.pop()
}

/// The memory guard's hook: while memory is `short`, stop the running batch, drop every still
/// (the HUD frees its textures on the epoch change) and make none.
pub fn rest_for_memory(short: bool) {
    let mut g = lock();
    if short && !g.short {
        g.reset();
    }
    g.short = short;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 minutes: the spread's points every 100 s, from 50 s.
    const DUR: i64 = 2_400_000;

    fn plan() -> Plan {
        let mut p = Plan::new();
        p.set_duration(DUR);
        p
    }

    /// Ask and land stills until the plan has nothing more for `focus`; the points asked, in order.
    fn drain(p: &mut Plan, focus: Option<i64>) -> Vec<i64> {
        let mut asked = Vec::new();
        while let Some(j) = p.next(focus) {
            p.land(j.slot, j.want_ms);
            asked.push(j.want_ms);
        }
        asked
    }

    #[test]
    fn the_spread_lays_its_anchors_first_then_fills_between_them() {
        let asked = drain(&mut plan(), None);
        assert_eq!(asked.len(), SLOTS);
        assert_eq!(asked[..3], [150_000, 450_000, 750_000]);
        assert_eq!(asked[8..10], [50_000, 350_000]);
        assert_eq!(asked[16..18], [250_000, 550_000]);
    }

    #[test]
    fn a_scrub_target_is_asked_first_and_then_refined_around() {
        let mut p = plan();
        p.next(None).unwrap();
        assert_eq!(p.next(Some(1_234_000)).map(|j| j.want_ms), Some(1_230_000), "before the rest of the spread");
        drain(&mut p, None);
        assert_eq!(drain(&mut p, Some(1_234_000)), [1_240_000, 1_220_000, 1_210_000]);
    }

    #[test]
    fn a_full_ring_gives_up_the_farthest_still_but_never_an_anchor() {
        let mut p = plan();
        drain(&mut p, None);
        assert_eq!(p.next(Some(1_234_000)).map(|j| j.want_ms), Some(1_230_000));
        assert!(p.entries.iter().all(|e| e.want_ms != 50_000), "the still farthest from the target went");
        for f in (0..DUR).step_by(97_000) {
            drain(&mut p, Some(f));
        }
        let anchors = p.entries.iter().filter(|e| e.anchor() && matches!(e.held, Held::Have(_))).count();
        assert_eq!((anchors, p.entries.len()), (SLOTS / ANCHOR_EVERY, SLOTS));
    }

    #[test]
    fn a_point_lost_to_a_stop_is_asked_again_and_a_failed_one_is_not() {
        let mut p = plan();
        let lost = p.next(None).unwrap();
        p.cancel(lost.slot);
        assert_eq!(p.next(None), Some(lost));
        let failed = p.next(None).unwrap();
        p.fail(failed.slot);
        assert!(drain(&mut p, None).iter().all(|ms| *ms != failed.want_ms));
    }

    #[test]
    fn only_h264_and_hevc_at_1080p_or_below_qualify() {
        assert!(eligible(1920, 1080, "h264"));
        assert!(eligible(1280, 720, "HEVC"));
        assert!(!eligible(3840, 2160, "hevc"));
        assert!(!eligible(1920, 1080, "vc1"));
        assert!(!eligible(0, 0, "h264"));
    }

    #[test]
    fn the_gate_opens_only_on_a_settled_steady_reserve() {
        let mut g = Gate::new();
        let full = Health { settled: true, reserve_ms: Some(5_000) };
        assert!(!g.step(Health { settled: false, reserve_ms: None }, 0));
        assert!(!g.step(full, 2_999), "the settle is waited out");
        assert!(!g.step(full, 3_000));
        assert!(!g.step(full, 4_999), "then the reserve must hold 2 s");
        assert!(g.step(full, 5_000));
        assert!(!g.step(Health { settled: true, reserve_ms: Some(1_000) }, 6_000), "a dip shuts it");
        assert!(!g.step(full, 15_999), "for 10 s");
        assert!(!g.step(full, 16_000));
        assert!(g.step(full, 18_000));
        assert!(!g.step(Health { settled: true, reserve_ms: None }, 18_016), "an unknown reserve is not a healthy one");
    }

    #[test]
    fn refusals_back_off_doubling_and_five_in_a_row_stop_the_film() {
        let mut b = Backoff::new();
        let mut now = 0;
        for wait in [2_000, 4_000, 8_000, 16_000] {
            b.refused(now);
            assert!(!b.ready(now + wait - 1) && b.ready(now + wait));
            now += wait;
        }
        b.served();
        b.refused(now);
        assert!(b.ready(now + 2_000), "a served batch starts the pauses over");
        for _ in 0..4 {
            b.refused(now);
        }
        assert!(b.gave_up() && !b.ready(now + 3_600_000));
    }
}
