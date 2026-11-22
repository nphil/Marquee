//! **The memory guard**: read the app's memory off the frame thread and, while the TV runs short,
//! give memory back and pause the work that only speculates.
//!
//! Every [`SAMPLE_EVERY_MS`] a small worker reads the app's resident size (`VmRSS` in
//! `/proc/self/status`) and the system's available memory (`MemAvailable` and `MemTotal` in
//! `/proc/meminfo`). The frame loop collects the answer on a later frame, never waits for it, and
//! starts no second read while one is out.
//!
//! A worse [`Level`] applies at once; a better one only after [`CALM_SAMPLES`] better samples in a
//! row, so memory hovering at a threshold does not flap. A change is logged and acted on, on the
//! frame thread: art that is not on screen is released down to a lower ceiling (`tex::squeeze`),
//! the art prefetch rests (`poster::rest_warms`), trailer autoplay holds
//! (`preview::rest_for_memory`) and the seek stills made on the TV are dropped and paused
//! (`seekprev::rest_for_memory`). Playback is left alone but for its read-ahead, which is sized
//! from every sample and goes while memory is short (`player::reserve`).

use std::sync::mpsc;

/// How often memory is read.
const SAMPLE_EVERY_MS: u32 = 2_000;
/// Better samples in a row a better level needs.
const CALM_SAMPLES: u8 = 3;

// The four thresholds are first values, not yet measured on the TV.
/// The app's resident size, as a share of the TV's memory, that counts as short.
const SHORT_RSS_PERCENT: u64 = 30;
/// Available memory at or under which memory counts as short.
const SHORT_AVAILABLE_KIB: u64 = 60 << 10;
/// The resident share that counts as critical.
const CRITICAL_RSS_PERCENT: u64 = 40;
/// Available memory at or under which memory counts as critical.
const CRITICAL_AVAILABLE_KIB: u64 = 35 << 10;

/// How short of memory the TV is, mildest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Fine,
    Short,
    Critical,
}

/// One read, in KiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    rss_kib: u64,
    available_kib: u64,
    total_kib: u64,
}

impl Sample {
    fn level(self) -> Level {
        let rss_percent = self.rss_kib * 100 / self.total_kib.max(1);
        if rss_percent >= CRITICAL_RSS_PERCENT || self.available_kib <= CRITICAL_AVAILABLE_KIB {
            Level::Critical
        } else if rss_percent >= SHORT_RSS_PERCENT || self.available_kib <= SHORT_AVAILABLE_KIB {
            Level::Short
        } else {
            Level::Fine
        }
    }
}

/// The level in force, and the run of better samples that may lower it.
struct Pressure {
    level: Level,
    /// Better samples in a row, and the worst of them: the level a completed run lowers to.
    calm: Option<(u8, Level)>,
}

impl Pressure {
    /// The new level when `seen` changes it.
    fn observe(&mut self, seen: Level) -> Option<Level> {
        if seen >= self.level {
            self.calm = None;
            let worse = seen > self.level;
            self.level = seen;
            return worse.then_some(seen);
        }
        let (count, worst) = self.calm.map_or((1, seen), |(count, worst)| (count + 1, worst.max(seen)));
        if count < CALM_SAMPLES {
            self.calm = Some((count, worst));
            return None;
        }
        self.calm = None;
        self.level = worst;
        Some(worst)
    }
}

/// A read on its way: `None` for a read that found no numbers.
type Read = mpsc::Receiver<Option<Sample>>;

/// What the frame loop owns between polls.
pub(crate) struct MemoryGuard {
    pressure: Pressure,
    /// When the next read starts (the app clock, ms); `None` until the first.
    due: Option<u32>,
    pending: Option<Read>,
}

impl MemoryGuard {
    pub(crate) fn new() -> Self {
        Self { pressure: Pressure { level: Level::Fine, calm: None }, due: None, pending: None }
    }

    /// One frame's look, on the frame thread: a channel poll, and a worker start every
    /// [`SAMPLE_EVERY_MS`].
    pub(crate) fn poll(&mut self, now: u32) {
        if let Some((level, sample)) = self.poll_with(now, read_off_frame) {
            apply(level, sample);
        }
    }

