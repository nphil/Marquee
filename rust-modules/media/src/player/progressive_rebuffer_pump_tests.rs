//! A starved direct-play (progressive Original) stream, driven through the real `pump()` over the
//! host sink: the link delivers less than the file needs, the sink clock plays out what it was fed,
//! and the viewer must be told "Buffering" instead of watching slow motion and then a catch-up that
//! drops frames.
//!
//! Every case runs the whole chain, the way `end_of_film_tests` does: the feed lanes, the pump's own
//! decision to hold, the sink's Pause, the position callback, the readout. Nothing here calls the
//! hold's decision function or sets `hls_rebuffering` itself, so a test that passes has seen the
//! pump do it. The numbers are literals, not the crate's constants: a test that names the threshold
//! it grades cannot fail when the threshold is wrong.
//!
//! The host clock is a plant, and its shape bounds what these can see. It clamps the position to the
//! highest video pts it was fed and keeps reporting while it plays, which is what Starfish does on a
//! link slower than the file (it presents at the delivered rate, with an empty queue). Two
//! instruments sit on top of it. The callback stamps its own time from the real monotonic clock, on
//! which a step takes microseconds, so a detector that differences the wall against the position
//! needs the two to move together: every rig that wants the clock debt stamps the callbacks with a
//! virtual wall (`Rig::with_virtual_wall`). The dead-link arm reads the real clock's silence
//! instead, so its test leaves the wall alone and ages the callback and the watch directly.
//!
//! The run's hold and resume lines are the field-check contract (`progressive rebuffer hold n=` and
//! `... resume n= reason=`): the tests read them back.

use super::end_of_film_tests::Rig;
use super::*;
use crate::player::engine::PRES_NONE;
use crate::player::ffi_host;
use crate::player::PlaybackState;
use crate::player::stall_watch::Notice;
use std::os::raw::c_int;

const S: i64 = 1_000_000_000;
const V_STEP_NS: i64 = 41_666_666; // 24 fps
const A_STEP_NS: i64 = 21_333_333; // AAC: 1024 samples at 48 kHz
/// A clock within one native period of the tail it clamps at is spent.
const CLAMPED_NS: i64 = 201_000_000;
/// A reserve that is plainly healthy: nothing may hold while the clock plays against it.
const HEALTHY_NS: i64 = 700_000_000;
/// Pump periods the debt may take to fire once the clock is parked: it owes the whole 200 ms period
/// against a 100 ms threshold, so the second parked sample fires it; the rest is slack for the
/// sample that straddles the clamp.
const DEBT_PERIODS: usize = 4;
/// Steps a run may take.
const STEP_BUDGET: usize = 120;

/// How many Pause verbs the host sink saw since the probe was reset.
fn pauses() -> usize {
    ffi_host::verb_order_for_test().iter().filter(|v| v.as_str() == "pause").count()
}

/// The pts of the last access unit a lane of `end_ns` holds, built the way `Rig` builds it.
fn last_pts(end_ns: i64, step_ns: i64) -> i64 {
    (end_ns / step_ns - 1) * step_ns
}

/// The audio lane's queue, for the tests that fill it.
fn audio_queue(rig: &mut Rig) -> *mut crate::aq::AuQueue {
    &mut **rig.eng().aq_audio.as_mut().unwrap() as *mut crate::aq::AuQueue
}

