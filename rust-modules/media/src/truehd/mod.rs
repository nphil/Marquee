//! Dolby TrueHD to the receiver, untouched, through a television whose pipeline has no TrueHD
//! input: the access units are packed into the MAT carrier ([`mat`]) and fed as 8-channel PCM that
//! the LumaTV helper routes past LG's mixer to the eARC output ([`contract`]).
//!
//! A plan picks a TrueHD track only while the helper admits this process and the sound chain
//! passes TrueHD ([`planned`]); the track it would otherwise have played stays on the session as
//! the fallback. The helper is armed with the renderer's tail and the decoder input LG's audio
//! sink is put on and reads back ([`sink`], [`session`]). The feeder ([`feeder`]) sends zeros
//! until the helper has armed the route and confirmed the bypass ([`gate`]), checks every burst,
//! and gives the carrier up after a second of forced zeros while the route is not bypassing; the
//! pump then reloads the fallback ([`take_gave_up`]). A plan picks the carrier only while the helper
//! reads idle ([`idle`]), so that late give-up is the exception. A debug build's `notruehd` trigger turns it
//! off.
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
/// Why this request's carrier was given up, for the read-out; kept after the pump acted on it.
static FELL_BACK: Mutex<Option<&'static str>> = Mutex::new(None);

/// A new play request starts: whatever the last one gave up is forgotten.
pub(crate) fn new_request() {
    VETO.store(false, Ordering::Release);
    CARRIED.store(false, Ordering::Release);
    *GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *FELL_BACK.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The rest of this request plays without the carrier.
pub(crate) fn veto() {
    VETO.store(true, Ordering::Release);
    CARRIED.store(false, Ordering::Release);
}

/// What does not depend on the helper: nothing in this request gave the carrier up, and the last
/// read of the sound chain passes TrueHD.
fn possible() -> bool {
    !VETO.load(Ordering::Acquire)
        && !plx_base::devtrig::flag("notruehd")
        && plx_platform::tv::earc::cached().is_some_and(|c| c.passes_truehd())
}

/// What a plan may count on, without a call: the helper admits this process, the last read of
/// the sound chain passes TrueHD, and nothing in this request gave the carrier up.
pub(crate) fn planned() -> bool {
    possible() && watch::topic().is_some_and(|t| t.admits(std::process::id()))
}

/// The helper's last answer is missing or past its validity, while the rest allows the carrier: a
/// plan that would pick a TrueHD track asks the helper itself first ([`idle_after_read`]). The
/// background read runs every 2 s and, while the helper does not answer, every 10 s, so a play
/// right after launch or after the plugin was switched on can come before it has seen the plugin.
pub(crate) fn reading_stale() -> bool {
    possible() && !watch::is_fresh()
}

/// What a plan may start a carrier on, without a call: the helper reads ready and idle
/// ([`contract::Topic::idle`]). Stricter than [`planned`]: a route the helper does not hold free
/// would make the carrier wait for the bypass and give up, which reloads the film on the track
/// the TV decodes.
pub(crate) fn idle() -> bool {
    possible() && watch::topic().is_some_and(|t| t.idle())
}

/// [`idle`] after one bounded read of the helper when its last answer is stale. Off the frame
/// thread only (the resolve worker); a helper that does not answer in time reads as not ready.
pub(crate) fn idle_after_read() -> bool {
    if !possible() {
        return false;
    }
    watch::refresh_if_stale();
    idle()
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
    note_fell_back(why);
    *GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
}

/// Why the carrier was given up, once.
pub(crate) fn take_gave_up() -> Option<&'static str> {
    GAVE_UP.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Why the carrier is off for the read-out, from here on in this request.
pub(crate) fn note_fell_back(why: &'static str) {
    *FELL_BACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
}

/// Where the TrueHD carrier stands for a title whose source audio is TrueHD, as Stats for Nerds
/// tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// This Load feeds the carrier; `adec` is the decoder input LG's sink holds, `audible` that the
    /// receiver is being sent the stream.
    Carrying { adec: Option<u8>, audible: bool },
    /// The carrier was given up; the title plays on the track the TV decodes.
    FellBack(&'static str),
    /// No carrier was started: the plugin is off or the TV is not ready.
    Off(String),
}

/// The carrier's state now.
pub fn status() -> Status {
    if carried() {
        let shared = session::active();
        let adec = shared.as_ref().and_then(|s| match s.granted() {
            session::Granted::Decoder(a) => Some(a),
            _ => None,
        });
        let audible = shared.is_some_and(|s| s.facts().route.is_some_and(|(r, _)| r.audible && r.bypassing));
        return Status::Carrying { adec, audible };
    }
    if let Some(why) = *FELL_BACK.lock().unwrap_or_else(|e| e.into_inner()) {
        return Status::FellBack(why);
    }
    if plx_base::devtrig::flag("notruehd") {
        return Status::Off("plugin off".into());
    }
    Status::Off(off_reason(watch::topic().as_ref()))
}

/// Why no carrier starts on this reading of the helper.
fn off_reason(topic: Option<&contract::Topic>) -> String {
    let Some(t) = topic else { return "plugin off".into() };
    match t.state {
        contract::State::Off => "plugin off".into(),
        _ if t.idle() => "not started".into(),
        _ if !t.reason.is_empty() => format!("not ready: {}", t.reason),
        _ if t.session_pid.is_some() => "not ready: route in use".into(),
        _ => "not ready".into(),
    }
}

impl Status {
    /// The Audio row's words. `atmos`: the track carries Dolby Atmos.
    pub fn line(&self, atmos: bool) -> String {
        match self {
            Status::Carrying { adec, audible: true } => {
                let kind = if atmos { "Atmos" } else { "TrueHD" };
                match adec {
                    Some(a) => format!("TrueHD passthrough: {kind} to receiver (ADEC{a}, audible)"),
                    None => format!("TrueHD passthrough: {kind} to receiver (audible)"),
                }
            }
            Status::Carrying { audible: false, .. } => "TrueHD passthrough: arming…".into(),
            Status::FellBack(why) => format!("TrueHD passthrough: fell back: {why}"),
            Status::Off(why) => format!("TrueHD passthrough: off ({why})"),
        }
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn the_status_reads_as_the_audio_path() {
        let carrying = |audible| Status::Carrying { adec: Some(1), audible }.line(true);
        assert_eq!(carrying(true), "TrueHD passthrough: Atmos to receiver (ADEC1, audible)");
        assert_eq!(carrying(false), "TrueHD passthrough: arming…");
        assert_eq!(Status::Carrying { adec: None, audible: true }.line(false), "TrueHD passthrough: TrueHD to receiver (audible)");
        assert_eq!(
            Status::FellBack("the route was not audible in time").line(true),
            "TrueHD passthrough: fell back: the route was not audible in time"
        );
        assert_eq!(Status::Off("plugin off".into()).line(true), "TrueHD passthrough: off (plugin off)");
        let busy = contract::parse_topic(br#"{"schema":1,"state":"not-ready","ready":false,"reason":"no converter"}"#).unwrap();
        assert_eq!(off_reason(Some(&busy)), "not ready: no converter");
        assert_eq!(off_reason(None), "plugin off");
    }
}
