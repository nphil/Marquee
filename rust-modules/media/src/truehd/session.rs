//! One carrier session with the helper: arm it with this process's audio renderer, keep the route
//! and the eARC controls sampled, release it at the end. A worker does the I/O; the feeder and the
//! sink probe read what it found ([`Facts`]).
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::alsa::{Controls, Earc};
use super::contract::{Carrier, Helper, Loopback};
use super::gate::Facts;

/// The contract's retry period for an arm, and the route's sample period.
const ARM_EVERY: Duration = Duration::from_millis(100);
const ROUTE_EVERY: Duration = Duration::from_millis(100);
/// The controls are read once a burst.
const TICK: Duration = Duration::from_millis(20);
/// How long the process waits at exit for the helper to take the route back.
const EXIT_WAIT: Duration = Duration::from_millis(300);

/// What the session's three sides share.
pub struct Shared {
    facts: Mutex<Facts>,
    /// The gate's epoch: the worker arms again whenever it moves.
    epoch: AtomicU64,
    /// LG's sink saw a flush the feeder has not restarted for yet.
    flushed: AtomicBool,
    stop: Mutex<bool>,
    wake: Condvar,
    /// The carrier's shape, as the helper named it at the start.
    pub carrier: Carrier,
}

impl Shared {
    pub fn facts(&self) -> Facts {
        *self.facts.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// The helper has armed the route for the epoch now in force.
    pub fn armed(&self) -> bool {
        self.facts().armed_epoch == Some(self.epoch())
    }

    /// The sink saw a flush: the feeder restarts the gate.
    pub fn note_flush(&self) {
        self.flushed.store(true, Ordering::Release);
    }
}

/// The session the sink hook acts for; `None` while nothing feeds the carrier.
static ACTIVE: Mutex<Option<Arc<Shared>>> = Mutex::new(None);
/// A session has armed since the process started, so exit owes the helper a release.
static ARMED_ONCE: AtomicBool = AtomicBool::new(false);

pub fn active() -> Option<Arc<Shared>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The live session; dropping it stops the worker, which releases the route.
pub struct Session {
    shared: Arc<Shared>,
}

impl Session {
    /// `None` when the eARC controls cannot be read (no carrier burst could ever be checked) or
    /// the worker cannot start.
    pub fn start(carrier: Carrier) -> Option<Session> {
        let controls = Controls::open()?;
        let shared = Arc::new(Shared {
            facts: Mutex::new(Facts::default()),
            epoch: AtomicU64::new(1),
            flushed: AtomicBool::new(false),
            stop: Mutex::new(false),
            wake: Condvar::new(),
            carrier,
        });
        let worker = Arc::clone(&shared);
        if !plx_base::task::spawn_small("truehd route", move || run(&worker, &Loopback, &controls)) {
            return None;
        }
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&shared));
        Some(Session { shared })
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// The gate restarted at `epoch`: release and arm again.
    pub fn rearm(&self, epoch: u64) {
        self.shared.epoch.store(epoch, Ordering::Release);
        self.shared.wake.notify_all();
    }