/// **A starved progressive stream is held and reads Buffering, through the pump.**
///
/// Both lanes hold six seconds, no EOF, and nothing more is coming: a link that stopped delivering.
/// The clock plays the six seconds out. While more than the healthy depth is buffered nothing may
/// hold; once the reserve reaches one native position period the pump must have paused the sink,
/// within [`DEBT_PERIODS`] pump periods, and the readout must say Buffering.
///
/// On a build whose pump returns from the hold at `!segmented_hls` no Pause is ever sent: the clock
/// parks at the tail, the sink would drop every later frame, and the viewer sees a frozen picture.
#[test]
fn a_starved_progressive_stream_pauses_the_sink_and_reads_buffering() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
    ffi_host::reset_volume_probe_for_test();

    // PRECONDITIONS, asserted rather than assumed: they say a red run is the defect and not a dead
    // instrument. The route is direct play; the furthest lane's tail is within one period of the
    // tail the clock will clamp at (otherwise the hold could never begin in this plant).
    assert!(!crate::route::is_segmented_hls(&rig.ps), "PRECONDITION FAILED, not the defect: the route is HLS");
    assert!(
        crate::route::takes_progressive_hold(&rig.ps),
        "PRECONDITION FAILED, not the defect: the rig is not a stream the progressive hold takes"
    );
    let video_tail = last_pts(6 * S, V_STEP_NS);
    let audio_tail = last_pts(6 * S, A_STEP_NS);
    assert!(
        audio_tail - video_tail <= CLAMPED_NS,
        "PRECONDITION FAILED, not the defect: audio ends {}ms past the video the sink clock clamps to",
        (audio_tail - video_tail) / 1_000_000
    );
    let tail = video_tail.max(audio_tail);

    let mut saw_healthy_reserve = false;
    let mut parked_at: Option<usize> = None;
    let mut paused_at: Option<usize> = None;
    for n in 0..STEP_BUDGET {
        rig.step();
        let position = SHARED.pres_fed.load(Relaxed);
        if position == PRES_NONE {
            continue;
        }
        let reserve = tail - position;
        if reserve > HEALTHY_NS {
            saw_healthy_reserve = true;
            assert_eq!(pauses(), 0, "a healthy reserve ({}ms) was held: {}", reserve / 1_000_000, rig.facts());
        }
        if parked_at.is_none() && saw_healthy_reserve && reserve <= CLAMPED_NS {
            parked_at = Some(n);
        }
        if paused_at.is_none() && pauses() > 0 {
            paused_at = Some(n);
        }
        if parked_at.is_some_and(|at| n >= at + DEBT_PERIODS) {
            break;
        }
    }

    assert!(
        saw_healthy_reserve,
        "PRECONDITION FAILED, not the defect: the clock never played with a reserve: {}",
        rig.facts()
    );
    let parked_at = parked_at.unwrap_or_else(|| {
        panic!("PRECONDITION FAILED, not the defect: the clock never reached the tail: {}", rig.facts())
    });
    assert!(
        paused_at.is_some_and(|at| at <= parked_at + DEBT_PERIODS),
        "the clock parked at the tail at step {parked_at} and the sink was not paused within {DEBT_PERIODS} \
         pump periods (first pause at step {paused_at:?}): the stream is left to drop frames. {}",
        rig.facts()
    );
    assert!(SHARED.hls_rebuffering.load(Relaxed), "the hold did not raise the Buffering mirror");
    assert_eq!(
        crate::player::state(&rig.ps),
        PlaybackState::Buffering,
        "the readout does not say Buffering: {}",
        rig.facts()
    );
}

// ---- the rest of the contract, on the same rig: every hold below is begun by the pump's own decision ----

/// The producer delivers `from_ns..to_ns` more media on both lanes, cut the way `Rig::with_media`
/// cut the first stretch (`from_ns` is where the last delivery ended: a running cursor makes the
/// ranges contiguous, whatever their size).
fn deliver(rig: &mut Rig, from_ns: i64, to_ns: i64) {
    let au = [0u8; 64];
    let eng = rig.eng();
    let qv = &mut **eng.aq_video.as_mut().unwrap() as *mut crate::aq::AuQueue;
    for i in from_ns / V_STEP_NS..to_ns / V_STEP_NS {
        crate::aq::aq_push(qv, au.as_ptr(), au.len() as c_int, i * V_STEP_NS, 0, 1);
    }
    let qa = &mut **eng.aq_audio.as_mut().unwrap() as *mut crate::aq::AuQueue;
    for i in from_ns / A_STEP_NS..to_ns / A_STEP_NS {
        crate::aq::aq_push(qa, au.as_ptr(), au.len() as c_int, i * A_STEP_NS, 1, 2);
    }
}

