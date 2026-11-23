//! Dolby TrueHD to the receiver, untouched, through a television whose pipeline has no TrueHD
//! input: the access units are packed into the MAT carrier ([`mat`]) and fed as 8-channel PCM that
//! the LumaTV helper routes past LG's mixer to the eARC output ([`contract`]).
//!
//! A plan picks a TrueHD track only while the helper admits this process and the sound chain
//! passes TrueHD ([`planned`]); the track it would otherwise have played stays on the session as
//! the fallback. The helper is armed with the renderer's tail and the decoder input LG's audio
//! sink reports ([`sink`], [`session`]). The feeder ([`feeder`]) sends zeros until the helper has
//! armed the route and confirmed the bypass ([`gate`]), checks every burst, and gives the carrier
//! up after a second of forced zeros while the route is not bypassing; the pump then reloads the
//! fallback ([`take_gave_up`]). A debug build's `notruehd` trigger turns it off.
pub mod contract;
pub mod feeder;
pub mod gate;
pub mod mat;
pub mod session;
pub mod sink;
pub mod watch;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The request has given the carrier up.
static VETO: AtomicBool = AtomicBool::new(false);
/// This Load feeds the carrier.
static CARRIED: AtomicBool = AtomicBool::new(false);
/// Why the carrier was given up, until the pump acts on it.
static GAVE_UP: Mutex<Option<&'static str>> = Mutex::new(None);

/// A new play request starts: whatever the last one gave up is forgotten.
pub(crate) fn new_request() {
    VETO.store(false, Ordering::Release);
    CARRIED.store(false, Ordering::Release);
    *GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The rest of this request plays without the carrier.
pub(crate) fn veto() {
    VETO.store(true, Ordering::Release);
    CARRIED.store(false, Ordering::Release);
}

/// What a plan may count on, without a call: the helper admits this process, the last read of
/// the sound chain passes TrueHD, and nothing in this request gave the carrier up.
pub(crate) fn planned() -> bool {
    !VETO.load(Ordering::Acquire)
        && !plx_base::devtrig::flag("notruehd")
        && watch::topic().is_some_and(|t| t.admits(std::process::id()))
        && plx_platform::tv::earc::cached().is_some_and(|c| c.passes_truehd())
}

/// Whether the carrier takes a track: TrueHD in the 48 kHz family of rates, or at a rate PMS did
/// not state. The 44.1 kHz family has no MAT code ([`mat::Fallback::Rate`]), and the feeder gives
/// it up at the first major sync, which the pump answers with the fallback track.
pub(crate) fn carries(codec: &str, sample_rate: i64) -> bool {
    plx_platform::devcaps::audio_codec_id(codec) == "truehd" && matches!(sample_rate, 0 | 48_000 | 96_000 | 192_000)
}

/// The Load says whether it feeds the carrier. A give-up an earlier Load's feeder left is not
/// this one's.
pub(crate) fn latch(carried: bool) {
    CARRIED.store(carried, Ordering::Release);
    if carried {
        *GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// This Load feeds the carrier.
pub(crate) fn carried() -> bool {
    CARRIED.load(Ordering::Acquire)
}

/// The carrier's shape for a new session, as the helper names it.
pub(crate) fn carrier() -> contract::Carrier {
    watch::topic().map(|t| t.carrier).unwrap_or_default()
}

/// The feeder gave the carrier up.
pub(crate) fn give_up(why: &'static str) {
    *GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
}

/// Why the carrier was given up, once.
pub(crate) fn take_gave_up() -> Option<&'static str> {
    GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()).take()
}
