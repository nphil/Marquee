//! **Multi Link as the demux thread uses it**: one [`Engine`], the transport that carries its
//! requests and the helper's state, behind the few calls a read callback makes.
//!
//! The machine decides; this file does what a machine may not. It reads the clock, takes the
//! helper's state from its watcher, writes the machine's lines to the event log, and waits when a
//! read has nothing to return.

use std::time::Instant;

use plx_base::eventlog::log;

use super::drive::{Driver, Transport};
use super::engine::{Demand, Engine, Event, Micros, Phase};
use super::status::{self, Status, Unusable};
use super::watch::{self, Watch};

/// A transport a read can wait on.
pub trait Wire: Transport {
    /// Wait until a connection has something to read, the transport is woken or `most_ms` pass.
    fn wait(&mut self, most_ms: u32);
}

/// How one [`Reader::read`] ended.
pub enum Read {
    Bytes(usize),
    /// The read position is the end of the object.
    End,
    /// The caller's stop held while there was nothing to return.
    Stopped,
    /// Ranges are over for now: the default reader carries on from this byte.
    HandBack(u64),
}

/// A state the helper has not replaced is judged again this often: it goes stale by the clock.
const JUDGE_EVERY: Micros = 50_000;
/// A read that has bytes to return moves the transport no more often than this. A demuxer reads
/// a few tens of kilobytes at a time, and a connection's socket holds far more than arrives in
/// two milliseconds.
const TURN_EVERY: Micros = 2_000;

pub struct Reader<W: Wire> {
    engine: Engine,
    driver: Driver,
    wire: W,
    watch: Watch,
    /// The helper's last answer, and which of the watcher's it is.
    state: Result<Status, Unusable>,
    revision: u64,
    judged_at: Micros,
    turned_at: Micros,
    /// What the last `helper state` line said, and whether links were offered under it.
    told: &'static str,
    offering: bool,
    epoch: Instant,
    demand: Demand,
    /// The longest one wait inside a read: how soon the caller's stop is seen when nothing wakes
    /// the transport.
    wait_most_ms: u32,
}

impl<W: Wire> Reader<W> {
    /// A reader for an object of `total` bytes that the default reader has open at its start.
    /// `None` when the state watcher could not start.
    pub fn new(wire: W, total: u64, wait_most_ms: u32) -> Option<Reader<W>> {
        Some(Reader {
            engine: Engine::new(total),
            driver: Driver::default(),
            wire,
            watch: Watch::start()?,
            state: Err(Unusable::Missing),
            revision: 0,
            judged_at: 0,
            turned_at: 0,
            told: "",
            offering: false,
            epoch: Instant::now(),
            demand: Demand::default(),
            wait_most_ms,
        })
    }

    fn now(&self) -> Micros {
        self.epoch.elapsed().as_micros() as Micros
    }

    /// The helper offers no link and nothing is out. Only its next answer changes that, so the
    /// default reader's reads cost one comparison until then.
    fn dormant(&self) -> bool {
        !self.offering && self.watch.revision() == self.revision && self.engine.in_flight() == 0
    }

    /// The reader follows the stream: the helper offers links, or has said something new.
    pub fn listening(&self) -> bool {
        !self.dormant()
    }

    fn put(&mut self, now: Micros, event: Event<'_>) {
        self.driver.step(&mut self.engine, &mut self.wire, now, event, &mut |line| log(line));
    }

    /// Tell the machine which links the helper offers at `now`.
    fn judge(&mut self, now: Micros) {
        let revision = self.watch.revision();
        if revision == 0 || (revision == self.revision && now < self.judged_at + JUDGE_EVERY) {
            return;
        }
        let changed = revision != self.revision;
        if changed {
            self.state = self.watch.latest();
            self.revision = revision;
        }
        self.judged_at = now;
        let usable = status::usable(&self.state, watch::uptime_ms());
        let name = match &usable {
            Ok(_) => "usable",
            Err(why) => why.name(),
        };
        if name != self.told {
            self.told = name;
            log(&format!("multilink: helper state {name}"));
        }
        if changed || usable.is_ok() != self.offering {
            self.offering = usable.is_ok();
            let (boot, offered) = match usable {
                Ok(status) => (status.boot, &status.candidates[..]),
                Err(_) => (0, &[][..]),
            };
            self.driver.step(&mut self.engine, &mut self.wire, now, Event::Links { boot, offered }, &mut |line| log(line));
        }
    }

