//! One carrier session with the helper: arm it with this process's audio renderer and the decoder
//! input LG granted it, keep the route sampled, release it once the sink is closed. A worker does
//! the I/O; the feeder and the sink probe read what it found ([`Facts`]).
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::contract::{Carrier, Helper, Loopback};
use super::gate::Facts;

/// The contract's retry period for an arm.
const ARM_EVERY: Duration = Duration::from_millis(100);
/// The helper samples the route every 100 ms and its answer counts as stale past 300 ms, so the
/// route is asked twice as often as it is sampled.
const ROUTE_EVERY: Duration = Duration::from_millis(50);
/// How often the worker looks for a flush, a stop or a due call.
const TICK: Duration = Duration::from_millis(50);
/// The helper puts the TV back on `release` and only unmutes exactly once the sink is closed (it
/// waits up to 2 s for that itself), so the worker waits for the close first.
const SINK_CLOSE_WAIT: Duration = Duration::from_millis(2500);
const SINK_CLOSE_POLL: Duration = Duration::from_millis(50);
/// How long the process waits at exit for a worker to finish its release.
const EXIT_WAIT: Duration = Duration::from_millis(500);
/// The highest decoder input the helper accepts.
const MAX_ADEC: i32 = 7;

/// The decoder input LG's resource manager granted this process's renderer, as its audio sink
/// reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granted {
    /// The sink has not been read yet, or has not been told its resource.
    Unknown,
    /// The sink's `index`: ADEC 0 to 7.
    Decoder(u8),
    /// The sink cannot name a decoder input, so the route is never armed for it.
    Refused(&'static str),
}

impl Granted {
    /// What the sink's `index` property and `mixer` property say. A mixer channel's `index` is a
    /// mixer port, not a decoder input, and the helper would connect the wrong one.
    pub fn from_sink(index: Option<i32>, mixer: Option<bool>) -> Granted {
        match (index, mixer) {
            (_, Some(true)) => Granted::Refused("the audio sink plays through a mixer channel, not a decoder input"),
            (None, _) => Granted::Refused("the audio sink does not name its decoder input"),
            (Some(-1), _) => Granted::Unknown,
            (Some(i), _) if (0..=MAX_ADEC).contains(&i) => Granted::Decoder(i as u8),
            (Some(_), _) => Granted::Refused("the audio sink's decoder input is out of range"),
        }
    }
}

/// What the session's three sides share.
pub struct Shared {
    facts: Mutex<Facts>,
    /// The gate's epoch: the worker arms again whenever it moves.
    epoch: AtomicU64,
    /// LG's sink saw a flush the feeder has not restarted for yet.
    flushed: AtomicBool,
    /// The decoder input the sink reported; the worker arms with it.
    granted: Mutex<Granted>,
    stop: Mutex<bool>,
    wake: Condvar,
    /// The carrier's shape, as the helper named it at the start.
    pub carrier: Carrier,
}

impl Shared {
    fn new(carrier: Carrier) -> Shared {
        Shared {
            facts: Mutex::new(Facts::default()),
            epoch: AtomicU64::new(1),
            flushed: AtomicBool::new(false),
            granted: Mutex::new(Granted::Unknown),
            stop: Mutex::new(false),
            wake: Condvar::new(),
            carrier,
        }
    }

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