/// `run`'s result and the event-log text it wrote.
fn logged<R>(run: impl FnOnce() -> R) -> (R, String) {
    plx_base::eventlog::with_private_log(|| {
        let result = run();
        (result, std::fs::read_to_string(plx_base::eventlog::events_log()).unwrap_or_default())
    })
}

/// Play the first stretch out until the pump has held the stream.
fn run_until_held(rig: &mut Rig) {
    ffi_host::reset_volume_probe_for_test();
    assert!(
        rig.run_until(|_| SHARED.hls_rebuffering.load(Relaxed)),
        "PRECONDITION FAILED, not the defect: the starved stream was never held: {}",
        rig.facts()
    );
    assert_eq!(pauses(), 1, "the hold is exactly one Pause");
}

/// **A fixed quality's progressive re-encode is held the same way.** Its supply is the server's
/// conversion as well as the link: a 4K source converted with a subtitle burned in arrived at
/// 0.84x of real time and played in slow motion. The same starved six seconds under a 1080p
/// ceiling: one Pause and the Buffering read-out.
#[test]
fn a_starved_fixed_quality_re_encode_is_held_and_reads_buffering() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
    crate::route::mark_fixed_quality_ceiling_for_test(&mut rig.ps);
    run_until_held(&mut rig);
    assert_eq!(crate::player::state(&rig.ps), PlaybackState::Buffering, "{}", rig.facts());
}

/// **A link slower than the file: one hold, not slow motion (the conservation rate).** After a healthy
/// start the link delivers 80 ms of media per 200 ms pump period (0.4x). Starfish presents at the
/// delivered rate with an empty queue, so once the six seconds it had run out, the position callbacks
/// keep arriving with the position crawling at 0.4x: slow motion, which is what the debt detects.
/// The pump holds within a dozen periods and the one hold stands while the link goes on delivering
/// (a hold releases on a full queue or the end of the stream, never on a runway).
#[test]
fn a_link_slower_than_the_file_is_held_once_and_the_hold_stands() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        ffi_host::reset_volume_probe_for_test();
        let mut delivered = 6 * S;
        let mut first_slow_step: Option<usize> = None;
        let mut held_at: Option<usize> = None;
        for n in 0..90 {
            // the link carries 0.4 s of film per second of wall
            deliver(&mut rig, delivered, delivered + 80_000_000);
            delivered += 80_000_000;
            rig.step();
            let position = SHARED.pres_fed.load(Relaxed);
            if position != PRES_NONE && first_slow_step.is_none() && delivered - position < S / 2 {
                first_slow_step = Some(n);
            }
            if held_at.is_none() && SHARED.hls_rebuffering.load(Relaxed) {
                held_at = Some(n);
            }
            if first_slow_step.is_none() {
                assert_eq!(pauses(), 0, "a healthy start was held at step {n}: {}", rig.facts());
            }
        }
        let slow = first_slow_step
            .unwrap_or_else(|| panic!("PRECONDITION FAILED, not the defect: the reserve never ran out: {}", rig.facts()));
        let held = held_at.unwrap_or_else(|| panic!("a 0.4x link was never held (slow from step {slow}): {}", rig.facts()));
        assert!(
            held <= slow + 12,
            "held at step {held}, {} periods after the reserve was spent: slow motion for {} s",
            held - slow,
            (held - slow) / 5
        );
        assert_eq!(pauses(), 1, "exactly one hold, standing while the link keeps delivering: {}", rig.facts());
        assert!(SHARED.hls_rebuffering.load(Relaxed), "still Buffering");
    });
    assert!(text.contains("progressive rebuffer hold n=1 arm=debt"), "no debt-arm hold line: {text}");
    assert!(!text.contains("progressive rebuffer hold n=2"), "a second hold began under the first: {text}");
}

