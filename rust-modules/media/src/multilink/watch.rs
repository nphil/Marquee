//! **The helper's state, fetched off the demux thread.**
//!
//! One small worker per reader asks the helper on loopback and publishes its last answer. The
//! reader copies the answer when it changes and never waits for one, so a helper that is absent
//! or slow costs a film nothing but this worker's time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::status::{self, Status, Unusable};

/// Loopback answers in well under a millisecond; a helper that has stopped answering is not
/// waited for.
const WIRE: plx_net::net::Timeouts =
    plx_net::net::Timeouts { connect_s: 1, total_s: 0, total_ms: 250, low_speed_bps: 0, low_speed_s: 0 };
/// The state is a few hundred bytes.
const ANSWER_BYTES: usize = 16 << 10;
/// The helper samples every 500 ms and vouches for each sample for a second: asked this often,
/// the newest answer in hand is never older than it vouches for.
const ASK_ACTIVE: Duration = Duration::from_millis(250);
/// A helper that is there and offers nothing (the plugin off, or one link only).
const ASK_IDLE: Duration = Duration::from_secs(2);
/// No helper, or none with this plugin.
const ASK_ABSENT: Duration = Duration::from_secs(10);
/// This many answers in a row must be lost before a helper that was answering counts as gone.
/// Until then what it last said stands, and goes stale on its own terms.
const LOST_BEFORE_ABSENT: u32 = 4;

struct Shared {
    latest: Mutex<Result<Status, Unusable>>,
    /// How many times `latest` has changed: 0 until the first answer.
    revision: AtomicU64,
    stop: Mutex<bool>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The worker's owner: dropping it ends the asking.
pub struct Watch {
    shared: Arc<Shared>,
}

impl Watch {
    /// Start asking. `None` when the worker could not start.
    pub fn start() -> Option<Watch> {
        let shared = Arc::new(Shared {
            latest: Mutex::new(Err(Unusable::Missing)),
            revision: AtomicU64::new(0),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let theirs = shared.clone();
        plx_base::task::spawn_small("multilink state", move || theirs.run()).then_some(Watch { shared })
    }

    /// Changes with every new answer; 0 before the first.
    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::Acquire)
    }

    pub fn latest(&self) -> Result<Status, Unusable> {
        lock(&self.shared.latest).clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        *lock(&self.shared.stop) = true;
        self.shared.wake.notify_all();
    }
}

impl Shared {
    fn run(&self) {
        let headers = ["X-Luma-Client: com.beb.plxnative".to_owned()];
        let mut lost = 0;
        loop {
            let answer = ask(&headers);
            lost = if answer == Err(Unusable::Missing) { lost + 1 } else { 0 };
            let pause = {
                let mut latest = lock(&self.latest);
                let stands = lost > 0 && lost < LOST_BEFORE_ABSENT && latest.is_ok();
                if !stands && (*latest != answer || self.revision.load(Ordering::Relaxed) == 0) {
                    *latest = answer;
                    self.revision.fetch_add(1, Ordering::Release);
                }
                match &*latest {
                    Ok(_) => ASK_ACTIVE,
                    Err(Unusable::Missing) => ASK_ABSENT,
                    Err(_) => ASK_IDLE,
                }
            };
            let stop = lock(&self.stop);
            if *stop || *self.wake.wait_timeout(stop, pause).unwrap_or_else(PoisonError::into_inner).0 {
                return;
            }
        }
    }
}

/// One question. No answer, or an answer that is not the state, reads as no plugin.
fn ask(headers: &[String]) -> Result<Status, Unusable> {
    match plx_net::net::request(status::URL, headers, "GET", None, WIRE, false, Some(ANSWER_BYTES), None) {
        Some(reply) if reply.status == 200 => status::parse(&reply.body),
        _ => Err(Unusable::Missing),
    }
}

/// Milliseconds of the clock the helper stamps its state with (`CLOCK_BOOTTIME`).
#[cfg(target_os = "linux")]
pub fn uptime_ms() -> Option<u64> {
    let mut at: libc::timespec = unsafe { std::mem::zeroed() };
    (unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut at) } == 0).then(|| at.tv_sec as u64 * 1000 + at.tv_nsec as u64 / 1_000_000)
}

/// The helper runs on the TV alone. Without its clock no state is fresh.
#[cfg(not(target_os = "linux"))]
pub fn uptime_ms() -> Option<u64> {
    None
}