    pub fn granted(&self) -> Granted {
        *self.granted.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the sink says its decoder input is; the worker arms as soon as it is a decoder.
    pub fn note_granted(&self, now: Granted) {
        let mut held = self.granted.lock().unwrap_or_else(|e| e.into_inner());
        if *held != now {
            match now {
                Granted::Decoder(adec) => crate::player::log(&format!("truehd: LG's audio sink holds decoder input ADEC{adec}")),
                Granted::Refused(why) => crate::player::log(&format!("truehd: {why}")),
                Granted::Unknown => {}
            }
            *held = now;
        }
        drop(held);
        self.wake.notify_all();
    }
}

/// The session the sink hook acts for; `None` while nothing feeds the carrier.
static ACTIVE: Mutex<Option<Arc<Shared>>> = Mutex::new(None);
/// A worker holds this from its start until its route is back, so a session that follows an
/// earlier one (a track switch reloads the Load) arms only after the earlier release.
static SLOT: Mutex<()> = Mutex::new(());

pub fn active() -> Option<Arc<Shared>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The live session; dropping it stops the worker, which releases the route.
pub struct Session {
    shared: Arc<Shared>,
}

impl Session {
    /// `None` when the worker cannot start.
    pub fn start(carrier: Carrier) -> Option<Session> {
        let shared = Arc::new(Shared::new(carrier));
        let worker = Arc::clone(&shared);
        if !plx_base::task::spawn_small("truehd route", move || run(&worker, &Loopback)) {
            return None;
        }
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&shared));
        Some(Session { shared })
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// The gate restarted at `epoch`: arm again for it. The route is not given back; the helper
    /// re-arms the session it holds, and a release belongs to the end of the sink.
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
        Session { shared: Arc::new(Shared::new(carrier)) }
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

fn run(shared: &Shared, helper: &dyn Helper) {
    let _slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let mut driver = Driver::new(std::process::id());
    let mut facts = Facts::default();
    loop {
        if *shared.stop.lock().unwrap_or_else(|e| e.into_inner()) {
            break;
        }
        let epoch = shared.epoch();
        driver.step(Instant::now(), epoch, helper, || renderer(shared, renderer_tail), &mut facts);
        *shared.facts.lock().unwrap_or_else(|e| e.into_inner()) = facts;
        let stop = shared.stop.lock().unwrap_or_else(|e| e.into_inner());
        let _ = shared
            .wake
            .wait_timeout_while(stop, TICK, |stopped| !*stopped && shared.epoch() == epoch)
            .unwrap_or_else(|e| e.into_inner());
    }
    driver.finish(helper, renderer_open, std::thread::sleep);
}

/// The renderer's bypass tail and the decoder input its sink holds.
fn renderer(shared: &Shared, tail: impl FnOnce() -> Result<u32, &'static str>) -> Result<(u32, u8), &'static str> {
    let tail = tail()?;
    match shared.granted() {
        Granted::Decoder(adec) => Ok((tail, adec)),
        Granted::Unknown => Err("the audio sink has not named its decoder input yet"),
        Granted::Refused(why) => Err(why),
    }
}

/// The worker's schedule, apart from its thread and its clock.
struct Driver {
    pid: u32,
    /// The helper holds a session for this process that has not been released.
    held: bool,
    /// The epoch the helper confirmed the last arm for.
    armed_for: Option<u64>,
    next_arm: Option<Instant>,
    next_route: Option<Instant>,
    /// The last line logged from the helper, so a repeated one is not.
    said: String,
}

impl Driver {
    fn new(pid: u32) -> Driver {
        Driver { pid, held: false, armed_for: None, next_arm: None, next_route: None, said: String::new() }
    }

    fn step(
        &mut self, now: Instant, epoch: u64, helper: &dyn Helper,
        renderer: impl FnOnce() -> Result<(u32, u8), &'static str>, facts: &mut Facts,
    ) {
        if self.armed_for.is_some_and(|armed| armed != epoch) {
            // A seek, a resume or a flush: LG builds a new renderer and the helper re-arms the
            // session it holds, so the arm is asked again for the new epoch.
            self.armed_for = None;
            self.next_arm = None;
        }
        if self.armed_for.is_none() && self.next_arm.is_none_or(|t| now >= t) {
            self.next_arm = Some(now + ARM_EVERY);
            match renderer() {
                Err(why) => self.say(why),
                Ok((tail, adec)) => match helper.arm(self.pid, tail, adec) {
                    Some(answer) if answer.armed => {
                        self.armed_for = Some(epoch);
                        self.held = true;
                        facts.armed_epoch = Some(epoch);
                        // Only a route sampled after this arm speaks for it.
                        facts.route = None;
                        self.next_route = None;
                        crate::player::log(&format!("truehd: armed renderer tail {tail:08x} on ADEC{adec} (epoch {epoch})"));
                    }
                    Some(answer) => self.say(&answer.reason),
                    None => self.say("the helper did not answer the arm"),
                },
            }
        }
        if self.armed_for == Some(epoch) && self.next_route.is_none_or(|t| now >= t) {
            self.next_route = Some(now + ROUTE_EVERY);
            if let Some((route, note)) = helper.route() {
                facts.route = Some((route, now));
                self.say(&note);
            }
        }
    }

    /// The end of the session: once the sink is closed (or has had `SINK_CLOSE_WAIT` to close) the
    /// route goes back.
    fn finish(&mut self, helper: &dyn Helper, mut sink_open: impl FnMut() -> bool, mut sleep: impl FnMut(Duration)) {
        if !self.held {
            return;
        }
        let mut waited = Duration::ZERO;
        while sink_open() && waited < SINK_CLOSE_WAIT {
            sleep(SINK_CLOSE_POLL);
            waited += SINK_CLOSE_POLL;
        }
        if sink_open() {
            crate::player::log("truehd: releasing the route with the audio sink still open");
        }
        helper.release(self.pid);
        self.held = false;
        crate::player::log("truehd: route released");
    }

    fn say(&mut self, why: &str) {
        if !why.is_empty() && why != self.said {
            crate::player::log(&format!("truehd: {why}"));
            why.clone_into(&mut self.said);
        }
    }
}

/// The descriptors this process holds on LG's audio renderer.
#[cfg(target_os = "linux")]
fn renderer_fds() -> Result<Vec<i32>, &'static str> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc/self/fd").map_err(|_| "no descriptor list")?.flatten() {
        if std::fs::read_link(entry.path()).is_ok_and(|p| p.as_os_str() == "/dev/lg/aren0") {
            found.extend(entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()));
        }
    }
    Ok(found)
}

