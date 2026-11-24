//! The helper's `audio.passthrough` topic, asked in the background for the whole process, so a
//! plan reads readiness without a call of its own.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::contract::{Helper, Loopback, Topic};

/// While the helper answers, a change reaches the next plan within this.
const ANSWERING: Duration = Duration::from_secs(2);
/// A helper that is not installed is asked again only this often.
const ABSENT: Duration = Duration::from_secs(10);

/// The longest a reading is trusted whatever validity the helper names.
const VALID_LONGEST: Duration = ANSWERING;

/// One answer (or the lack of one) and when it arrived.
struct Reading {
    topic: Option<Topic>,
    at: Instant,
}

type Slot = Mutex<Option<Reading>>;

static LATEST: Slot = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Start asking; later calls do nothing.
pub fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = plx_base::task::spawn_small("truehd readiness", || loop {
        let topic = Loopback.topic();
        let wait = if topic.is_some() { ANSWERING } else { ABSENT };
        publish(topic);
        std::thread::sleep(wait);
    });
    if !spawned {
        STARTED.store(false, Ordering::Release);
    }
}

fn publish(topic: Option<Topic>) {
    publish_to(&LATEST, topic, Instant::now());
}

fn publish_to(slot: &Slot, topic: Option<Topic>, at: Instant) {
    let mut latest = slot.lock().unwrap_or_else(|e| e.into_inner());
    let said = |t: &Option<Topic>| {
        t.as_ref().map(|t| (t.state, t.ready, t.reason.clone(), t.session_adec, t.after.clone()))
    };
    if said(&latest.as_ref().and_then(|r| r.topic.clone())) != said(&topic) {
        match &topic {
            Some(t) => crate::player::log(&format!(
                "truehd: helper {:?}, ready {} ({}), session ADEC {:?}, after {:?}",
                t.state, t.ready, t.reason, t.session_adec, t.after
            )),
            None => crate::player::log("truehd: helper not answering"),
        }
    }
    *latest = Some(Reading { topic, at });
}

/// The helper's last answer; `None` while it has not answered.
pub fn topic() -> Option<Topic> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|r| r.topic.clone())
}

/// The last answer is still inside the validity the helper named for it. A helper that did not
/// answer, or named none, is never fresh.
fn fresh(slot: &Slot, now: Instant) -> bool {
    let latest = slot.lock().unwrap_or_else(|e| e.into_inner());
    latest.as_ref().is_some_and(|r| {
        r.topic.as_ref().is_some_and(|t| {
            now.saturating_duration_since(r.at) <= Duration::from_millis(t.valid_for_ms).min(VALID_LONGEST)
        })
    })
}

/// The last answer is still good.
pub fn is_fresh() -> bool {
    fresh(&LATEST, Instant::now())
}

/// Reads the helper once, now, unless the last answer is still fresh; true when it asked. The
/// answer replaces the last one, so a helper that does not answer in time reads as not ready.
fn refresh(slot: &Slot, helper: &dyn Helper, now: Instant) -> bool {
    if fresh(slot, now) {
        return false;
    }
    publish_to(slot, helper.topic(), Instant::now());
    true
}

/// [`refresh`] on this process's helper, for a worker that is about to plan a TrueHD track
/// before the background read has caught up (the first play after launch or after the plugin
/// was switched on). One call, bounded by [`contract`](super::contract)'s wire timeout; never on
/// the frame thread.
pub fn refresh_if_stale() {
    if !is_fresh() {
        let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("truehd readiness read") });
        refresh(&LATEST, &Loopback, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A helper that gives one answer and counts how often it was asked.
    struct Fake {
        answer: Option<Topic>,
        asked: AtomicU32,
    }

    impl Fake {
        fn new(answer: Option<Topic>) -> Fake {
            Fake { answer, asked: AtomicU32::new(0) }
        }

        fn asked(&self) -> u32 {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Helper for Fake {
        fn topic(&self) -> Option<Topic> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            self.answer.clone()
        }
        fn arm(&self, _: u32, _: u32, _: u8) -> Option<super::super::contract::Armed> {
            None
        }
        fn route(&self) -> Option<(super::super::contract::Route, String)> {
            None
        }
        fn flowing(&self, _: u32) -> Option<(bool, String)> {
            None
        }
        fn release(&self, _: u32) {}
    }

    /// The helper's real answer while the plugin is on and the TV ready, `validForMs` 300.
    fn ready() -> Topic {
        super::super::contract::parse_topic(
            br#"{"ok":true,"topic":"audio.passthrough","value":{"schema":1,"state":"ready","ready":true,"validForMs":300}}"#,
        )
        .unwrap()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_reading_inside_its_validity_is_used_without_a_call() {
        let t0 = Instant::now();
        let slot: Slot = Mutex::new(None);
        publish_to(&slot, Some(ready()), t0);
        let helper = Fake::new(None);
        assert!(!refresh(&slot, &helper, t0 + ms(300)));
        assert_eq!(helper.asked(), 0);
        assert!(slot.lock().unwrap().as_ref().unwrap().topic.as_ref().unwrap().admits(1));
    }

    #[test]
    fn a_stale_or_missing_reading_is_read_once_before_the_decision() {
        let t0 = Instant::now();
        // Never read, as on the first play after launch.
        let slot: Slot = Mutex::new(None);
        let helper = Fake::new(Some(ready()));
        assert!(refresh(&slot, &helper, t0));
        assert_eq!(helper.asked(), 1);
        assert!(fresh(&slot, Instant::now()), "the answer just read is fresh");

        // Older than the helper's validForMs.
        let slot: Slot = Mutex::new(None);
        publish_to(&slot, Some(Topic { state: super::super::contract::State::Off, ready: false, ..ready() }), t0);
        assert!(!fresh(&slot, t0 + ms(301)));
        let helper = Fake::new(Some(ready()));
        assert!(refresh(&slot, &helper, t0 + ms(301)));
        assert!(slot.lock().unwrap().as_ref().unwrap().topic.as_ref().unwrap().admits(1), "the plugin came on since");
    }

    #[test]
    fn a_helper_that_does_not_answer_leaves_the_normal_track() {
        let t0 = Instant::now();
        let slot: Slot = Mutex::new(None);
        publish_to(&slot, Some(ready()), t0);
        let helper = Fake::new(None);
        assert!(refresh(&slot, &helper, t0 + ms(5000)));
        let latest = slot.lock().unwrap();
        assert!(latest.as_ref().unwrap().topic.is_none(), "a stale Ready is not kept when the helper cannot confirm it");
        drop(latest);
        assert!(!fresh(&slot, Instant::now()), "a missing answer is read again next time");
    }

    #[test]
    fn a_reading_that_names_no_validity_is_never_fresh() {
        let t0 = Instant::now();
        let slot: Slot = Mutex::new(None);
        publish_to(&slot, Some(Topic { valid_for_ms: 0, ..ready() }), t0);
        assert!(!fresh(&slot, t0 + ms(1)));
        publish_to(&slot, Some(Topic { valid_for_ms: 3_600_000, ..ready() }), t0);
        assert!(fresh(&slot, t0 + ms(1999)) && !fresh(&slot, t0 + ms(2001)), "capped at the watch's own period");
    }
}
