//! **The read-ahead reserve of a direct-play stream**: how much media the two AU queues may hold
//! beyond their base caps, and how much of it a stall hold needs before it plays again.
//!
//! A file whose busy scenes need more than the link carries plays through them only on media read
//! ahead while the calm scenes left the link spare. The base caps (10 MiB of video) are about a
//! second of an 89 Mbit/s film (measured 2026-11-21: a hold every 30 to 100 s on a saturated
//! 100 Mbit/s port), so the queues of a progressive Original stream get an allowance on top
//! ([`crate::aq::aq_set_reserve`]): up to [`TARGET_NS`] of media, in as many bytes as the TV has
//! to spare. The demuxer keeps reading at link speed until one of the two is reached.
//!
//! The bytes come from the app's memory guard, which reports the system's available memory every
//! two seconds ([`note_memory`]). Half of what is available above [`KEEP_FREE_BYTES`] is the
//! allowance, at most [`MAX_BYTES`], and none while the guard calls memory short: the queues then
//! drain to their base caps as the film plays. Nothing is dropped to shrink, because dropped media
//! would have to be read again.
//!
//! Pure values and two atomics; `pump` applies the allowance and `engine::try_prime` asks
//! [`hold_releases`].

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

/// Media the queues aim to hold ahead of the feed.
pub const TARGET_NS: i64 = 60_000_000_000;
/// Available memory the allowance never draws on. webOS starts closing apps when memory runs low
/// and this TV idles with its swap nearly full, so the margin is wide.
const KEEP_FREE_BYTES: u64 = 224 << 20;
/// The most the allowance is, however much is free.
const MAX_BYTES: u64 = 192 << 20;
/// An allowance under this is not worth a second cap: the base caps stand alone.
const MIN_BYTES: u64 = 16 << 20;
/// The allowance moves in steps of this, so a sample that wobbles does not move the caps.
const STEP_BYTES: u64 = 8 << 20;
/// The audio lane's share of the bytes. One reader fills both lanes, so the audio lane must be
/// able to hold the same seconds as the video lane: an eighth covers lossless audio beside 4K.
const AUDIO_SHARE: u64 = 8;

/// The memory guard's last sample: available bytes (0 until the first) and whether it is short.
static AVAILABLE_BYTES: AtomicU64 = AtomicU64::new(0);
static MEMORY_SHORT: AtomicBool = AtomicBool::new(false);
/// Bumped per sample, so the pump recomputes only when there is a new one.
static SAMPLES: AtomicU32 = AtomicU32::new(0);

/// The app's memory guard: one sample of the system's available memory, and its own verdict.
pub fn note_memory(available_bytes: u64, short: bool) {
    AVAILABLE_BYTES.store(available_bytes, Relaxed);
    MEMORY_SHORT.store(short, Relaxed);
    SAMPLES.fetch_add(1, Relaxed);
}

/// How many samples [`note_memory`] has taken.
pub fn samples() -> u32 {
    SAMPLES.load(Relaxed)
}

/// What the two lanes may hold beyond their base caps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allowance {
    pub video_bytes: i64,
    pub audio_bytes: i64,
}

impl Allowance {
    pub const fn total(self) -> i64 {
        self.video_bytes + self.audio_bytes
    }
}

/// The allowance for the last memory sample. `held_bytes` is what the queues hold now: it is
/// already missing from the available figure, so it is added back before the share is taken.
pub fn allowance(held_bytes: u64) -> Allowance {
    allowance_for(AVAILABLE_BYTES.load(Relaxed), held_bytes, MEMORY_SHORT.load(Relaxed))
}

fn allowance_for(available_bytes: u64, held_bytes: u64, short: bool) -> Allowance {
    if short || available_bytes == 0 {
        return Allowance::default();
    }
    let spare = (available_bytes + held_bytes).saturating_sub(KEEP_FREE_BYTES) / 2;
    let total = spare.min(MAX_BYTES) / STEP_BYTES * STEP_BYTES;
    if total < MIN_BYTES {
        return Allowance::default();
    }
    let audio = total / AUDIO_SHARE;
    Allowance { video_bytes: (total - audio) as i64, audio_bytes: audio as i64 }
}

/// The least media a hold plays again with.
pub const RESUME_MIN_NS: i64 = 3_000_000_000;
/// The most media a hold waits for.
pub const RESUME_MAX_NS: i64 = 12_000_000_000;
/// Playing time a hold aims to buy before the next one, at the rate the link filled it.
const CLEAR_NS: i64 = 180_000_000_000;
/// A hold longer than this plays with what it has, if that is at least [`DEADLINE_MIN_NS`].
pub const MAX_HOLD_MS: u32 = 15_000;
const DEADLINE_MIN_NS: i64 = 1_500_000_000;
/// A hold this young has not measured the link: it waits for [`RESUME_MAX_NS`] or the measure.
const MEASURE_MS: u32 = 1_000;

/// Which clause let a progressive stall hold release the clock, as the resume line names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// The demuxer finished: nothing more will arrive.
    Eof,
    /// A queue holds all it may and the demuxer is parked on it: nothing more can be read ahead of
    /// Play. One reader feeds both lanes, so whichever fills first parks it.
    Full,
    /// The reserve reached what the link's rate during the hold asks for ([`resume_reserve_ns`]).
    Reserve,
    /// The hold lasted [`MAX_HOLD_MS`].
    Deadline,
}

