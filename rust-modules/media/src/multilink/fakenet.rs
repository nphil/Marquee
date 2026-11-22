//! A made-up network for the tests: a clock the test moves, one object, and for each source
//! address a path with a rate, a delay and a way of answering. [`Sim`] plays the demuxer, the
//! default reader and the player's reserve around a real [`Engine`] and [`Driver`].

use std::net::Ipv4Addr;

use super::drive::{Arrival, Driver, Transport};
use super::engine::{Demand, Engine, Event, Fault, Micros, Phase, ReqId, Request};
use super::object::Head;
use super::status::{Candidate, Kind};

pub const MS: Micros = 1_000;
pub const ETAG: &str = "\"part-41-1756720800\"";
pub const WIRE: Ipv4Addr = Ipv4Addr::new(192, 168, 5, 27);
pub const WIFI: Ipv4Addr = Ipv4Addr::new(192, 168, 5, 31);
pub const USB: Ipv4Addr = Ipv4Addr::new(192, 168, 5, 44);

/// Megabits a second as bytes a second.
pub const fn mbit(n: u64) -> u64 {
    n * 1_000_000 / 8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Good,
    /// `200` and the file from its head.
    IgnoresRange,
    /// The body ends after this many bytes.
    Truncates(u64),
    LiesAboutTotal,
    LiesAboutOffset,
    NewValidator,
    Redirects,
    TlsFails,
    /// The right head over the wrong bytes.
    OtherBytes,
    Silent,
    WeakEtagOnly,
    Chunked,
    Encoded,
    Overruns,
}

pub struct Path {
    pub source: Ipv4Addr,
    /// Bytes a second from each moment on, in time order.
    pub rates: Vec<(Micros, u64)>,
    pub delay: Micros,
    /// What a connection made anew adds to its first answer. A request finds one open only where
    /// an earlier request ended whole: a cancelled request takes its connection with it.
    pub connect: Micros,
    pub answer: Answer,
    /// This many requests are answered well before `answer` applies.
    pub good_first: u32,
    seen: u32,
    /// Connections that requests which ended whole left open.
    open: u32,
}

impl Path {
    pub fn new(source: Ipv4Addr, rate: u64) -> Path {
        Path { source, rates: vec![(0, rate)], delay: 2 * MS, connect: 0, answer: Answer::Good, good_first: 0, seen: 0, open: 0 }
    }

    pub fn answering(mut self, answer: Answer, good_first: u32) -> Path {
        (self.answer, self.good_first) = (answer, good_first);
        self
    }

    fn rate_at(&self, at: Micros) -> u64 {
        self.rates.iter().rev().find(|(from, _)| *from <= at).map_or(0, |(_, rate)| *rate)
    }
}

struct Live {
    request: Request,
    path: usize,
    answer: Answer,
    /// When its answer begins.
    answers_at: Micros,
    /// Up to when its bytes are accounted.
    at: Micros,
    sent: u64,
    headed: bool,
    /// Byte-microseconds not yet a whole byte.
    credit: u64,
}

pub struct FakeNet {
    pub now: Micros,
    pub object: Vec<u8>,
    pub paths: Vec<Path>,
    live: Vec<Live>,
    pub started: Vec<(Micros, Request, Option<String>)>,
    pub cancelled: Vec<(Micros, ReqId)>,
    /// Body bytes delivered per source, with when.
    pub delivered: Vec<(Micros, Ipv4Addr, u64)>,
    /// Requests answered in full, in the order they ended.
    pub finished: Vec<(Micros, Request)>,
    /// How many body bytes each request that ended had delivered.
    ended: Vec<(Request, u64)>,
}

impl FakeNet {
    pub fn new(object: Vec<u8>, paths: Vec<Path>) -> FakeNet {
        FakeNet {
            now: 0,
            object,
            paths,
            live: Vec::new(),
            started: Vec::new(),
            cancelled: Vec::new(),
            delivered: Vec::new(),
            finished: Vec::new(),
            ended: Vec::new(),
        }
    }

    pub fn path(&mut self, source: Ipv4Addr) -> &mut Path {
        self.paths.iter_mut().find(|p| p.source == source).expect("a path for that source")
    }

    /// The requests out now.
    pub fn live(&self) -> Vec<Request> {
        self.live.iter().map(|l| l.request.clone()).collect()
    }

    /// Body bytes `source` delivered in `from..to`.
    pub fn bytes(&self, source: Ipv4Addr, from: Micros, to: Micros) -> u64 {
        self.delivered.iter().filter(|(at, s, _)| *s == source && *at >= from && *at < to).map(|(_, _, n)| n).sum()
    }