/// A stamp `ago_ms` before `now_ms` on the millisecond clock the silence observation reads. A stamp
/// of 0 means "no callback yet", so it is never produced. The clock starts at 0 with the test
/// process, so a stamp aged past its start wraps; the silence is a wrapping distance
/// (`silence_between`), which reads it as the age it is.
fn stamp_ago(now_ms: u32, ago_ms: u32) -> u32 {
    now_ms.wrapping_sub(ago_ms).max(1)
}

/// Age the silence observation without waiting for it: the clock last reported `callback_ago_ms`
/// ago and the watch was last restarted `restart_ago_ms` ago (the silence runs from the later of
/// the two). The pump reads the same real clock, which only moves forward by the microseconds a
/// step takes, so a test that ages by 1100 ms and expects 1005 ms of silence cannot be early, and
/// one that ages by 800 ms cannot be late. No test here sleeps.
fn age_observation(callback_ago_ms: u32, restart_ago_ms: u32) {
    let now = crate::player::vclock_ms();
    SHARED.dg_vpres_at.store(stamp_ago(now, callback_ago_ms), Relaxed);
    CLOCK_WATCH_SINCE_MS.store(stamp_ago(now, restart_ago_ms), Relaxed);
}

/// A direct-play stream with six seconds in both lanes and its position callbacks owned by the test
/// (`Rig::hushed`), played to within 700 ms of the tail it will starve at, with nothing held yet.
fn a_stream_about_to_run_dry() -> Rig {
    let mut rig = Rig::with_media(6 * S, 6 * S).hushed();
    ffi_host::reset_volume_probe_for_test();
    let tail = last_pts(6 * S, V_STEP_NS).max(last_pts(6 * S, A_STEP_NS));
    assert!(
        rig.run_until(|_| {
            let p = SHARED.pres_fed.load(Relaxed);
            p != PRES_NONE && tail - p <= HEALTHY_NS
        }),
        "PRECONDITION FAILED, not the defect: the clock never came within 700 ms of the tail: {}",
        rig.facts()
    );
    assert_eq!(pauses(), 0, "PRECONDITION FAILED, not the defect: held before the link died: {}", rig.facts());
    rig
}

/// **A dead link is held through the silence arm.** Nothing arrives and the native clock stops
/// reporting: the callbacks the debt watches are gone, so the dead-link arm (the silence observation,
/// 1005 ms, over a reserve the stopped clock has already outlasted) must take the hold. The last
/// callback is aged to 800 ms ago, which is a clock that is merely late and must not hold, and then
/// to 1100 ms ago, which must: one Pause, Buffering, and a hold line that names the silence arm.
#[test]
fn a_dead_link_is_held_through_the_silence_arm() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = a_stream_about_to_run_dry();
        age_observation(800, 900);
        rig.step_without_sink();
        assert_eq!(pauses(), 0, "a clock that is 800 ms late was held: {}", rig.facts());
        age_observation(1_100, 1_200);
        rig.step_without_sink();
        assert_eq!(pauses(), 1, "a clock silent for 1100 ms over a spent reserve was not held: {}", rig.facts());
        assert!(SHARED.hls_rebuffering.load(Relaxed), "the hold did not raise Buffering");
        assert_eq!(crate::player::state(&rig.ps), PlaybackState::Buffering, "the readout does not say Buffering");
    });
    assert!(text.contains("progressive rebuffer hold n=1 arm=silence"), "no silence-arm hold line: {text}");
}