    /// The decision, with the worker start injected so a test can answer it.
    fn poll_with(&mut self, now: u32, start: impl FnOnce() -> Option<Read>) -> Option<(Level, Sample)> {
        let mut changed = None;
        if let Some(read) = &self.pending {
            match read.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return None,
                Ok(sample) => {
                    self.pending = None;
                    changed = sample.and_then(|s| self.pressure.observe(s.level()).map(|level| (level, s)));
                    if let Some(s) = sample {
                        // Every sample, not only a change of level: the read-ahead is sized from it.
                        plx_media::player::reserve::note_memory(s.available_kib << 10, self.pressure.level != Level::Fine);
                    }
                }
                // The worker ended without an answer; the next read starts on the clock.
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.due.is_none_or(|due| now.wrapping_sub(due) < 1 << 31) {
            self.due = Some(now.wrapping_add(SAMPLE_EVERY_MS));
            self.pending = start();
        }
        changed
    }
}

/// Start a read on a small worker; `None` when the thread was refused.
fn read_off_frame() -> Option<Read> {
    let (tx, rx) = mpsc::channel();
    plx_base::task::spawn_small("memory guard", move || {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let _ = tx.send(sample(&status, &meminfo));
    })
    .then_some(rx)
}

fn sample(status: &str, meminfo: &str) -> Option<Sample> {
    use super::adapters::poster::proc_kib;
    Some(Sample {
        rss_kib: proc_kib(status, "VmRSS:")?,
        available_kib: proc_kib(meminfo, "MemAvailable:")?,
        total_kib: proc_kib(meminfo, "MemTotal:")?,
    })
}

/// Act on a new level. GL thread: the squeeze frees textures.
fn apply(level: Level, s: Sample) {
    plx_base::eventlog::log(&format!(
        "memory: {level:?} (rss {} MiB, available {} of {} MiB)",
        s.rss_kib >> 10,
        s.available_kib >> 10,
        s.total_kib >> 10
    ));
    let ordinary = plx_ui::tex::TEX_RESIDENT_BYTES_MAX * plx_ui::tex::render_area();
    let ceiling = match level {
        Level::Fine => None,
        Level::Short => Some(ordinary / 2),
        Level::Critical => Some(ordinary / 4),
    };
    plx_ui::tex::squeeze(ceiling, &mut super::adapters::poster::GfxUploader);
    super::adapters::poster::rest_warms(level != Level::Fine);
    plx_media::player::preview::rest_for_memory(level != Level::Fine);
    plx_media::seekprev::rest_for_memory(level != Level::Fine);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TV's memory in these tests, KiB: a round number keeps the shares exact.
    const TOTAL: u64 = 1_000_000;

    fn at(rss_kib: u64, available_kib: u64) -> Sample {
        Sample { rss_kib, available_kib, total_kib: TOTAL }
    }

    #[test]
    fn a_sample_is_short_or_critical_by_either_measure() {
        assert_eq!(at(TOTAL / 10, TOTAL / 2).level(), Level::Fine);
        assert_eq!(at(TOTAL * 30 / 100, TOTAL / 2).level(), Level::Short, "the app's own size");
        assert_eq!(at(TOTAL / 10, SHORT_AVAILABLE_KIB).level(), Level::Short, "the TV's free memory");
        assert_eq!(at(TOTAL * 40 / 100, TOTAL / 2).level(), Level::Critical);
        assert_eq!(at(TOTAL / 10, CRITICAL_AVAILABLE_KIB).level(), Level::Critical);
        assert_eq!(sample("VmRSS:\t 1024 kB\n", "MemTotal: 4096 kB\nMemAvailable: 2048 kB\n"), Some(Sample {
            rss_kib: 1024,
            available_kib: 2048,
            total_kib: 4096
        }));
        assert_eq!(sample("", "MemTotal: 4096 kB\nMemAvailable: 2048 kB\n"), None);
    }

    #[test]
    fn a_worse_level_applies_at_once_and_a_better_one_after_a_calm_run() {
        let mut p = Pressure { level: Level::Fine, calm: None };
        assert_eq!(p.observe(Level::Short), Some(Level::Short));
        assert_eq!(p.observe(Level::Critical), Some(Level::Critical));
        assert_eq!(p.observe(Level::Fine), None);
        assert_eq!(p.observe(Level::Short), None);
        assert_eq!(p.observe(Level::Fine), Some(Level::Short), "a run lowers to its worst sample");
        assert_eq!(p.observe(Level::Fine), None);
        assert_eq!(p.observe(Level::Short), None, "a sample at the level breaks the run");
        assert_eq!(p.observe(Level::Fine), None);
        assert_eq!(p.observe(Level::Fine), None);
        assert_eq!(p.observe(Level::Fine), Some(Level::Fine));
    }

    #[derive(Default)]
    struct Reads {
        started: u32,
        answer: Option<mpsc::Sender<Option<Sample>>>,
        refuse: bool,
    }

    impl Reads {
        fn start(&mut self) -> impl FnOnce() -> Option<Read> + '_ {
            move || {
                self.started += 1;
                if self.refuse {
                    return None;
                }
                let (tx, rx) = mpsc::channel();
                self.answer = Some(tx);
                Some(rx)
            }
        }
    }

    #[test]
    fn one_read_at_a_time_on_the_sample_clock() {
        let short = at(TOTAL / 10, SHORT_AVAILABLE_KIB);
        let mut reads = Reads::default();
        let mut guard = MemoryGuard::new();
        assert_eq!(guard.poll_with(5, reads.start()), None);
        assert_eq!(reads.started, 1, "the first frame reads");
        assert_eq!(guard.poll_with(5 + SAMPLE_EVERY_MS, reads.start()), None);
        assert_eq!(reads.started, 1, "no second read while one is out");
        reads.answer.take().unwrap().send(Some(short)).unwrap();
        assert_eq!(guard.poll_with(5 + SAMPLE_EVERY_MS, reads.start()), Some((Level::Short, short)));
        assert_eq!(reads.started, 2, "a late read's successor starts once it is in");

        drop(reads.answer.take());
        assert_eq!(guard.poll_with(6 + SAMPLE_EVERY_MS, reads.start()), None);
        assert_eq!(reads.started, 2, "a read that ended without an answer waits for its time");
        reads.refuse = true;
        assert_eq!(guard.poll_with(5 + 2 * SAMPLE_EVERY_MS, reads.start()), None);
        assert_eq!(guard.poll_with(6 + 2 * SAMPLE_EVERY_MS, reads.start()), None);
        assert_eq!(reads.started, 3, "a refused worker is retried on the clock, not every frame");
    }
}