    pub fn take_flush(&self) -> bool {
        self.shared.flushed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
impl Session {
    /// A session with no worker and no helper: its facts stay empty, so the gate answers zeros.
    pub fn for_test(carrier: Carrier) -> Session {
        let shared = Shared {
            facts: Mutex::new(Facts::default()),
            epoch: AtomicU64::new(1),
            flushed: AtomicBool::new(false),
            stop: Mutex::new(false),
            wake: Condvar::new(),
            carrier,
        };
        Session { shared: Arc::new(shared) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active.as_ref().is_some_and(|a| Arc::ptr_eq(a, &self.shared)) {
            *active = None;
        }
        drop(active);
        *self.shared.stop.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.shared.wake.notify_all();
    }
}

fn run(shared: &Shared, helper: &dyn Helper, controls: &Controls) {
    let mut driver = Driver::new(std::process::id());
    let mut facts = Facts::default();
    loop {
        let epoch = shared.epoch();
        driver.step(Instant::now(), epoch, helper, renderer_tail, controls.read(), &mut facts);
        *shared.facts.lock().unwrap_or_else(|e| e.into_inner()) = facts;
        let stop = shared.stop.lock().unwrap_or_else(|e| e.into_inner());
        let (stop, _) = shared
            .wake
            .wait_timeout_while(stop, TICK, |stopped| !*stopped && shared.epoch() == epoch)
            .unwrap_or_else(|e| e.into_inner());
        if *stop {
            break;
        }
    }
    if driver.armed_for.is_some() {
        helper.release(driver.pid);
        crate::player::log("truehd: route released");
    }
}

/// The worker's schedule, apart from its thread and its clock.
struct Driver {
    pid: u32,
    /// The epoch the helper confirmed the last arm for.
    armed_for: Option<u64>,
    next_arm: Option<Instant>,
    next_route: Option<Instant>,
    /// The last reason logged, so a repeated one is not.
    said: String,
}

impl Driver {
    fn new(pid: u32) -> Driver {
        Driver { pid, armed_for: None, next_arm: None, next_route: None, said: String::new() }
    }

    fn step(
        &mut self, now: Instant, epoch: u64, helper: &dyn Helper, tail: impl FnOnce() -> Result<u32, &'static str>,
        alsa: Option<Earc>, facts: &mut Facts,
    ) {
        if let Some(earc) = alsa {
            facts.alsa = Some((earc, now));
        }
        if self.armed_for.is_some_and(|armed| armed != epoch) {
            // A seek, a resume or a flush: the route is given back and taken again for the new epoch.
            helper.release(self.pid);
            self.armed_for = None;
            self.next_arm = None;
        }
        if self.armed_for.is_none() && self.next_arm.is_none_or(|t| now >= t) {
            self.next_arm = Some(now + ARM_EVERY);
            match tail() {
                Err(why) => self.say(why),
                Ok(tail) => match helper.arm(self.pid, tail) {
                    Some(answer) if answer.armed => {
                        self.armed_for = Some(epoch);
                        facts.armed_epoch = Some(epoch);
                        // Only a route sampled after this arm speaks for it.
                        facts.route = None;
                        self.next_route = None;
                        ARMED_ONCE.store(true, Ordering::Release);
                        crate::player::log(&format!("truehd: armed renderer tail {tail:08x} (epoch {epoch})"));
                    }
                    Some(answer) => self.say(&answer.reason),
                    None => self.say("the helper did not answer the arm"),
                },
            }
        }
        if self.armed_for == Some(epoch) && self.next_route.is_none_or(|t| now >= t) {
            self.next_route = Some(now + ROUTE_EVERY);
            if let Some((route, reason)) = helper.route() {
                facts.route = Some((route, now));
                self.say(&reason);
            }
        }
    }

    fn say(&mut self, why: &str) {
        if !why.is_empty() && why != self.said {
            crate::player::log(&format!("truehd: {why}"));
            why.clone_into(&mut self.said);
        }
    }
}

/// The bypass tail of this process's audio renderer: its one `/dev/lg/aren0` descriptor answers
/// `ioctl 0x80045223` with it, as upstream reads it.
#[cfg(target_os = "linux")]
fn renderer_tail() -> Result<u32, &'static str> {
    use std::os::fd::AsRawFd;
    const TAIL: u32 = 0x8004_5223;
    let mut found = None;
    for entry in std::fs::read_dir("/proc/self/fd").map_err(|_| "no descriptor list")?.flatten() {
        if std::fs::read_link(entry.path()).is_ok_and(|p| p.as_os_str() == "/dev/lg/aren0") {
            if found.is_some() {
                return Err("more than one audio renderer is open");
            }
            found = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok());
        }
    }
    let fd = found.ok_or("no audio renderer is open yet")?;
    // The descriptor is LG's; a duplicate keeps it from being closed and reused under the ioctl.
    let own = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if own < 0 {
        return Err("the audio renderer closed");
    }
    let own = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(own) };
    let mut tail: u32 = 0;
    if unsafe { libc::ioctl(own.as_raw_fd(), TAIL as _, &mut tail) } != 0 {
        return Err("the audio renderer has no bypass tail");
    }
    if tail == 0 || tail == u32::MAX {
        return Err("the audio renderer has no bypass tail");
    }
    Ok(tail)
}