/// **A hold whose own release is already true is not begun.** The audio queue is at its byte cap and
/// the sink takes no more audio, so the demuxer is parked on it: the full queue that releases a hold
/// is already there. Beginning one anyway would Pause and then Play in the same tick, and again on
/// the next. The video clock plays out and parks at the tail, which owes the debt arm a hold; none
/// may begin.
#[test]
fn a_hold_is_not_begun_when_its_own_release_is_already_true() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        ffi_host::reset_volume_probe_for_test();
        for _ in 0..8 {
            rig.step();
        }
        ffi_host::refuse_feed_lane_for_test(Some(2));
        let qa = audio_queue(&mut rig);
        let big = vec![0u8; crate::aq::aq_cap(qa) as usize + 1];
        crate::aq::aq_push(qa, big.as_ptr(), big.len() as c_int, 6 * S, 1, 2);
        assert!(crate::aq::aq_would_park(qa), "PRECONDITION FAILED, not the defect: the audio queue is not at its cap");
        let plays = ffi_host::play_calls_for_test();
        let tail = last_pts(6 * S, V_STEP_NS).max(last_pts(6 * S, A_STEP_NS));
        let mut parked_steps = 0;
        for _ in 0..STEP_BUDGET {
            rig.step();
            let position = SHARED.pres_fed.load(Relaxed);
            if position != PRES_NONE && tail - position <= CLAMPED_NS {
                parked_steps += 1;
            }
            if parked_steps >= 3 * DEBT_PERIODS {
                break;
            }
        }
        assert!(
            parked_steps >= 3 * DEBT_PERIODS,
            "PRECONDITION FAILED, not the defect: the clock never parked at the tail, so no hold was owed: {}",
            rig.facts()
        );
        let qa = audio_queue(&mut rig);
        assert!(
            crate::aq::aq_would_park(qa),
            "PRECONDITION FAILED, not the defect: the audio queue drained, so its release was not true"
        );
        assert_eq!(pauses(), 0, "a hold was begun over a queue that already releases it: {}", rig.facts());
        assert_eq!(ffi_host::play_calls_for_test(), plays, "a Play followed a Pause that should never have been sent");
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "Buffering was raised for a hold that released at once");
    });
    assert!(!text.contains("progressive rebuffer hold"), "a hold line was written: {text}");
}

#[test]
fn a_slow_clock_with_both_tracks_already_fed_is_not_paused_for_network_refill() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
    assert!(rig.run_until(|rig| {
        let p = SHARED.pres_fed.load(Relaxed);
        let e = rig.eng();
        p > super::super::clock_debt::DEBT_MIN_POS_NS && e.max_fed_video_pts.min(e.max_fed_audio_pts) - p > HEALTHY_NS
    }));
    ffi_host::reset_volume_probe_for_test();
    let mut wall = SHARED.dg_vpres_at.load(Relaxed);
    for _ in 0..3 {
        ffi_host::advance_clock_for_test(40);
        ffi_host::tick_for_test();
        wall += 200;
        SHARED.dg_vpres_at.store(wall, Relaxed);
        rig.step_without_sink();
    }
    assert_eq!(pauses(), 0, "refilling cannot help a decoder already holding both tracks");
}