    /// How many body bytes every request delivered, those still out too.
    pub fn spans(&self) -> Vec<(Request, u64)> {
        self.ended.iter().cloned().chain(self.live.iter().map(|l| (l.request.clone(), l.sent))).collect()
    }

    /// Requests sent from `source`.
    pub fn requests(&self, source: Ipv4Addr) -> Vec<&Request> {
        self.started.iter().filter(|(_, r, _)| r.source == source).map(|(_, r, _)| r).collect()
    }

    fn head(&self, live: &Live) -> Head {
        let (first, last, total) = (live.request.first, live.request.last, self.object.len() as u64);
        let mut head = Head {
            status: 206,
            range: Some((first, last, Some(total))),
            length: Some(last - first + 1),
            etag: Some(ETAG.to_owned()),
            modified: Some("Tue, 01 Sep 2026 10:00:00 GMT".to_owned()),
            encoded: false,
            chunked: false,
        };
        match live.answer {
            Answer::IgnoresRange => {
                head.status = 200;
                head.range = None;
                head.length = Some(total);
            }
            Answer::LiesAboutTotal => head.range = Some((first, last, Some(total + 1))),
            Answer::LiesAboutOffset => head.range = Some((first + 1, last + 1, Some(total))),
            Answer::NewValidator => head.etag = Some("\"part-41-1756724400\"".to_owned()),
            Answer::Redirects => head = Head { status: 302, ..Head::default() },
            Answer::WeakEtagOnly => {
                head.etag = Some("W/\"weak\"".to_owned());
                head.modified = None;
            }
            Answer::Chunked => {
                head.chunked = true;
                head.length = None;
            }
            Answer::Encoded => head.encoded = true,
            _ => {}
        }
        head
    }
}

impl Transport for FakeNet {
    fn start(&mut self, request: &Request, if_range: Option<&str>) -> bool {
        self.started.push((self.now, request.clone(), if_range.map(str::to_owned)));
        let Some(path) = self.paths.iter().position(|p| p.source == request.source) else { return false };
        let answer = if self.paths[path].seen < self.paths[path].good_first { Answer::Good } else { self.paths[path].answer };
        self.paths[path].seen += 1;
        let made = if self.paths[path].open > 0 {
            self.paths[path].open -= 1;
            0
        } else {
            self.paths[path].connect
        };
        let answers_at = self.now + self.paths[path].delay + made;
        self.live.push(Live { request: request.clone(), path, answer, answers_at, at: self.now, sent: 0, headed: false, credit: 0 });
        true
    }

    fn cancel(&mut self, id: ReqId) {
        self.cancelled.push((self.now, id));
        if let Some(at) = self.live.iter().position(|l| l.request.id == id) {
            let live = self.live.remove(at);
            self.ended.push((live.request, live.sent));
        }
    }

    fn poll(&mut self, arrived: &mut Vec<Arrival>) {
        let now = self.now;
        let total = self.object.len() as u64;
        let mut at = 0;
        while at < self.live.len() {
            let sharing = self.live.iter().filter(|l| l.path == self.live[at].path && l.headed).count().max(1) as u64;
            let head = (!self.live[at].headed).then(|| self.head(&self.live[at]));
            let live = &mut self.live[at];
            let path = &self.paths[live.path];
            let id = live.request.id;
            if let Some(head) = head {
                if live.answer == Answer::Silent || now < live.answers_at {
                    at += 1;
                    continue;
                }
                if live.answer == Answer::TlsFails {
                    arrived.push(Arrival::Failed(id, Fault::Tls));
                    self.live.remove(at);
                    continue;
                }
                live.headed = true;
                live.at = live.answers_at;
                arrived.push(Arrival::Head(id, head));
                if live.answer == Answer::Redirects {
                    arrived.push(Arrival::Done(id));
                    self.live.remove(at);
                    continue;
                }
            }
            let asked = live.request.last - live.request.first + 1;
            let (from, len) = match live.answer {
                Answer::IgnoresRange => (0, total),
                Answer::Truncates(n) => (live.request.first, n.min(asked)),
                Answer::LiesAboutOffset => (live.request.first + 1, asked.min(total - live.request.first - 1)),
                Answer::Overruns => (live.request.first, (asked + 1000).min(total - live.request.first)),
                _ => (live.request.first, asked),
            };
            // The rate may change inside the step: account it a millisecond at a time.
            while live.at < now {
                let step = (now - live.at).min(MS);
                live.credit += path.rate_at(live.at) * step / sharing;
                live.at += step;
            }
            let n = (live.credit / 1_000_000).min(len - live.sent);
            live.credit -= n * 1_000_000;
            if n > 0 {
                let start = (from + live.sent) as usize;
                let mut bytes = self.object[start..start + n as usize].to_vec();
                if live.answer == Answer::OtherBytes {
                    bytes[0] = !bytes[0];
                }
                live.sent += n;
                self.delivered.push((now, live.request.source, n));
                arrived.push(Arrival::Body(id, bytes));
            }
            if live.sent == len {
                arrived.push(Arrival::Done(id));
                let live = self.live.remove(at);
                if len == asked {
                    self.paths[live.path].open += 1;
                }
                self.ended.push((live.request.clone(), live.sent));
                self.finished.push((now, live.request));
            } else {
                at += 1;
            }
        }
    }
}

