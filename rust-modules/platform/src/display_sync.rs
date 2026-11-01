//! Best-effort native display-engine sync measurement, not optical panel refresh.
//!
//! The worker is started at most once, only when the readout is visible. It reads the
//! `osd_irq` interrupt observed on the target; availability in an app jail is not guaranteed.
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_secs(1);
const MAX_AGE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snapshot {
    Measuring,
    Unavailable,
    /// Approximate whole Hz, measured over the actual elapsed monotonic interval.
    Hz(u64),
}

#[derive(Clone, Copy)]
struct Published {
    epoch: u64,
    at: Instant,
    value: Snapshot,
}

enum Command {
    Active(u64),
    Inactive,
}

/// A cheap, nonblocking consumer interface; a supported platform API can replace the worker.
#[derive(Default)]
pub struct Monitor {
    started: bool,
    active: bool,
    epoch: u64,
    tx: Option<mpsc::Sender<Command>>,
    published: Option<Arc<Mutex<Published>>>,
}

impl Monitor {
    pub fn set_active(&mut self, active: bool) {
        if active == self.active { return; }
        self.active = active;
        self.epoch = self.epoch.wrapping_add(1);
        if active && !self.started {
            self.started = true;
            let (tx, rx) = mpsc::channel();
            let published = Arc::new(Mutex::new(Published {
                epoch: self.epoch, at: Instant::now(), value: Snapshot::Measuring,
            }));
            let output = Arc::clone(&published);
            if plx_base::task::spawn("display-sync", move || worker(rx, output)).is_some() {
                self.tx = Some(tx);
                self.published = Some(published);
            }
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(if active { Command::Active(self.epoch) } else { Command::Inactive });
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        if !self.active { return Snapshot::Unavailable; }
        let Some(published) = &self.published else { return Snapshot::Unavailable; };
        let Ok(published) = published.try_lock() else { return Snapshot::Measuring; };
        if published.epoch != self.epoch { return Snapshot::Measuring; }
        if Instant::now().saturating_duration_since(published.at) > MAX_AGE {
            return Snapshot::Unavailable;
        }
        published.value
    }
}

#[cfg(any(test, all(target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
fn parse_count(text: &str) -> Option<u64> {
    let mut lines = text.lines();
    let cpus = lines.next()?.split_whitespace().filter(|s| {
        s.strip_prefix("CPU").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }).count();
    if cpus == 0 { return None; }
    let mut found = None;
    for line in lines {
        let mut fields = line.split_whitespace();
        let Some(id) = fields.next() else { continue; };
        if !id.ends_with(':') || line.split_whitespace().next_back() != Some("osd_irq") { continue; }
        if found.is_some() { return None; }
        let mut total = 0u64;
        for _ in 0..cpus {
            total = total.checked_add(fields.next()?.parse::<u64>().ok()?)?;
        }
        found = Some(total);
    }
    found
}

#[derive(Default)]
struct Rate { previous: Option<(Instant, u64)> }

impl Rate {
    fn sample(&mut self, now: Instant, count: Option<u64>) -> Snapshot {
        let Some(count) = count else {
            self.previous = None;
            return Snapshot::Unavailable;
        };
        let Some((at, before)) = self.previous.replace((now, count)) else {
            return Snapshot::Measuring;
        };
        let elapsed = now.saturating_duration_since(at);
        if elapsed.is_zero() || elapsed > MAX_AGE || count < before {
            return Snapshot::Measuring;
        }
        let delta = count - before;
        if delta == 0 { return Snapshot::Unavailable; }
        let hz = (delta as f64 / elapsed.as_secs_f64()).round();
        if hz < 1.0 || hz >= u64::MAX as f64 { Snapshot::Unavailable } else { Snapshot::Hz(hz as u64) }
    }
}

#[cfg(any(test, all(target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
fn read_interrupts(mut reader: impl std::io::Read, buffer: &mut [u8]) -> Option<u64> {
    let mut used = 0;
    loop {
        if used == buffer.len() { return None; }
        match reader.read(&mut buffer[used..]) {
            Ok(0) => return parse_count(std::str::from_utf8(&buffer[..used]).ok()?),
            Ok(n) => used += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

fn read_count(_buffer: &mut [u8]) -> Option<u64> {
    #[cfg(all(target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    { read_interrupts(std::fs::File::open("/proc/interrupts").ok()?, _buffer) }
    #[cfg(not(all(target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
    { None }
}

fn worker(rx: mpsc::Receiver<Command>, output: Arc<Mutex<Published>>) {
    let mut active = None;
    let mut rate = Rate::default();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let command = if active.is_some() {
            match rx.recv_timeout(PERIOD) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() { Ok(command) => Some(command), Err(_) => return }
        };
        match command {
            Some(Command::Active(epoch)) => {
                active = Some(epoch);
                rate = Rate::default();
            }
            Some(Command::Inactive) => { active = None; continue; }
            None => {}
        }
        if let Some(epoch) = active {
            let count = read_count(&mut buffer);
            let now = Instant::now();
            let value = rate.sample(now, count);
            if let Ok(mut published) = output.lock() {
                *published = Published { epoch, at: now, value };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_columns_and_rejects_bad_rows() {
        assert_eq!(parse_count(" CPU0 CPU1 CPU2\n 42: 10 20 30 GIC osd_irq\n"), Some(60));
        assert_eq!(parse_count(" CPU0 CPU1\n 42: 10 nope GIC osd_irq\n"), None);
        assert_eq!(parse_count(" CPU0 CPU1\n 42: 18446744073709551615 1 GIC osd_irq\n"), None);
        assert_eq!(parse_count(" CPU0\n 42: 10 GIC DE2VSYNC\n"), None);
        assert_eq!(parse_count(" CPU0\n 42: 10 GIC osd_irq\n 43: 20 GIC osd_irq\n"), None);
    }

    #[test]
    fn rate_uses_elapsed_and_invalidates_discontinuities() {
        let t = Instant::now();
        let mut rate = Rate::default();
        assert_eq!(rate.sample(t, Some(100)), Snapshot::Measuring);
        assert_eq!(rate.sample(t + Duration::from_millis(1500), Some(250)), Snapshot::Hz(100));
        assert_eq!(rate.sample(t + Duration::from_millis(2500), Some(369)), Snapshot::Hz(119));
        assert_eq!(rate.sample(t + Duration::from_millis(3500), Some(369)), Snapshot::Unavailable);
        assert_eq!(rate.sample(t + Duration::from_millis(4500), Some(2)), Snapshot::Measuring);
        assert_eq!(rate.sample(t + Duration::from_millis(8500), Some(402)), Snapshot::Measuring);
        assert_eq!(rate.sample(t + Duration::from_millis(9500), None), Snapshot::Unavailable);
        assert_eq!(rate.sample(t + Duration::from_millis(10500), Some(500)), Snapshot::Measuring);
        assert_eq!(rate.sample(t + Duration::from_millis(10500), Some(510)), Snapshot::Measuring);
    }

    #[test]
    fn bounded_reader_reuses_buffer_and_rejects_truncation() {
        let mut buffer = [0; 64];
        assert_eq!(read_interrupts(&b" CPU0 CPU1\n 42: 10 20 GIC osd_irq\n"[..], &mut buffer), Some(30));
        assert_eq!(read_interrupts(&b" CPU0\n 42: 2 GIC osd_irq\n"[..], &mut buffer), Some(2));
        assert_eq!(read_interrupts(&b" CPU0\n 42: 2 GIC osd_irq\n"[..], &mut buffer[..12]), None);
    }

    #[test]
    fn snapshots_expire_and_reactivation_hides_old_epoch() {
        let published = Arc::new(Mutex::new(Published {
            epoch: 1, at: Instant::now(), value: Snapshot::Hz(120),
        }));
        let mut monitor = Monitor {
            started: true, active: true, epoch: 1, tx: None, published: Some(Arc::clone(&published)),
        };
        assert_eq!(monitor.snapshot(), Snapshot::Hz(120));
        published.lock().unwrap().at = Instant::now() - MAX_AGE - PERIOD;
        assert_eq!(monitor.snapshot(), Snapshot::Unavailable);
        monitor.set_active(false);
        monitor.set_active(true);
        assert_eq!(monitor.snapshot(), Snapshot::Measuring);
    }
}