/// **The hold's way out.** A progressive hold has no HLS segment to credit, so a certificate borrowed
/// from the HLS hold would show Buffering for good. The producer delivers again: media that leaves
/// the queue with room keeps holding (no runway constant), and the Play that ends it clears Buffering
/// and starts the clock. (The release by a full queue is pinned in `engine.rs`: the host sink takes
/// every access unit, so a queue cannot be left full through a feed pass here.) The end of the stream
/// is the release the rig can reach, and the film still ends: the EOS follows the release.
#[test]
fn a_hold_releases_at_the_end_of_the_stream_and_not_on_a_runway() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        run_until_held(&mut rig);
        let plays = ffi_host::play_calls_for_test();
        assert!(!rig.eng().eos_pushed, "PRECONDITION FAILED, not the defect: the EOS went out under the hold");

        deliver(&mut rig, 6 * S, 7 * S);
        for _ in 0..8 {
            rig.step();
        }
        assert!(SHARED.hls_rebuffering.load(Relaxed), "a queue with room is still holding");
        assert_eq!(ffi_host::play_calls_for_test(), plays, "no Play while the queue has room and the stream goes on");

        deliver(&mut rig, 7 * S, 12 * S);
        rig.producer_done();
        assert!(
            rig.run_until(|_| !SHARED.hls_rebuffering.load(Relaxed)),
            "the stream ended and the hold never released: {}",
            rig.facts()
        );
        assert_eq!(ffi_host::play_calls_for_test(), plays + 1, "one Play ends the hold");
        assert_ne!(crate::player::state(&rig.ps), PlaybackState::Buffering, "Buffering is still up after the Play");
        let before = SHARED.pres_fed.load(Relaxed);
        for _ in 0..5 {
            rig.step();
        }
        assert!(SHARED.pres_fed.load(Relaxed) > before, "the clock runs again after the Play");
        assert_eq!(pauses(), 1, "playing the new media out did not hold again");
        assert!(rig.run_until(|r| r.eng().eos_pushed), "the hold released but pushEOS never followed: {}", rig.facts());
    });
    assert!(text.contains("progressive rebuffer hold n=1 arm=debt"), "no debt-arm hold line: {text}");
    for field in ["S=", "debt=", "qv=", "qa=", "silent="] {
        assert!(text.contains(field), "the hold line lacks {field}: {text}");
    }
    assert!(text.contains("progressive rebuffer resume n=1 reason=eof held="), "no resume line: {text}");
}

/// **The viewer's Pause and Resume inside a hold.** The Resume must not start the clock under the
/// hold (the media it waits for has not arrived): it stays held, still reading Buffering, and the
/// hold's own certificate plays it once the stream ends.
#[test]
fn a_user_pause_and_resume_inside_a_hold_stay_held_until_the_hold_releases() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
    run_until_held(&mut rig);
    let plays = ffi_host::play_calls_for_test();

    assert!(crate::player::pause(&mut rig.pa), "the viewer's Pause is accepted inside the hold");
    for _ in 0..3 {
        rig.step();
    }
    assert!(SHARED.hls_rebuffering.load(Relaxed), "a user Pause does not end the hold");
    assert!(crate::player::resume(&mut rig.pa), "the viewer's Resume is accepted");
    for _ in 0..3 {
        rig.step();
    }
    assert_eq!(ffi_host::play_calls_for_test(), plays, "Resume started the clock under a hold with no media");
    assert!(SHARED.hls_rebuffering.load(Relaxed), "still Buffering");

    deliver(&mut rig, 6 * S, 12 * S);
    rig.producer_done();
    assert!(
        rig.run_until(|_| !SHARED.hls_rebuffering.load(Relaxed)),
        "the stream ended and the hold never released: {}",
        rig.facts()
    );
    assert_eq!(ffi_host::play_calls_for_test(), plays + 1);
    assert!(!TX.paused.load(Relaxed), "the viewer asked for Play");
}

/// **A hold the viewer paused inside ends on its reserve, not on their Resume.** On 2026-11-21 a
/// Pause three seconds into a stall and a menu left the screen reading Buffering for 40 s over a
/// queue that had refilled long before. Thirteen seconds of film arrive under the pause: Buffering
/// comes down with no Play, the clock is the viewer's, and their Resume plays.
#[test]
fn a_hold_the_viewer_paused_inside_ends_on_its_reserve_and_reads_paused() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        run_until_held(&mut rig);
        let plays = ffi_host::play_calls_for_test();
        assert!(crate::player::pause(&mut rig.pa), "the viewer's Pause is accepted inside the hold");

        deliver(&mut rig, 6 * S, 19 * S);
        rig.step();
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "the hold outlasted its reserve: {}", rig.facts());
        assert_ne!(crate::player::state(&rig.ps), PlaybackState::Buffering, "a paused viewer still reads Buffering");
        assert!(TX.paused.load(Relaxed), "the viewer's Pause was dropped with the hold");
        assert_eq!(ffi_host::play_calls_for_test(), plays, "the clock started under the viewer's Pause");

        assert!(crate::player::resume(&mut rig.pa), "the viewer's Resume is accepted");
        assert!(
            rig.run_until(|_| ffi_host::play_calls_for_test() == plays + 1),
            "Resume did not play: {}",
            rig.facts()
        );
        assert_eq!(pauses(), 1, "the Resume was held again over a full reserve: {}", rig.facts());
    });
    assert!(
        text.lines().any(|l| l.contains("progressive rebuffer resume n=1 reason=reserve") && l.ends_with("-> viewer pause")),
        "no line for a hold ended under the viewer's pause: {text}"
    );
}