impl Release {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Eof => "eof",
            Self::Full => "full",
            Self::Reserve => "reserve",
            Self::Deadline => "deadline",
        }
    }
}

/// What a hold knows when it asks whether to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    /// A lane queue is full with the demuxer parked on it.
    pub queue_full: bool,
    pub producer_done: bool,
    /// Media between the playhead and the point the demuxer has read to, now and when the hold
    /// began.
    pub reserve_ns: i64,
    pub reserve_at_start_ns: i64,
    pub held_ms: u32,
}

/// The reserve a hold releases at, from the media it gained over `held_ms`.
///
/// While the clock is held the reserve grows by `link rate / film rate` seconds per second; once
/// it plays, the reserve shrinks by one minus that. The hold waits for enough to play
/// [`CLEAR_NS`] at that rate, between [`RESUME_MIN_NS`] (a link that keeps up needs no more) and
/// [`RESUME_MAX_NS`] (a link far behind is not waited on: the freeze watch offers a lower quality).
pub fn resume_reserve_ns(gained_ns: i64, held_ms: u32) -> i64 {
    if held_ms < MEASURE_MS {
        return RESUME_MAX_NS;
    }
    let held_ns = i64::from(held_ms) * 1_000_000;
    let short_permille = (1_000 - gained_ns.max(0) * 1_000 / held_ns).clamp(0, 1_000);
    (CLEAR_NS / 1_000 * short_permille).clamp(RESUME_MIN_NS, RESUME_MAX_NS)
}

/// Whether a progressive stall hold may release the clock, and by which clause. Each plays what
/// there is, so none waits for a decoder prime; the stronger fact is named first.
pub fn hold_releases(h: Hold) -> Option<Release> {
    if h.producer_done {
        Some(Release::Eof)
    } else if h.queue_full {
        Some(Release::Full)
    } else if h.reserve_ns >= resume_reserve_ns(h.reserve_ns - h.reserve_at_start_ns, h.held_ms) {
        Some(Release::Reserve)
    } else if h.held_ms >= MAX_HOLD_MS && h.reserve_ns >= DEADLINE_MIN_NS {
        Some(Release::Deadline)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;
    const S: i64 = 1_000_000_000;

    fn hold(reserve_ns: i64, held_ms: u32) -> Hold {
        Hold { queue_full: false, producer_done: false, reserve_ns, reserve_at_start_ns: 0, held_ms }
    }

    /// The TV of 2026-11-21: 539 MiB available with the film playing.
    #[test]
    fn the_allowance_is_half_of_what_is_available_above_the_margin() {
        let a = allowance_for(539 * MIB, 0, false);
        assert_eq!(a.total() as u64, 152 * MIB, "(539 - 224) / 2, in 8 MiB steps");
        assert_eq!(a.audio_bytes as u64, 19 * MIB);
        // The queues' own bytes are added back: filling the allowance does not shrink it.
        assert_eq!(allowance_for(387 * MIB, 152 * MIB, false), a);
    }

    #[test]
    fn the_allowance_is_capped_and_goes_when_memory_is_short() {
        assert_eq!(allowance_for(4096 * MIB, 0, false).total() as u64, MAX_BYTES);
        assert_eq!(allowance_for(4096 * MIB, 0, true), Allowance::default(), "the guard calls it short");
        assert_eq!(allowance_for(250 * MIB, 0, false), Allowance::default(), "under the least worth having");
        assert_eq!(allowance_for(0, 64 * MIB, false), Allowance::default(), "no sample yet");
    }

    /// The film of 2026-11-21 gained about 0.97 s of media per second held: a deficit of 3%.
    #[test]
    fn a_hold_waits_for_what_the_measured_link_needs() {
        assert_eq!(resume_reserve_ns(970_000_000 * 4, 4_000), 5_400_000_000, "3% short: 180 s of it");
        assert_eq!(resume_reserve_ns(5 * S, 4_000), RESUME_MIN_NS, "a link ahead of the film");
        assert_eq!(resume_reserve_ns(2 * S, 4_000), RESUME_MAX_NS, "a link at half the film");
        assert_eq!(resume_reserve_ns(0, 500), RESUME_MAX_NS, "not measured yet");
    }

    #[test]
    fn a_hold_releases_on_its_reserve_a_full_queue_the_end_or_the_deadline() {
        assert_eq!(hold_releases(hold(2 * S, 2_000)), None, "under the least reserve");
        assert_eq!(hold_releases(hold(3 * S, 3_000)), Some(Release::Reserve), "the link keeps up: 3 s is enough");
        // 5% short: 9 s wanted, not there at 6 s of hold, there at 9.5 s.
        assert_eq!(hold_releases(hold(5_700_000_000, 6_000)), None);
        assert_eq!(hold_releases(hold(9_025_000_000, 9_500)), Some(Release::Reserve));
        // A link at a third of the film never reaches 12 s in time: the deadline plays what there is.
        assert_eq!(hold_releases(hold(4_950_000_000, 14_900)), None);
        assert_eq!(hold_releases(hold(5 * S, MAX_HOLD_MS)), Some(Release::Deadline));
        assert_eq!(hold_releases(hold(S, 60_000)), None, "a dead link has nothing to play");
        let full = Hold { queue_full: true, ..hold(S, 500) };
        assert_eq!(hold_releases(full), Some(Release::Full), "nothing more can be read ahead");
        assert_eq!(hold_releases(Hold { producer_done: true, ..full }), Some(Release::Eof), "the stronger fact is named");
    }
}