/// The helper's candidate for a made-up link.
pub fn candidate(interface: &str, kind: Kind, address: Ipv4Addr) -> Candidate {
    Candidate { interface: interface.to_owned(), kind, address, prefix: 24, carrier_changes: 1, control: kind == Kind::Ethernet }
}

/// A file's worth of bytes no two stretches of which read alike: noise, repeated, with every
/// block of it numbered.
pub fn object(len: usize) -> Vec<u8> {
    const BLOCK: usize = 4096;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut bytes = Vec::with_capacity(len.max(BLOCK) + BLOCK);
    while bytes.len() < BLOCK {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    while bytes.len() < len {
        bytes.extend_from_within(..bytes.len().min(len - bytes.len()));
    }
    bytes.resize(len, 0);
    for (number, block) in bytes.chunks_exact_mut(BLOCK).enumerate() {
        block[..8].copy_from_slice(&(number as u64).to_le_bytes());
    }
    bytes
}

/// A 64-bit sum over whole words, every one of which changes it. Built for tests, SHA-256 takes
/// seconds over a film's megabytes and an iterator costs a call a word: this is an index loop.
fn digest(bytes: &[u8]) -> u64 {
    const PRIME: u128 = 0x0000_0100_0000_01B3;
    let (pairs, rest) = bytes.as_chunks::<16>();
    let mut sum = 0xCBF2_9CE4_8422_2325u64;
    let mut at = 0;
    while at < pairs.len() {
        let pair = u128::from_le_bytes(pairs[at]);
        sum = ((sum ^ pair as u64) as u128 * PRIME) as u64;
        sum = ((sum ^ (pair >> 64) as u64) as u128 * PRIME) as u64;
        at += 1;
    }
    for byte in rest {
        sum = ((sum ^ u64::from(*byte)) as u128 * PRIME) as u64;
    }
    sum
}

/// The player around the reader: a film of `bps` playing out of a reserve the demuxer fills.
pub struct Film {
    pub bps: u64,
    /// The demuxer stops reading at this much queued media.
    pub full_ms: u64,
    /// Media queued, in microseconds.
    pub queued: Micros,
    /// When the playhead first found the reserve empty.
    pub starved_at: Option<Micros>,
}

impl Film {
    pub fn new(mbps: u64, queued_ms: u64) -> Film {
        Film { bps: mbps * 1_000_000, full_ms: 60_000, queued: queued_ms * MS, starved_at: None }
    }
}

pub struct Sim {
    pub engine: Engine,
    pub net: FakeNet,
    pub driver: Driver,
    pub logs: Vec<String>,
    pub now: Micros,
    /// What the helper offers: nothing when its state is missing, stale or off.
    pub offered: Vec<Candidate>,
    pub boot: u64,
    pub room: u64,
    /// The default reader's rate, bytes a second.
    pub default_rate: u64,
    pub film: Option<Film>,
    /// Where the demuxer reads next.
    pub pos: u64,
    /// What the demuxer read, as `(offset, bytes)` runs: a seek starts a new one.
    pub read: Vec<(u64, Vec<u8>)>,
    pub carried: u64,
    pub passed: u64,
    pub handbacks: u32,
    pub peak_held: u64,
    /// The longest the demuxer waited with nothing to read, in milliseconds.
    pub longest_wait_ms: u64,
    wait_ms: u64,
    buf: Vec<u8>,
}

impl Sim {
    pub fn new(len: usize, paths: Vec<Path>, offered: Vec<Candidate>) -> Sim {
        let mut sim = Sim {
            engine: Engine::new(len as u64),
            net: FakeNet::new(object(len), paths),
            driver: Driver::default(),
            logs: Vec::new(),
            now: 0,
            offered,
            boot: 7,
            room: 8 << 20,
            default_rate: mbit(8),
            film: None,
            pos: 0,
            read: vec![(0, Vec::new())],
            carried: 0,
            passed: 0,
            handbacks: 0,
            peak_held: 0,
            longest_wait_ms: 0,
            wait_ms: 0,
            buf: vec![0; 256 << 10],
        };
        sim.put(Event::Resumed { at: 0 });
        sim
    }

    pub fn total(&self) -> u64 {
        self.net.object.len() as u64
    }

    pub fn put(&mut self, event: Event<'_>) {
        let (now, logs) = (self.now, &mut self.logs);
        self.driver.step(&mut self.engine, &mut self.net, now, event, &mut |line| logs.push(format!("{} {line}", now / MS)));
    }

    /// The demuxer seeks.
    pub fn seek(&mut self, to: u64) {
        self.pos = to;
        self.read.push((to, Vec::new()));
        self.put(Event::Seek { to });
    }

    /// The demux thread is elsewhere for `ms`: nothing is read, the transport is not moved, and
    /// the film plays on.
    pub fn away(&mut self, ms: u64) {
        self.now += ms * MS;
        self.net.now = self.now;
        if let Some(film) = &mut self.film {
            film.queued = film.queued.saturating_sub(ms * MS);
        }
    }

    /// One millisecond of everything.
    pub fn tick(&mut self) {
        self.now += MS;
        self.net.now = self.now;
        let now = self.now;
        let offered = std::mem::take(&mut self.offered);
        self.put(Event::Links { boot: self.boot, offered: &offered });
        self.offered = offered;
        let (reserve_ms, full, film_bps) = match &self.film {
            Some(film) => ((film.queued / MS) as u32, film.queued >= film.full_ms * MS, film.bps),
            None => (0, false, 0),
        };
        self.put(Event::Demand(Demand { reserve_ms, full, film_bps, room: self.room }));
        let logs = &mut self.logs;
        self.driver.turn(&mut self.engine, &mut self.net, now, &mut |line| logs.push(format!("{} {line}", now / MS)));
        // The demuxer reads unless the queues are full; a default reader is as fast as its wire.
        let left = (self.total() - self.pos.min(self.total())) as usize;
        let mut got = 0;
        match self.engine.phase() {
            _ if full => {}
            Phase::Ready => self.put(Event::Carry { at: self.pos }),
            Phase::Standby => {
                got = left.min((self.default_rate / 1_000) as usize);
                let at = self.pos as usize;
                let bytes = self.net.object[at..at + got].to_vec();
                self.put(Event::Passed { at: self.pos, bytes: &bytes });
                self.read.last_mut().expect("a run").1.extend_from_slice(&bytes);
                self.passed += got as u64;
            }
            Phase::Carrying => {
                got = self.engine.take(&mut self.buf);
                self.read.last_mut().expect("a run").1.extend_from_slice(&self.buf[..got]);
                self.carried += got as u64;
            }
            Phase::HandBack => {
                assert_eq!(self.engine.position(), self.pos, "the hand-back is where the demuxer is");
                self.handbacks += 1;
                self.put(Event::Resumed { at: self.pos });
            }
        }
        self.pos += got as u64;
        if got == 0 && !full && left > 0 {
            self.wait_ms += 1;
            self.longest_wait_ms = self.longest_wait_ms.max(self.wait_ms);
        } else {
            self.wait_ms = 0;
        }
        if let Some(film) = &mut self.film {
            film.queued += got as u64 * 8 * 1_000_000 / film.bps;
            if film.queued >= MS {
                film.queued -= MS;
            } else if left > 0 {
                film.starved_at.get_or_insert(now);
            }
        }
        self.peak_held = self.peak_held.max(self.engine.held_bytes());
        assert!(self.engine.held_bytes() <= self.room, "the reader holds no more than its room");
    }

    pub fn run_ms(&mut self, ms: u64) {
        for _ in 0..ms {
            self.tick();
        }
    }

    /// Run until `done` says so; false when `limit_ms` passed first.
    pub fn run_until(&mut self, limit_ms: u64, done: impl Fn(&Sim) -> bool) -> bool {
        for _ in 0..limit_ms {
            if done(self) {
                return true;
            }
            self.tick();
        }
        done(self)
    }

    /// Run to the end of the file.
    pub fn finish(&mut self, limit_ms: u64) {
        let done = self.run_until(limit_ms, |s| s.pos == s.total());
        assert!(done, "the file is read to its end; read {} of {}\n{}", self.pos, self.total(), self.logs.join("\n"));
    }

    pub fn logged(&self, what: &str) -> bool {
        self.logs.iter().any(|l| l.contains(what))
    }

    /// When `what` was first logged, in milliseconds.
    pub fn logged_at(&self, what: &str) -> Option<u64> {
        let line = self.logs.iter().find(|l| l.contains(what))?;
        line.split(' ').next()?.parse().ok()
    }

    /// Every run the demuxer read is the object's bytes at that offset, by hash.
    pub fn assert_exact(&self) {
        for (at, bytes) in &self.read {
            let at = *at as usize;
            assert_eq!(digest(bytes), digest(&self.net.object[at..at + bytes.len()]), "the bytes read at {at} are the file's\n{}", self.logs.join("\n"));
        }
    }
}