/// **One component decides "buffering"; the freeze net only reports it.** A standing hold is a wait
/// the freeze net (`stall_watch`) times for its notice and its offer of a lower quality, and it is
/// neither the viewer's Pause nor a seek, which are the states the net and the Auto watchdog
/// stand down for. The net must neither pause the stream a second time nor restart a source that is
/// not parked: a source parked on a full queue is the one state where the hold is never begun.
#[test]
fn the_freeze_net_reports_a_standing_hold_and_neither_pauses_nor_restarts_it() {
    let _serial = plx_base::testlock::serial();
    let ((), text) = logged(|| {
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        run_until_held(&mut rig);
        assert!(!TX.paused.load(Relaxed), "the hold reads as a viewer Pause");
        assert!(!SHARED.seeking.load(Relaxed), "the hold reads as a seek");
        for _ in 0..20 {
            rig.step();
        }
        let notice = Notice::from_u8(SHARED.stall_notice.load(Relaxed));
        assert!(
            matches!(notice, Notice::Network | Notice::Offer),
            "four seconds into a hold the screen says {notice:?}, not the network: {}",
            rig.facts()
        );
        for _ in 0..40 {
            rig.step();
        }
        assert_eq!(pauses(), 1, "the freeze net paused a held stream again: {}", rig.facts());
        assert_eq!(SHARED.dg_stall_restarts.load(Relaxed), 0, "the freeze net restarted a held stream: {}", rig.facts());
        assert!(SHARED.hls_rebuffering.load(Relaxed), "the hold did not stand");
    });
    assert!(!text.contains("stall:"), "the freeze net acted on a held stream: {text}");
}

/// **What never holds.** Each is a stream whose clock is stopped on purpose, still starting, or not
/// a stream a link can starve (or one Quality=Auto's own supervisor watches): zero Pauses of the
/// pump's own.
mod never_held {
    use super::*;

    /// Run `steps` periods and return how many Pause verbs the sink saw.
    fn pauses_over(rig: &mut Rig, steps: usize) -> usize {
        for _ in 0..steps {
            rig.step();
        }
        pauses()
    }