#[cfg(not(target_os = "linux"))]
fn renderer_tail() -> Result<u32, &'static str> {
    Err("no audio renderer on this platform")
}

/// At exit: give the route back now rather than leave LG's mixer bypassed for a process that is
/// gone. Bounded, since the helper may not answer.
pub fn release_at_exit() {
    if !ARMED_ONCE.load(Ordering::Acquire) {
        return;
    }
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let signal = Arc::clone(&done);
    let spawned = plx_base::task::spawn_small("truehd exit", move || {
        Loopback.release(std::process::id());
        *signal.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        signal.1.notify_all();
    });
    if spawned {
        let held = done.0.lock().unwrap_or_else(|e| e.into_inner());
        let _ = done.1.wait_timeout_while(held, EXIT_WAIT, |finished| !*finished);
    }
}

#[cfg(test)]
mod tests {
    use super::super::contract::{Armed, Route, Topic};
    use super::*;

    /// A helper that refuses `refusals` arms, then arms, and records what it was asked.
    #[derive(Default)]
    struct Fake {
        refusals: Mutex<u32>,
        calls: Mutex<Vec<String>>,
    }

    impl Helper for Fake {
        fn topic(&self) -> Option<Topic> {
            None
        }

        fn arm(&self, pid: u32, tail: u32) -> Option<Armed> {
            self.calls.lock().unwrap().push(format!("arm {pid} {tail:08x}"));
            let mut refusals = self.refusals.lock().unwrap();
            let armed = *refusals == 0;
            *refusals = refusals.saturating_sub(1);
            Some(Armed { armed, reason: if armed { String::new() } else { "no converter".into() } })
        }

        fn route(&self) -> Option<(Route, String)> {
            self.calls.lock().unwrap().push("route".into());
            Some((Route { armed: true, bypassing: true, mixed: false, age_ms: 10 }, String::new()))
        }

        fn release(&self, pid: u32) {
            self.calls.lock().unwrap().push(format!("release {pid}"));
        }
    }

    #[test]
    fn arms_every_100_ms_until_armed_then_samples_and_rearms_per_epoch() {
        let helper = Fake { refusals: Mutex::new(1), ..Fake::default() };
        let mut d = Driver::new(7);
        let mut facts = Facts::default();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        d.step(at(0), 1, &helper, || Err("no audio renderer is open yet"), None, &mut facts);
        d.step(at(20), 1, &helper, || Ok(0x2a), None, &mut facts);
        assert!(helper.calls.lock().unwrap().is_empty(), "the next try waits out the period");
        d.step(at(100), 1, &helper, || Ok(0x2a), None, &mut facts);
        d.step(at(200), 1, &helper, || Ok(0x2a), None, &mut facts);
        assert_eq!(facts.armed_epoch, Some(1));
        assert!(facts.route.is_some(), "the route is sampled straight after the arm");
        d.step(at(220), 1, &helper, || Ok(0x2a), None, &mut facts);
        d.step(at(240), 2, &helper, || Ok(0x2b), None, &mut facts);
        assert_eq!(facts.armed_epoch, Some(2));
        assert_eq!(*helper.calls.lock().unwrap(), ["arm 7 0000002a", "arm 7 0000002a", "route", "release 7", "arm 7 0000002b", "route"]);
    }
}