/// The bypass tail of this process's audio renderer: its one `/dev/lg/aren0` descriptor answers
/// `ioctl 0x80045223` with it, as upstream reads it.
#[cfg(target_os = "linux")]
fn renderer_tail() -> Result<u32, &'static str> {
    use std::os::fd::AsRawFd;
    const TAIL: u32 = 0x8004_5223;
    let fds = renderer_fds()?;
    let fd = match fds[..] {
        [] => return Err("no audio renderer is open yet"),
        [fd] => fd,
        _ => return Err("more than one audio renderer is open"),
    };
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

/// Whether this process still holds a renderer: the sink is not closed.
#[cfg(target_os = "linux")]
fn renderer_open() -> bool {
    renderer_fds().is_ok_and(|fds| !fds.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn renderer_tail() -> Result<u32, &'static str> {
    Err("no audio renderer on this platform")
}

#[cfg(not(target_os = "linux"))]
fn renderer_open() -> bool {
    false
}

/// At exit: let a worker that is putting the route back finish, for a moment. The helper puts the
/// TV back by itself when the process is gone, so this only shortens the time LG's mixer stays
/// bypassed.
pub fn release_at_exit() {
    let end = Instant::now() + EXIT_WAIT;
    while SLOT.try_lock().is_err() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
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

        fn arm(&self, pid: u32, tail: u32, adec: u8) -> Option<Armed> {
            self.calls.lock().unwrap().push(format!("arm {pid} {tail:08x} adec{adec}"));
            let mut refusals = self.refusals.lock().unwrap();
            let armed = *refusals == 0;
            *refusals = refusals.saturating_sub(1);
            Some(Armed { armed, reason: if armed { String::new() } else { "no converter".into() } })
        }

        fn route(&self) -> Option<(Route, String)> {
            self.calls.lock().unwrap().push("route".into());
            let route = Route { armed: true, bypassing: true, mixed: false, connected: false, audible: false, age_ms: 10 };
            Some((route, String::new()))
        }

        fn release(&self, pid: u32) {
            self.calls.lock().unwrap().push(format!("release {pid}"));
        }
    }

    #[test]
    fn arms_every_100_ms_until_armed_then_samples_and_rearms_per_epoch_without_releasing() {
        let helper = Fake { refusals: Mutex::new(1), ..Fake::default() };
        let mut d = Driver::new(7);
        let mut facts = Facts::default();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        d.step(at(0), 1, &helper, || Err("no audio renderer is open yet"), &mut facts);
        d.step(at(20), 1, &helper, || Ok((0x2a, 1)), &mut facts);
        assert!(helper.calls.lock().unwrap().is_empty(), "the next try waits out the period");
        d.step(at(100), 1, &helper, || Ok((0x2a, 1)), &mut facts);
        d.step(at(200), 1, &helper, || Ok((0x2a, 1)), &mut facts);
        assert_eq!(facts.armed_epoch, Some(1));
        assert!(facts.route.is_some(), "the route is sampled straight after the arm");
        d.step(at(220), 1, &helper, || Ok((0x2a, 1)), &mut facts);
        d.step(at(240), 2, &helper, || Ok((0x2b, 1)), &mut facts);
        assert_eq!(facts.armed_epoch, Some(2));
        assert_eq!(
            *helper.calls.lock().unwrap(),
            ["arm 7 0000002a adec1", "arm 7 0000002a adec1", "route", "arm 7 0000002b adec1", "route"],
            "a flush asks for the arm again and gives nothing back"
        );
    }

    #[test]
    fn the_arm_waits_for_the_decoder_input_the_sink_names() {
        let shared = Shared::new(Carrier::default());
        let tail = || Ok(0x2a);
        assert_eq!(renderer(&shared, tail), Err("the audio sink has not named its decoder input yet"));
        shared.note_granted(Granted::from_sink(Some(1), Some(true)));
        assert!(matches!(renderer(&shared, tail), Err(why) if why.contains("mixer channel")));
        shared.note_granted(Granted::from_sink(Some(1), Some(false)));
        assert_eq!(renderer(&shared, tail), Ok((0x2a, 1)));
        assert_eq!(renderer(&shared, || Err("no audio renderer is open yet")), Err("no audio renderer is open yet"));
    }

    #[test]
    fn the_sink_s_index_and_mixer_flag_name_the_decoder_input() {
        assert_eq!(Granted::from_sink(Some(1), Some(false)), Granted::Decoder(1));
        assert_eq!(Granted::from_sink(Some(0), None), Granted::Decoder(0));
        assert_eq!(Granted::from_sink(Some(7), Some(false)), Granted::Decoder(7));
        assert_eq!(Granted::from_sink(Some(-1), Some(false)), Granted::Unknown, "the resource has not reached the sink");
        assert!(matches!(Granted::from_sink(Some(8), Some(false)), Granted::Refused(_)));
        assert!(matches!(Granted::from_sink(Some(1), Some(true)), Granted::Refused(_)), "a mixer port is not an ADEC");
        assert!(matches!(Granted::from_sink(None, Some(false)), Granted::Refused(_)));
    }

    #[test]
    fn the_route_is_released_only_once_the_sink_is_closed() {
        let helper = Fake::default();
        let mut d = Driver::new(7);
        d.held = true;
        let mut open = 3;
        let mut slept = Duration::ZERO;
        d.finish(&helper, || { open -= 1; open > 0 }, |t| slept += t);
        assert_eq!(*helper.calls.lock().unwrap(), ["release 7"]);
        assert_eq!(slept, 2 * SINK_CLOSE_POLL, "released as soon as the last poll saw the sink closed");
        assert!(!d.held);

        let mut slept = Duration::ZERO;
        d.held = true;
        d.finish(&helper, || true, |t| slept += t);
        assert_eq!(slept, SINK_CLOSE_WAIT, "a sink that stays open is waited for, then released anyway");
        assert_eq!(helper.calls.lock().unwrap().len(), 2);

        let mut never = Driver::new(8);
        never.finish(&helper, || unreachable!("nothing was armed"), |_| unreachable!());
        assert_eq!(helper.calls.lock().unwrap().len(), 2, "a session that never armed releases nothing");
    }
}