    /// **A start is not judged, through the pump.** 800 ms is just enough for the first Play (the
    /// decoder prime wants 700 ms accepted) and the clock then plays it out: its position never
    /// passes the two seconds below which the clock debt judges nothing, so the start is a start and
    /// not a starvation, and running dry at its tail is left to the existing handling, as before the
    /// hold existed.
    #[test]
    fn a_play_that_never_passes_the_starting_position_is_not_held() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(4 * S / 5, 4 * S / 5).with_virtual_wall();
        ffi_host::reset_volume_probe_for_test();
        assert_eq!(pauses_over(&mut rig, 40), 0, "a Play that never left its starting position was held");
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "it reads as a rebuffer: {}", rig.facts());
        assert!(
            ffi_host::clock_state_for_test().0 >= 500_000_000,
            "PRECONDITION FAILED, not the defect: the Play never started the clock on 800 ms: {}",
            rig.facts()
        );
    }

    /// A start that never reached the prime depth stays in the existing startup handling: its first
    /// Play is still owed, so there is no clock to stall and nothing for the hold to wait on.
    #[test]
    fn a_start_that_never_reached_the_prime_depth_is_not_held() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(S / 2, S / 2).with_virtual_wall();
        ffi_host::reset_volume_probe_for_test();
        assert_eq!(pauses_over(&mut rig, 40), 0, "a start that never reached the prime depth was held");
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "the startup wait reads as a rebuffer: {}", rig.facts());
        assert!(
            !ffi_host::clock_state_for_test().1,
            "PRECONDITION FAILED, not the defect: the clock started on half a second: {}",
            rig.facts()
        );
    }

    /// A link that delivers exactly the file's rate (the producer keeps pace with the clock, the
    /// reserve stays a constant two seconds) is healthy: no hold however long it runs.
    #[test]
    fn a_link_that_keeps_pace_is_never_held() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(2 * S, 2 * S).with_virtual_wall();
        ffi_host::reset_volume_probe_for_test();
        let mut delivered = 2 * S;
        for _ in 0..120 {
            deliver(&mut rig, delivered, delivered + 200_000_000);
            delivered += 200_000_000;
            rig.step();
        }
        assert_eq!(pauses(), 0, "a link delivering the file's rate was held: {}", rig.facts());
        assert!(
            ffi_host::clock_state_for_test().0 > 10 * S,
            "PRECONDITION FAILED, not the defect: the clock never played: {}",
            rig.facts()
        );
    }

    /// A trailer preview is direct play too, and its stall ends the preview, never the readout.
    #[test]
    fn a_trailer_preview_is_never_held() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        crate::route::mark_preview_for_test(&mut rig.ps);
        ffi_host::reset_volume_probe_for_test();
        assert_eq!(pauses_over(&mut rig, 45), 0, "a preview was held");
        assert!(
            ffi_host::clock_state_for_test().0 >= 5 * S,
            "PRECONDITION FAILED, not the defect: the clock never reached the tail: {}",
            rig.facts()
        );
        assert!(!SHARED.hls_rebuffering.load(Relaxed));
    }

    /// **Quality=Auto keeps its own exit.** A direct-play stream the Auto Original supervisor watches
    /// (`cur_auto_original_watched`) is left exactly as it is today: Auto's Original -> HLS exit is
    /// what handles a starved link, and a hold standing in front of it could hold the exit off. The
    /// same starved stream that the hold takes under Quality=Original is not held here, even with the
    /// clock parked at the tail for far longer than the debt needs.
    #[test]
    fn a_stream_the_auto_supervisor_watches_is_never_held() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        crate::route::mark_auto_original_watched_for_test(&mut rig.ps);
        ffi_host::reset_volume_probe_for_test();
        assert_eq!(pauses_over(&mut rig, 60), 0, "an Auto-watched stream was held");
        assert!(
            ffi_host::clock_state_for_test().0 >= 5 * S,
            "PRECONDITION FAILED, not the defect: the clock never reached the tail: {}",
            rig.facts()
        );
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "an Auto-watched stream reads Buffering");
    }

    /// The viewer's own Pause is the only Pause a paused clock gets: running dry under it is not a
    /// starvation to hold, and the viewer who pressed Pause is not made to read Buffering.
    #[test]
    fn a_user_pause_is_not_a_starvation() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::with_media(6 * S, 6 * S).with_virtual_wall();
        for _ in 0..8 {
            rig.step();
        }
        ffi_host::reset_volume_probe_for_test();
        assert!(crate::player::pause(&mut rig.pa), "the viewer's Pause is accepted");
        let at_pause = pauses();
        assert_eq!(pauses_over(&mut rig, 40), at_pause, "a paused stream was held a second time");
        assert!(!SHARED.hls_rebuffering.load(Relaxed), "a paused stream reads Buffering");
    }
}
