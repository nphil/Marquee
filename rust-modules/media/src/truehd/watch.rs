//! The helper's `audio.passthrough` topic, asked in the background for the whole process, so a
//! plan reads readiness without a call of its own.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::contract::{Helper, Loopback, Topic};

/// While the helper answers, a change reaches the next plan within this.
const ANSWERING: Duration = Duration::from_secs(2);
/// A helper that is not installed is asked again only this often.
const ABSENT: Duration = Duration::from_secs(10);

static LATEST: Mutex<Option<Topic>> = Mutex::new(None);
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
    let mut latest = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    let said = |t: &Option<Topic>| {
        t.as_ref().map(|t| (t.state, t.ready, t.reason.clone(), t.session_adec, t.after.clone()))
    };
    if said(&latest) != said(&topic) {
        match &topic {
            Some(t) => crate::player::log(&format!(
                "truehd: helper {:?}, ready {} ({}), session ADEC {:?}, after {:?}",
                t.state, t.ready, t.reason, t.session_adec, t.after
            )),
            None => crate::player::log("truehd: helper not answering"),
        }
    }
    *latest = topic;
}

/// The helper's last answer; `None` while it has not answered.
pub fn topic() -> Option<Topic> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
pub fn set_for_test(topic: Option<Topic>) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = topic;
}