    /// One turn at `now`: the helper's state as it stands, what arrived, and what is due.
    fn turn(&mut self, now: Micros) {
        self.turned_at = now;
        self.judge(now);
        self.driver.turn(&mut self.engine, &mut self.wire, now, &mut |line| log(line));
    }

    /// What the player needs of the reader now.
    pub fn demand(&mut self, demand: Demand) {
        if demand != self.demand {
            self.demand = demand;
            let now = self.now();
            self.put(now, Event::Demand(demand));
        }
    }

    /// The default reader handed the demuxer `bytes` at `at`.
    pub fn passed(&mut self, at: u64, bytes: &[u8]) {
        if self.dormant() {
            return;
        }
        let now = self.now();
        self.put(now, Event::Passed { at, bytes });
        self.turn(now);
    }

    /// Ranges have the stream, and the default reader is parked.
    pub fn carries(&self) -> bool {
        matches!(self.engine.phase(), Phase::Carrying | Phase::HandBack)
    }

    /// Take the stream at `at`, where the default reader stands with nothing read and not passed
    /// on. True when a proven link takes it: the caller parks the default reader.
    pub fn take_over(&mut self, at: u64) -> bool {
        if self.engine.phase() != Phase::Ready {
            return false;
        }
        let now = self.now();
        self.put(now, Event::Carry { at });
        self.carries()
    }

    /// Read at the read position, waiting as a blocking read does. `stopped` is asked whenever
    /// there is nothing to return.
    pub fn read(&mut self, dst: &mut [u8], stopped: &dyn Fn() -> bool) -> Read {
        loop {
            let now = self.now();
            let n = self.engine.take(dst);
            if n == 0 || now >= self.turned_at + TURN_EVERY {
                self.turn(now);
            }
            let n = if n > 0 { n } else { self.engine.take(dst) };
            if n > 0 {
                return Read::Bytes(n);
            }
            match self.engine.phase() {
                Phase::Carrying if self.engine.at_end() => return Read::End,
                Phase::Carrying => {}
                _ => return Read::HandBack(self.engine.position()),
            }
            if stopped() {
                return Read::Stopped;
            }
            let most = self.wait_most_ms as Micros * 1000;
            let wait = self.engine.next_wake(now).map_or(most, |at| (at - now).min(most));
            self.wire.wait((wait / 1000 + 1) as u32);
        }
    }

    /// The demuxer moved to `to`. True when ranges have the stream and follow it there; false
    /// when the move is the default reader's to make.
    pub fn seek(&mut self, to: u64) -> bool {
        if self.engine.phase() != Phase::Carrying {
            return false;
        }
        let now = self.now();
        self.put(now, Event::Seek { to });
        true
    }

    /// The default reader has the stream again, open at `at`.
    pub fn resumed(&mut self, at: u64) {
        let now = self.now();
        self.put(now, Event::Resumed { at });
    }

    /// The demuxer is parked on a full reserve: move what is in flight. True while requests are
    /// out, which is while this is worth calling again soon.
    pub fn idle(&mut self) -> bool {
        if self.dormant() {
            return false;
        }
        let now = self.now();
        self.turn(now);
        self.engine.in_flight() > 0
    }

    /// The bytes this reader holds or has asked for.
    pub fn held(&self) -> u64 {
        self.engine.held_bytes()
    }
}

impl<W: Wire> Drop for Reader<W> {
    fn drop(&mut self) {
        let now = self.now();
        self.put(now, Event::Stop);
    }
}
