//! Where the time between a seek (or the start) and the first video keyframe in the lane goes,
//! told in one line from the demux thread. A `#[path]` child of `ff.rs`.
//!
//! The stages are the ones that can each hold a rebuffer: the container seek (which reopens the
//! transport), the first packet back from it, the first video keyframe, and the push of that
//! keyframe into the lane (which parks while the lane is full). The thread's own scheduler
//! counters say whether the time was spent running, or waiting for a CPU on a four-core set
//! shared with the decoder, the renderer and the radios. One `Instant` read per packet until the
//! line is out, none after.

use std::time::{Duration, Instant};

/// What `/proc/thread-self/schedstat` says: nanoseconds on a CPU and nanoseconds runnable but
/// not running.
#[derive(Clone, Copy)]
struct Sched {
    run_ns: u64,
    wait_ns: u64,
}

fn sched() -> Option<Sched> {
    let text = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let mut fields = text.split_whitespace().map(|f| f.parse::<u64>().ok());
    Some(Sched { run_ns: fields.next()??, wait_ns: fields.next()?? })
}

pub(crate) struct SeekTrace {
    what: &'static str,
    began: Instant,
    sched: Option<Sched>,
    /// Milliseconds from `began` when the container seek returned.
    seek_ms: Option<u64>,
    /// The transport's offset when the reading from here began.
    from: i64,
    packet_ms: Option<u64>,
    key_ms: Option<u64>,
}

impl SeekTrace {
    pub(crate) fn begin(what: &'static str, offset: i64) -> SeekTrace {
        SeekTrace { what, began: Instant::now(), sched: sched(), seek_ms: None, from: offset, packet_ms: None, key_ms: None }
    }

    fn ms(&self) -> u64 {
        self.began.elapsed().as_millis() as u64
    }

    /// The container seek returned, with the transport at `offset`.
    pub(crate) fn sought(&mut self, offset: i64) {
        (self.seek_ms, self.from) = (Some(self.ms()), offset);
    }

    /// A packet came back from `av_read_frame`: only the first is kept.
    pub(crate) fn packet(&mut self) {
        if self.packet_ms.is_none() {
            self.packet_ms = Some(self.ms());
        }
    }

    /// The first video keyframe is about to be pushed.
    pub(crate) fn key(&mut self) {
        if self.key_ms.is_none() {
            self.key_ms = Some(self.ms());
        }
    }

    /// The keyframe is in the lane after `held` inside the push, the transport at `offset`.
    pub(crate) fn finish(self, held: Duration, offset: i64) {
        let total = self.ms();
        let (seek_ms, from) = (self.seek_ms.unwrap_or(0), self.from);
        let cpu = match (self.sched, sched()) {
            (Some(a), Some(b)) => {
                format!("ran {} ms, waited {} ms for a CPU", b.run_ns.saturating_sub(a.run_ns) / 1_000_000, b.wait_ns.saturating_sub(a.wait_ns) / 1_000_000)
            }
            _ => "scheduler counters unavailable".to_owned(),
        };
        crate::player::log(&format!(
            "ff: {} trace: container seek {seek_ms} ms, first packet {} ms, first key {} ms, key in the lane {total} ms (push held {} ms); \
             demux thread {cpu}; {} bytes read",
            self.what,
            self.packet_ms.unwrap_or(0),
            self.key_ms.unwrap_or(0),
            held.as_millis(),
            offset.saturating_sub(from).max(0),
        ));
    }
}
