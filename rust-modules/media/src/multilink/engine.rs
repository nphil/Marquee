//! **The decisions of Multi Link**: which link has proven itself, which link fetches the next
//! range, when a range moves to another link, and when the default reader takes the stream back.
//!
//! A machine and nothing else: it reads no clock, opens no socket and writes no log. Time arrives
//! with every [`Engine::step`], answers arrive as [`Event`]s, and what it wants done leaves as
//! [`Fx`]s for [`super::drive`] to carry out. The bytes it has joined leave by
//! [`Engine::take`], which copies and decides nothing.
//!
//! * **Proof.** A link carries nothing until a small range fetched over it equals, byte for byte,
//!   what the trusted path delivered for the same range (the default reader, or links already
//!   proven), under an answer that names the same object ([`Identity`]). The wire is always
//!   proven, as the place a failed range goes; of the others one at a time, and a further one only
//!   when what is proven does not cover the film.
//! * **Ranges.** The file ahead of the demuxer is cut into ranges of about [`RANGE_SPAN_MS`] of
//!   the fetching link's measured rate. Each has one slot in the window, in file order, so what
//!   arrives out of order is read out in order.
//! * **Which link.** The fastest proven link carries alone while it covers the film. The others
//!   join when it does not or the reserve is low, and a range goes to the idle link that will
//!   finish it first, or to a slower one only when the demuxer will not be waiting for it.
//! * **The wire's share.** While another link carries too, the built-in wire is given ranges at
//!   no more than [`WIRE_SHARE`], so the TV's controls keep the rest of it.
//! * **Doubt.** An answer that is not the object ends ranges for the file. A link that stalls
//!   loses its range to another and must be proven again. No usable helper state, or no proven
//!   link, hands the stream back to the default reader.
//! * **What counts against a link.** Only what its connections bring while the transport is
//!   moved. A deadline or a rate is held against the clock on a [`Event::Tick`], a range against
//!   its plan from its head on, and a stretch the driver left the transport alone against
//!   nothing. A link nothing is asked of is not failing.

use std::collections::VecDeque;
use std::net::Ipv4Addr;

use super::object::{Head, Identity};
use super::status::{Candidate, Kind};

/// Microseconds on the driver's monotonic clock.
pub type Micros = u64;
pub type ReqId = u32;

const MS: Micros = 1_000;
const SECOND: Micros = 1_000_000;

/// A proof is this many bytes of the file.
const PROBE_BYTES: u64 = 64 << 10;
/// How far ahead of the default reader a proof's range starts, so its bytes are still to come.
const PROBE_AHEAD: u64 = 256 << 10;
/// No proof starts until the read position has been left alone this long: a demuxer opening a
/// file jumps about, and every jump abandons a proof.
const CALM: Micros = SECOND;
/// A range is about this long at its link's measured rate, within the two sizes below.
const RANGE_SPAN_MS: u64 = 150;
const RANGE_FLOOR: u64 = 128 << 10;
const RANGE_CEIL: u64 = 1 << 20;
/// Ranges need this much room to be worth carrying: two of the largest.
const ROOM_FLOOR: u64 = 2 * RANGE_CEIL;
/// A request with no answer this long after it was sent has failed.
const FIRST_BYTE: Micros = SECOND;
/// An answer that brings nothing for this long has stalled.
const STALL: Micros = 500 * MS;
/// What a failed link was fetching is asked of another no later than this after its last byte,
/// beside that link's own range if every other link is busy.
const REPLACE_BY: Micros = SECOND;
/// A failed link is proven again no sooner than this, doubling to [`RETRY_LONGEST`].
const RETRY_FIRST: Micros = 2 * SECOND;
const RETRY_LONGEST: Micros = 30 * SECOND;
/// A link proven again after a failure carries nothing until it has stayed offered this long.
const WATCH: Micros = 3 * SECOND;
/// A proven link that has not carried yet is given this many ranges to show its rate.
const TRIAL_RANGES: u32 = 8;
/// The built-in wire's share while another link carries too: 70 of its 100 Mbit/s, the rest left
/// to the TV's own traffic and its remote controls (measured on the TV: wired controls answer in
/// 8 ms at the 95th percentile at this rate).
const WIRE_SHARE: u64 = 70_000_000 / 8;
/// Another link counts as carrying for this long after its last range.
const SHARE_HOLD: Micros = 500 * MS;
/// Below this much queued media every proven link carries; they stop at [`RESTORED_MS`] or when
/// the queues are full.
const REFILL_MS: u32 = 8_000;
const RESTORED_MS: u32 = 16_000;
/// One link carries alone while its rate is this many percent of the film's, and while the
/// reserve is being restored, while it fills it three times as fast as it plays.
const COVERS_PERCENT: u64 = 115;
const REFILLS_PERCENT: u64 = 400;
/// A range in flight is judged against its link's rate every this long, and a range without a
/// link is forced onto a busy one this long before [`REPLACE_BY`].
const JUDGED_AFTER: Micros = 100 * MS;
/// A driver waiting on its transport steps the machine at least every [`JUDGED_AFTER`] while a
/// request is out. A longer gap is the reader's own time, away from the transport.
const UNWATCHED: Micros = 2 * JUDGED_AFTER;
/// A fill shorter than this gets no line: a seek that another follows.
const FILL_TOLD: Micros = SECOND;

/// One range to fetch over a connection bound to `source`. It names no URL: a transport is built
/// for one object, so a request cannot be sent anywhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: ReqId,
    /// The read position's generation: a seek or a hand-back starts the next, and nothing asked
    /// before it is used after it.
    pub generation: u32,
    pub source: Ipv4Addr,
    pub first: u64,
    /// Inclusive, as `Range` spells it.
    pub last: u64,
    /// Keep this connection's receive buffer small: it is the built-in wire, whose queue at the
    /// switch the TV's controls wait in.
    pub bounded: bool,
}

/// Why a transport gave a request up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// No connection from that source address.
    Connect,
    Tls,
    /// The answer ended before its length.
    Cut,
    /// More body than the range asked for.
    Overrun,
}

impl Fault {
    fn name(self) -> &'static str {
        match self {
            Fault::Connect => "no connection",
            Fault::Tls => "TLS",
            Fault::Cut => "cut short",
            Fault::Overrun => "more bytes than asked",
        }
    }
}

/// What the player needs of the reader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Demand {
    /// Media queued ahead of the playhead.
    pub reserve_ms: u32,
    /// The queues hold all they may: the demuxer reads at the film's own rate.
    pub full: bool,
    /// The film's mean rate in bits a second, 0 when not known.
    pub film_bps: u64,
    /// The bytes the read-ahead reserve leaves this reader for ranges, fetched or in flight.
    pub room: u64,
}

pub enum Event<'a> {
    /// The helper's sources as judged now: none when its state is missing, stale or off.
    Links { boot: u64, offered: &'a [Candidate] },
    Demand(Demand),
    /// The default reader handed the demuxer `bytes` at `at`.
    Passed { at: u64, bytes: &'a [u8] },
    /// The default reader is parked: ranges carry the stream from `at`.
    Carry { at: u64 },
    /// The demuxer moved to `to`.
    Seek { to: u64 },
    /// The default reader has the stream, at `at`.
    Resumed { at: u64 },
    Head { id: ReqId, head: Head },
    Body { id: ReqId, bytes: Vec<u8> },
    Done { id: ReqId },
    Failed { id: ReqId, fault: Fault },
    /// Time passed, and everything the transport had received is in: the one event at which a
    /// deadline or a rate is held against the clock.
    Tick,
    /// The reader is closing: nothing is asked again.
    Stop,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Fx {
    Start(Request),
    Cancel(ReqId),
    Log(String),
}

/// Where the stream comes from, as the driver needs to know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The default reader feeds the demuxer.
    Standby,
    /// As `Standby`, and a link is proven: the driver may say `Carry` and park the default reader.
    Ready,
    /// [`Engine::take`] feeds the demuxer.
    Carrying,
    /// Nothing more comes from ranges: the default reader resumes at [`Engine::position`].
    HandBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Standby,
    Carrying,
    /// Retired: what is in hand is still read out.
    Draining,
}

struct Probe {
    id: ReqId,
    first: u64,
    len: usize,
    since: Micros,
    progress_at: Micros,
    /// What the answer says the object is, once its head is in.
    identity: Option<Identity>,
    got: Vec<u8>,
    /// The same range as the trusted path delivered it.
    trusted: Vec<u8>,
    done: bool,
}

impl Probe {
    fn trust(&mut self, at: u64, bytes: &[u8]) {
        let next = self.first + self.trusted.len() as u64;
        let end = self.first + self.len as u64;
        if next < end && at <= next && next < at + bytes.len() as u64 {
            let from = (next - at) as usize;
            let to = bytes.len().min((end - at) as usize);
            self.trusted.extend_from_slice(&bytes[from..to]);
        }
    }

    fn deadline(&self) -> Micros {
        if self.identity.is_some() {
            self.progress_at + STALL
        } else {
            self.since + FIRST_BYTE
        }
    }
}

enum Proof {
    /// Not proven: a proof may start once `from` is reached.
    Waiting { from: Micros },
    Probing(Box<Probe>),
    /// Proven again after a failure, and carrying nothing until `until`.
    Watched { until: Micros },
    Proven,
    /// It answered with other bytes or another object: not for this file.
    Rejected,
}

struct Link {
    key: u16,
    who: Candidate,
    proof: Proof,
    /// Useful body bytes a second: whole ranges over the time each took, request included. 0
    /// until its first range.
    rate: u64,
    /// Ranges measured since it was last proven.
    samples: u32,
    retry: Micros,
    /// It failed since it was last trusted: its next proof is watched before it carries.
    lapsed: bool,
    /// It counts as carrying until then.
    carried_until: Micros,
    /// When its next range may start, while its share is limited.
    next_at: Micros,
    /// A range was taken from it while it brought nothing, and that request is still out to be
    /// heard: the link has failed unless it brings a byte by then.
    doubted: Option<(ReqId, Micros)>,
    /// Body bytes it brought during the fill under way.
    filled: u64,
}

impl Link {
    fn proven(&self) -> bool {
        matches!(self.proof, Proof::Proven)
    }

    /// The built-in wire, the one link whose share is ever limited.
    fn wire(&self) -> bool {
        self.who.control && self.who.kind == Kind::Ethernet
    }
}

struct Fetch {
    id: ReqId,
    link: u16,
    since: Micros,
    progress_at: Micros,
    /// How much of the slot was in hand when this was asked: it fetches the rest.
    from: u64,
    /// When its head came.
    headed: Option<Micros>,
    /// How long the range was to take at its link's rate when it was asked.
    planned: Micros,
    /// When its link's rate was last held against what this range shows, and how much of the
    /// slot was in hand then.
    judged_at: Micros,
    judged_got: u64,
    /// The transport was moved all the while it was out: its time is a measure of its link.
    watched: bool,
}

impl Fetch {
    fn deadline(&self) -> Micros {
        if self.headed.is_some() {
            self.progress_at + STALL
        } else {
            self.since + FIRST_BYTE
        }
    }
}

/// One range of the window, in file order.
struct Slot {
    first: u64,
    len: u64,
    /// Bytes received and not yet read out, in order.
    chunks: VecDeque<Vec<u8>>,
    /// How much of the front chunk has been read out.
    skip: usize,
    got: u64,
    fetch: Option<Fetch>,
    /// When its bytes last arrived, or it was made: what a range without a link is replaced by.
    progress_at: Micros,
}

/// The reserve being restored, from when that began.
struct Fill {
    since: Micros,
    /// The least media queued during it: a seek empties the queues a moment after it begins.
    low_ms: u32,
    /// Body bytes of ranges received during it.
    bytes: u64,
}

pub struct Engine {
    total: u64,
    mode: Mode,
    /// The next byte the demuxer reads.
    head: u64,
    /// The end of the last slot.
    tail: u64,
    slots: VecDeque<Slot>,
    /// In the helper's order, which is the fastest expected first.
    links: Vec<Link>,
    boot: Option<u64>,
    identity: Option<Identity>,
    /// No ranges for this file again: an answer was not the object, or the reader closed.
    spoiled: bool,
    calm_from: Micros,
    demand: Demand,
    /// The links carrying now, the lead first.
    using: Vec<u16>,
    /// Every proven link is carrying until the reserve is restored.
    refilling: bool,
    /// The lead alone carries the film.
    covered: bool,
    /// The wire's share is limited: another link is carrying too.
    sharing: bool,
    generation: u32,
    next_id: ReqId,
    next_key: u16,
    failovers: u32,
    /// The links, the sharing, the fail-overs and how many links had shown their rate when the
    /// last `using` line was written.
    told: (Vec<u16>, bool, u32, usize),
    /// The fill under way, while the reserve is being restored.
    fill: Option<Fill>,
    /// When the machine was last stepped.
    stepped_at: Micros,
}

impl Engine {
    /// A machine for an object of `total` bytes, in standby.
    pub fn new(total: u64) -> Engine {
        Engine {
            total,
            mode: Mode::Standby,
            head: 0,
            tail: 0,
            slots: VecDeque::new(),
            links: Vec::new(),
            boot: None,
            identity: None,
            spoiled: false,
            calm_from: 0,
            demand: Demand::default(),
            using: Vec::new(),
            refilling: false,
            covered: true,
            sharing: false,
            generation: 0,
            next_id: 1,
            next_key: 1,
            failovers: 0,
            told: (Vec::new(), false, 0, 0),
            fill: None,
            stepped_at: 0,
        }
    }

    pub fn step(&mut self, now: Micros, event: Event<'_>, fx: &mut Vec<Fx>) {
        let gap = now.saturating_sub(self.stepped_at);
        if gap > UNWATCHED {
            self.unwatched(gap);
        }
        self.stepped_at = now;
        // Before a `Tick` a connection's bytes may be waiting unread: the clock says nothing of
        // a link yet.
        let moved = matches!(event, Event::Tick);
        match event {
            Event::Links { boot, offered } => self.on_links(now, boot, offered, fx),
            Event::Demand(demand) => self.demand = demand,
            Event::Passed { at, bytes } => self.on_passed(now, at, bytes, fx),
            Event::Carry { at } => self.on_carry(at, fx),
            Event::Seek { to } => self.on_seek(now, to, fx),
            Event::Resumed { at } => {
                self.mode = Mode::Standby;
                self.slots.clear();
                self.jump(now, at, fx);
            }
            Event::Head { id, head } => self.on_head(now, id, &head, fx),
            Event::Body { id, bytes } => self.on_body(now, id, bytes, fx),
            Event::Done { id } => self.on_done(now, id, fx),
            Event::Failed { id, fault } => self.on_failed(now, id, fault, fx),
            Event::Tick => {}
            Event::Stop => {
                self.filled(now, fx);
                self.spoiled = true;
                self.cancel_fetches(fx);
                self.abandon_proofs(now, fx);
                self.slots.clear();
                self.tail = self.head;
                self.mode = Mode::Standby;
            }
        }
        self.settle(now, moved, fx);
    }

    /// Copy what is in hand at the read position into `dst` and move past it.
    pub fn take(&mut self, dst: &mut [u8]) -> usize {
        if self.mode == Mode::Standby {
            return 0;
        }
        let mut done = 0;
        while done < dst.len() {
            let Some(slot) = self.slots.front_mut() else { break };
            let Some(chunk) = slot.chunks.front() else {
                if slot.first + slot.len == self.head {
                    self.slots.pop_front();
                    continue;
                }
                break;
            };
            let n = (chunk.len() - slot.skip).min(dst.len() - done);
            dst[done..done + n].copy_from_slice(&chunk[slot.skip..slot.skip + n]);
            slot.skip += n;
            if slot.skip == chunk.len() {
                slot.chunks.pop_front();
                slot.skip = 0;
            }
            for link in &mut self.links {
                if let Proof::Probing(probe) = &mut link.proof {
                    probe.trust(self.head, &dst[done..done + n]);
                }
            }
            done += n;
            self.head += n as u64;
        }
        done
    }

    pub fn phase(&self) -> Phase {
        match self.mode {
            Mode::Standby if self.takes_over() => Phase::Ready,
            Mode::Standby => Phase::Standby,
            Mode::Carrying => Phase::Carrying,
            Mode::Draining if self.head < self.tail => Phase::Carrying,
            Mode::Draining => Phase::HandBack,
        }
    }

    /// The next byte the demuxer reads.
    pub fn position(&self) -> u64 {
        self.head
    }

    /// The read position is the end of the object.
    pub fn at_end(&self) -> bool {
        self.head >= self.total
    }

    /// Bytes [`take`](Self::take) would return now.
    pub fn readable(&self) -> usize {
        let mut at = self.head;
        for slot in &self.slots {
            at = slot.first + slot.got;
            if slot.got < slot.len {
                break;
            }
        }
        (at - self.head) as usize
    }

    /// Requests out now.
    pub fn in_flight(&self) -> usize {
        self.slots.iter().filter(|s| s.fetch.is_some()).count()
            + self.links.iter().filter(|l| l.doubted.is_some() || matches!(&l.proof, Proof::Probing(p) if !p.done)).count()
    }

    /// What this reader holds or has asked for: the window from the read position, and proofs.
    pub fn held_bytes(&self) -> u64 {
        let proofs: usize = self
            .links
            .iter()
            .map(|l| match &l.proof {
                Proof::Probing(p) => 2 * p.len,
                _ => 0,
            })
            .sum();
        self.tail - self.head + proofs as u64
    }

    /// What a request sends as `If-Range`.
    pub fn if_range(&self) -> Option<&str> {
        self.identity.as_ref().and_then(Identity::if_range)
    }

    /// When, after `now`, the machine has something to do without an event: a deadline, a proof
    /// due, the wire's share coming free.
    pub fn next_wake(&self, now: Micros) -> Option<Micros> {
        let mut wake: Option<Micros> = None;
        let mut at = |t: Micros| {
            if t > now {
                wake = Some(wake.map_or(t, |w| w.min(t)));
            }
        };
        for slot in &self.slots {
            match &slot.fetch {
                Some(fetch) => {
                    at(fetch.deadline());
                    if fetch.headed.is_some() {
                        at(fetch.judged_at + JUDGED_AFTER);
                    }
                }
                None if slot.got < slot.len && self.mode == Mode::Carrying => at(slot.progress_at + REPLACE_BY - JUDGED_AFTER),
                None => {}
            }
        }
        let proves = !self.spoiled && self.mode != Mode::Draining;
        for link in &self.links {
            match &link.proof {
                Proof::Waiting { from } if proves => at((*from).max(self.calm_from)),
                Proof::Probing(probe) if !probe.done => at(probe.deadline()),
                Proof::Watched { until } => at(*until),
                _ => {}
            }
            if self.mode == Mode::Carrying && self.limited(link) {
                at(link.next_at);
            }
            if let Some((_, by)) = link.doubted {
                at(by);
            }
        }
        wake
    }

    // ---- events ----------------------------------------------------------------------------

    fn on_links(&mut self, now: Micros, boot: u64, offered: &[Candidate], fx: &mut Vec<Fx>) {
        if offered.is_empty() {
            self.forget(|_| true, fx);
            self.boot = None;
            if self.mode == Mode::Carrying {
                self.retire(now, "the helper offers no links", true, fx);
            }
            return;
        }
        if self.boot != Some(boot) {
            // Another helper: nothing proven under the last one holds.
            self.forget(|_| true, fx);
            self.boot = Some(boot);
        }
        self.forget(|link| !offered.iter().any(|c| c.same_link(&link.who)), fx);
        for candidate in offered {
            match self.links.iter_mut().find(|l| l.who.same_link(candidate)) {
                Some(link) if link.who != *candidate => link.who = candidate.clone(),
                Some(_) => {}
                None => {
                    self.links.push(Link {
                        key: self.next_key,
                        who: candidate.clone(),
                        proof: Proof::Waiting { from: now },
                        rate: 0,
                        samples: 0,
                        retry: RETRY_FIRST,
                        lapsed: false,
                        carried_until: 0,
                        next_at: 0,
                        doubted: None,
                        filled: 0,
                    });
                    self.next_key += 1;
                }
            }
        }
        self.links.sort_by_key(|l| offered.iter().position(|c| c.same_link(&l.who)));
    }

    fn on_passed(&mut self, now: Micros, at: u64, bytes: &[u8], fx: &mut Vec<Fx>) {
        if self.mode != Mode::Standby {
            return;
        }
        if at != self.head {
            self.jump(now, at, fx);
        }
        for link in &mut self.links {
            if let Proof::Probing(probe) = &mut link.proof {
                probe.trust(at, bytes);
            }
        }
        self.head = at + bytes.len() as u64;
        self.tail = self.head;
    }

    fn on_carry(&mut self, at: u64, fx: &mut Vec<Fx>) {
        if self.mode != Mode::Standby || at != self.head || !self.takes_over() {
            return;
        }
        self.mode = Mode::Carrying;
        self.generation += 1;
        fx.push(Fx::Log(format!("multilink: carrying from byte {at}")));
    }

    fn on_seek(&mut self, now: Micros, to: u64, fx: &mut Vec<Fx>) {
        if to == self.head {
            return;
        }
        if self.mode == Mode::Carrying && to > self.head && to - self.head <= self.readable() as u64 {
            // Inside what is in hand: read past it, and keep everything asked for.
            self.abandon_proofs(now, fx);
            self.calm_from = now + CALM;
            let mut sink = [0u8; 4096];
            while self.head < to {
                let n = ((to - self.head) as usize).min(sink.len());
                if self.take(&mut sink[..n]) == 0 {
                    break;
                }
            }
            return;
        }
        // What is fetched from here on fills other queues: the demuxer flushes on a seek.
        self.filled(now, fx);
        self.cancel_fetches(fx);
        self.slots.clear();
        self.generation += 1;
        self.jump(now, to, fx);
    }

    fn on_head(&mut self, now: Micros, id: ReqId, head: &Head, fx: &mut Vec<Fx>) {
        if let Some(at) = self.slot_of(id) {
            let slot = &mut self.slots[at];
            let Some(fetch) = slot.fetch.as_mut() else { return };
            let (first, last) = (slot.first + fetch.from, slot.first + slot.len - 1);
            match self.identity.as_ref().map(|identity| identity.accepts(head, first, last)) {
                Some(Ok(())) => {
                    fetch.headed = Some(now);
                    fetch.progress_at = now;
                    (fetch.judged_at, fetch.judged_got) = (now, slot.got);
                }
                Some(Err(why)) => {
                    let link = fetch.link;
                    self.spoil(now, &format!("{why} on {}", self.name(link)), fx);
                }
                None => {}
            }
        } else if let Some(at) = self.probing(id) {
            let Proof::Probing(probe) = &mut self.links[at].proof else { return };
            match Identity::of(head, probe.first, probe.first + probe.len as u64 - 1, self.total) {
                Ok(identity) if self.identity.as_ref().is_none_or(|known| *known == identity) => {
                    probe.identity = Some(identity);
                    probe.progress_at = now;
                }
                Ok(_) => self.reject(at, "another validator", fx),
                Err(why) => self.reject(at, &why.to_string(), fx),
            }
        }
    }

    fn on_body(&mut self, now: Micros, id: ReqId, bytes: Vec<u8>, fx: &mut Vec<Fx>) {
        if let Some(at) = self.slot_of(id) {
            let slot = &mut self.slots[at];
            let Some(fetch) = slot.fetch.as_mut() else { return };
            let (key, n) = (fetch.link, bytes.len() as u64);
            if fetch.headed.is_none() || slot.got + n > slot.len {
                self.spoil(now, &format!("{} on {}", Fault::Overrun.name(), self.name(key)), fx);
            } else if n > 0 {
                slot.got += n;
                slot.chunks.push_back(bytes);
                slot.progress_at = now;
                fetch.progress_at = now;
                if let Some(fill) = &mut self.fill {
                    fill.bytes += n;
                    if let Some(link) = self.links.iter_mut().find(|l| l.key == key) {
                        link.filled += n;
                    }
                }
            }
        } else if let Some(at) = self.probing(id) {
            let Proof::Probing(probe) = &mut self.links[at].proof else { return };
            if probe.identity.is_none() || probe.got.len() + bytes.len() > probe.len {
                self.reject(at, Fault::Overrun.name(), fx);
            } else {
                probe.got.extend_from_slice(&bytes);
                probe.progress_at = now;
            }
        } else if let Some(at) = self.doubting(id) {
            // The request a range was taken from brings bytes again: its link stands. The bytes
            // themselves are another link's to fetch by now.
            if !bytes.is_empty() {
                self.links[at].doubted = None;
                fx.push(Fx::Cancel(id));
            }
        }
    }

    fn on_done(&mut self, now: Micros, id: ReqId, fx: &mut Vec<Fx>) {
        if let Some(at) = self.slot_of(id) {
            let slot = &mut self.slots[at];
            let Some(fetch) = slot.fetch.take() else { return };
            if fetch.headed.is_some() && slot.got == slot.len {
                let sample = fetch.watched.then(|| (slot.len - fetch.from, now - fetch.since));
                self.measured(now, fetch.link, sample);
            } else {
                self.lapse(now, fetch.link, Fault::Cut.name(), fx);
            }
        } else if let Some(at) = self.probing(id) {
            let Proof::Probing(probe) = &mut self.links[at].proof else { return };
            if probe.identity.is_some() && probe.got.len() == probe.len {
                probe.done = true;
            } else {
                self.unproven(now, at, Fault::Cut.name(), fx);
            }
        }
    }

    fn on_failed(&mut self, now: Micros, id: ReqId, fault: Fault, fx: &mut Vec<Fx>) {
        if let Some(at) = self.slot_of(id) {
            let Some(fetch) = self.slots[at].fetch.take() else { return };
            if fault == Fault::Overrun {
                self.spoil(now, &format!("{} on {}", fault.name(), self.name(fetch.link)), fx);
            } else {
                self.lapse(now, fetch.link, fault.name(), fx);
            }
        } else if let Some(at) = self.probing(id) {
            if fault == Fault::Overrun {
                self.reject(at, fault.name(), fx);
            } else {
                self.unproven(now, at, fault.name(), fx);
            }
        } else if let Some(at) = self.doubting(id) {
            let key = self.links[at].key;
            self.lapse(now, key, fault.name(), fx);
        }
    }

    // ---- what follows every event ------------------------------------------------------------

    fn settle(&mut self, now: Micros, moved: bool, fx: &mut Vec<Fx>) {
        if moved {
            // A fetch past its deadline takes its link with it, and with the link every fetch it had.
            while let Some((link, headed)) = self.slots.iter().filter_map(|s| s.fetch.as_ref()).find(|f| now >= f.deadline()).map(|f| (f.link, f.headed.is_some())) {
                self.lapse(now, link, if headed { "stalled" } else { "no answer" }, fx);
            }
            while let Some(key) = self.links.iter().find(|l| l.doubted.is_some_and(|(_, by)| now >= by)).map(|l| l.key) {
                self.lapse(now, key, "stalled", fx);
            }
        }
        for at in 0..self.links.len() {
            let verdict = match &self.links[at].proof {
                Proof::Probing(p) if moved && !p.done && now >= p.deadline() => Some(Err(p.id)),
                Proof::Probing(p) if p.done && p.trusted.len() == p.len => Some(Ok(p.got == p.trusted)),
                _ => None,
            };
            match verdict {
                Some(Err(id)) => {
                    fx.push(Fx::Cancel(id));
                    self.unproven(now, at, "no answer", fx);
                }
                Some(Ok(true)) => self.prove(now, at, fx),
                Some(Ok(false)) => self.reject(at, "other bytes", fx),
                None => {}
            }
            let link = &mut self.links[at];
            if matches!(link.proof, Proof::Watched { until } if now >= until) {
                link.proof = Proof::Proven;
                link.lapsed = false;
                fx.push(Fx::Log(format!("multilink: {} may carry again", link.who.interface)));
            }
        }
        if self.spoiled || self.mode == Mode::Draining {
            return;
        }
        if self.demand.room < ROOM_FLOOR {
            // Memory is short: nothing is held for ranges, in hand or asked for.
            if self.mode == Mode::Carrying {
                self.retire(now, "the reserve has no room for ranges", false, fx);
            } else {
                self.abandon_proofs(now, fx);
            }
            return;
        }
        self.prove_more(now, fx);
        if self.mode != Mode::Carrying {
            return;
        }
        if !self.links.iter().any(Link::proven) {
            self.retire(now, "no proven link", true, fx);
        } else {
            if moved {
                self.judge(now);
            }
            self.choose(now, fx);
            if moved {
                self.relieve(now, fx);
            }
            self.dispatch(now, fx);
            self.tell(fx);
        }
    }

    /// Start the proofs that are due: the wire's always, and of the others the most promising
    /// one, when none is in hand or what is proven does not cover the film.
    fn prove_more(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        if now < self.calm_from {
            return;
        }
        // A further link is worth proving once the lead has shown, past its first small ranges,
        // that it does not cover the film.
        let short = self.mode == Mode::Carrying
            && !self.covered
            && self.using.first().is_some_and(|&lead| self.links.iter().any(|l| l.key == lead && l.samples >= TRIAL_RANGES));
        let mut proving = self.links.iter().any(|l| !l.who.control && matches!(l.proof, Proof::Probing(_)));
        let in_hand = proving || self.links.iter().any(|l| !l.who.control && matches!(l.proof, Proof::Proven | Proof::Watched { .. }));
        for at in 0..self.links.len() {
            let link = &self.links[at];
            if !matches!(link.proof, Proof::Waiting { from } if now >= from) {
                continue;
            }
            if link.who.control {
                self.probe(now, at, fx);
            } else if !proving && (short || !in_hand) {
                self.probe(now, at, fx);
                proving = true;
            }
        }
    }

    /// Start a proof of link `at`: a small range the trusted path is about to deliver.
    fn probe(&mut self, now: Micros, at: usize, fx: &mut Vec<Fx>) {
        let first = if self.mode == Mode::Standby { self.head + PROBE_AHEAD } else { self.tail };
        let link = &mut self.links[at];
        if first >= self.total {
            // Too near the end for a range still to come.
            link.proof = Proof::Waiting { from: now + RETRY_LONGEST };
            return;
        }
        let len = PROBE_BYTES.min(self.total - first) as usize;
        let id = self.next_id;
        self.next_id += 1;
        link.proof = Proof::Probing(Box::new(Probe {
            id,
            first,
            len,
            since: now,
            progress_at: now,
            identity: None,
            got: Vec::with_capacity(len),
            trusted: Vec::with_capacity(len),
            done: false,
        }));
        fx.push(Fx::Start(Request {
            id,
            generation: self.generation,
            source: link.who.address,
            first,
            last: first + len as u64 - 1,
            bounded: link.wire(),
        }));
    }

    fn prove(&mut self, now: Micros, at: usize, fx: &mut Vec<Fx>) {
        let Proof::Probing(probe) = &self.links[at].proof else { return };
        let identity = probe.identity.clone();
        // Another link's proof may have named the object since this answer began.
        if self.identity.is_some() && self.identity != identity {
            return self.reject(at, "another validator", fx);
        }
        self.identity = identity;
        let link = &mut self.links[at];
        link.proof = if link.lapsed { Proof::Watched { until: now + WATCH } } else { Proof::Proven };
        // Its rate is measured afresh: what it carried before a failure says nothing of it now.
        (link.samples, link.rate) = (0, 0);
        fx.push(Fx::Log(format!("multilink: {} proven", link.who.interface)));
    }

    fn reject(&mut self, at: usize, why: &str, fx: &mut Vec<Fx>) {
        let link = &mut self.links[at];
        if let Proof::Probing(probe) = &link.proof {
            fx.push(Fx::Cancel(probe.id));
        }
        link.proof = Proof::Rejected;
        fx.push(Fx::Log(format!("multilink: {} rejected: {why}", link.who.interface)));
    }

    /// A proof of link `at` did not finish: it is tried again later, each time later.
    fn unproven(&mut self, now: Micros, at: usize, why: &str, fx: &mut Vec<Fx>) {
        let link = &mut self.links[at];
        fx.push(Fx::Log(format!("multilink: {} not proven ({why}), again in {} s", link.who.interface, link.retry / SECOND)));
        link.proof = Proof::Waiting { from: now + link.retry };
        link.retry = (link.retry * 2).min(RETRY_LONGEST);
    }

    /// Link `key` carries nothing more until it is proven again: what it was fetching is free
    /// for another.
    fn demote(&mut self, now: Micros, key: u16, fx: &mut Vec<Fx>) -> Option<&mut Link> {
        for slot in &mut self.slots {
            if let Some(fetch) = slot.fetch.take_if(|f| f.link == key) {
                fx.push(Fx::Cancel(fetch.id));
            }
        }
        let link = self.links.iter_mut().find(|l| l.key == key)?;
        if let Some((id, _)) = link.doubted.take() {
            fx.push(Fx::Cancel(id));
        }
        link.proof = Proof::Waiting { from: now + link.retry };
        link.retry = (link.retry * 2).min(RETRY_LONGEST);
        link.lapsed = true;
        Some(link)
    }

    /// A link failed while it carried.
    fn lapse(&mut self, now: Micros, key: u16, why: &str, fx: &mut Vec<Fx>) {
        let failovers = self.failovers + 1;
        if let Some(link) = self.demote(now, key, fx) {
            let line = format!("multilink: {} failed ({why}), failovers {failovers}", link.who.interface);
            fx.push(Fx::Log(line));
            self.failovers = failovers;
        }
    }

    /// One range fetched whole over link `key`, and what it measured: its bytes and how long it
    /// took, unless the transport stood unmoved for some of that. The rate rises fast and falls
    /// slowly, so one slow range does not halve the next.
    fn measured(&mut self, now: Micros, key: u16, sample: Option<(u64, Micros)>) {
        let Some(link) = self.links.iter_mut().find(|l| l.key == key) else { return };
        link.carried_until = now + SHARE_HOLD;
        let Some((bytes, took)) = sample else { return };
        let rate = bytes * SECOND / took.max(MS);
        link.rate = if link.rate == 0 {
            rate
        } else if rate > link.rate {
            (link.rate + rate) / 2
        } else {
            (link.rate * 3 + rate) / 4
        };
        link.samples += 1;
        if link.samples == 2 * TRIAL_RANGES {
            // It outlasted its trial: the next failure is a first one again.
            link.retry = RETRY_FIRST;
        }
    }

    /// Which proven links carry now, and whether the wire's share is limited.
    fn choose(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        let demand = self.demand;
        if !demand.full && demand.reserve_ms < REFILL_MS {
            self.refilling = true;
        } else if demand.full || demand.reserve_ms >= RESTORED_MS {
            self.refilling = false;
        }
        if !self.refilling {
            self.filled(now, fx);
        } else if let Some(fill) = &mut self.fill {
            fill.low_ms = fill.low_ms.min(demand.reserve_ms);
        } else {
            self.fill = Some(Fill { since: now, low_ms: demand.reserve_ms, bytes: 0 });
            for link in &mut self.links {
                link.filled = 0;
            }
        }
        let mut fastest: Option<usize> = None;
        for (at, link) in self.links.iter().enumerate() {
            if link.proven() && fastest.is_none_or(|best| link.rate > self.links[best].rate) {
                fastest = Some(at);
            }
        }
        let Some(fastest) = fastest else { return };
        // The lead changes only for a link a quarter faster: two alike would trade places.
        let lead = self
            .using
            .first()
            .and_then(|&key| self.links.iter().position(|l| l.key == key && l.proven()))
            .filter(|&at| self.links[at].rate * 5 >= self.links[fastest].rate * 4)
            .unwrap_or(fastest);
        // A film whose rate is not known is covered by one link except while the reserve fills.
        let percent = if self.refilling { REFILLS_PERCENT } else { COVERS_PERCENT };
        self.covered = match demand.film_bps {
            0 => !self.refilling,
            film => self.links[lead].rate * 8 * 100 >= film * percent,
        };
        let tried = |link: &Link| link.samples >= TRIAL_RANGES;
        // The wire alone at its whole rate carries more than the others beside its share: they
        // are not worth what limiting it takes.
        let alone = !self.covered
            && self.links.iter().find(|l| l.proven() && l.wire()).is_some_and(|wire| {
                let mut others = self.links.iter().filter(|l| l.proven() && !l.wire()).peekable();
                others.peek().is_some() && others.clone().all(tried) && wire.rate > others.map(|l| l.rate).sum::<u64>() + wire.rate.min(WIRE_SHARE)
            });
        let wire_leads = self.links[lead].who.control;
        self.using.clear();
        self.using.push(self.links[lead].key);
        for (at, link) in self.links.iter().enumerate() {
            // Beside a wire that covers the film, a link that has not carried yet shows its
            // rate, and takes the lead if it is the faster.
            let joins = if alone { link.wire() } else { !self.covered || (wire_leads && !tried(link)) };
            if at != lead && link.proven() && joins {
                self.using.push(link.key);
            }
        }
        if alone {
            self.using.retain(|&key| self.links.iter().any(|l| l.key == key && l.wire()));
            for at in 0..self.links.len() {
                let link = &self.links[at];
                if !link.proven() || link.wire() {
                    continue;
                }
                let (key, mbps) = (link.key, link.rate * 8 / 1_000_000);
                if let Some(link) = self.demote(now, key, fx) {
                    let line = format!("multilink: {} set aside at {mbps} Mbit/s", link.who.interface);
                    fx.push(Fx::Log(line));
                }
            }
        }
        self.sharing = self.links.iter().any(|l| l.wire() && self.using.contains(&l.key))
            && self.links.iter().any(|l| !l.wire() && l.doubted.is_none() && self.using.contains(&l.key) && (now < l.carried_until || !self.idle(l.key)));
    }

    /// Lower a link's rate to what its range in flight shows, by no more than half a time: a
    /// link that slows is planned for at once, and a pause is not taken for a collapse. A stretch
    /// that brought nothing shows no rate: it is a stall, which has its own deadline.
    fn judge(&mut self, now: Micros) {
        for slot in &mut self.slots {
            let Some(fetch) = slot.fetch.as_mut() else { continue };
            if fetch.headed.is_none() || now < fetch.judged_at + JUDGED_AFTER {
                continue;
            }
            let seen = (slot.got - fetch.judged_got) * SECOND / (now - fetch.judged_at);
            (fetch.judged_at, fetch.judged_got) = (now, slot.got);
            if let Some(link) = self.links.iter_mut().find(|l| l.key == fetch.link) {
                if seen > 0 && seen < link.rate {
                    link.rate = seen.max(link.rate / 2);
                }
            }
        }
    }

    /// Take a range from a link that will not finish it before the demuxer needs it, when it is
    /// taking over twice as long as planned from its head on and another link would finish it in
    /// half the time. The wait for an answer is not counted: a connection made anew owes that to
    /// its handshake, not to its link. A link still bringing bytes is planned at what the range
    /// showed. One that has gone quiet keeps the request, to be heard by: it has failed unless a
    /// byte comes within [`STALL`] of its last.
    fn relieve(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        for at in 0..self.slots.len() {
            let slot = &self.slots[at];
            let Some(fetch) = &slot.fetch else { continue };
            let Some(headed) = fetch.headed.filter(|&since| now >= since + JUDGED_AFTER) else { continue };
            let (id, key, left, heard) = (fetch.id, fetch.link, slot.len - slot.got, fetch.progress_at);
            let seen = (slot.got - fetch.from) * SECOND / (now - headed);
            let finishes = now + left * SECOND / seen.max(1);
            if finishes <= headed + 2 * fetch.planned + JUDGED_AFTER || finishes <= self.needed_at(now, slot.first + slot.got) {
                continue;
            }
            let sooner = self.using.iter().any(|&other| other != key && (self.finish(now, other, left) - now) * 2 < finishes - now);
            if !sooner {
                continue;
            }
            self.slots[at].fetch = None;
            let mut heard_out = false;
            if let Some(link) = self.links.iter_mut().find(|l| l.key == key) {
                if now < heard + JUDGED_AFTER {
                    link.rate = link.rate.min(seen).max(1);
                } else if link.doubted.is_none() {
                    link.doubted = Some((id, heard + STALL));
                    heard_out = true;
                }
                fx.push(Fx::Log(format!("multilink: {} is slow, its range goes to another link", link.who.interface)));
            }
            if !heard_out {
                fx.push(Fx::Cancel(id));
            }
        }
    }

    /// Give the next ranges to the links that finish them first.
    fn dispatch(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        loop {
            // A range that lost its link comes before a new one.
            let orphan = self.slots.iter().position(|s| s.fetch.is_none() && s.got < s.len);
            let (first, left) = match orphan {
                Some(at) => (self.slots[at].first + self.slots[at].got, self.slots[at].len - self.slots[at].got),
                None => (self.tail, self.total - self.tail),
            };
            if left == 0 {
                return;
            }
            let late = orphan.is_some_and(|at| now + JUDGED_AFTER >= self.slots[at].progress_at + REPLACE_BY);
            let room = self.demand.room.saturating_sub(self.held_bytes());
            let mut soonest = Micros::MAX;
            let mut pick: Option<(usize, Micros, u64)> = None;
            for &key in &self.using {
                let Some(at) = self.links.iter().position(|l| l.key == key) else { continue };
                let link = &self.links[at];
                // Nothing more is asked of a link while the request it is heard by is out.
                if link.doubted.is_some() {
                    continue;
                }
                let size = if orphan.is_some() { left } else { self.range_size(link).min(left) };
                let done = self.finish(now, key, size);
                soonest = soonest.min(done);
                let free = self.idle(key) && !(self.limited(link) && now < link.next_at);
                if (free || late) && (orphan.is_some() || size <= room) && pick.is_none_or(|(_, best, _)| done < best) {
                    pick = Some((at, done, size));
                }
            }
            let Some((at, done, size)) = pick else { return };
            // Not the link that finishes it first: it takes the range only if the demuxer will
            // not be waiting for it, with as much time again to spare.
            if !late && soonest < done && now + 2 * (done - now) > self.needed_at(now, first) {
                return;
            }
            let id = self.next_id;
            self.next_id += 1;
            let slot_at = orphan.unwrap_or_else(|| {
                self.slots.push_back(Slot { first, len: size, chunks: VecDeque::new(), skip: 0, got: 0, fetch: None, progress_at: now });
                self.tail += size;
                self.slots.len() - 1
            });
            let limited = self.limited(&self.links[at]);
            let planned = (self.slots[slot_at].len - self.slots[slot_at].got) * SECOND / self.pace(&self.links[at]).max(1);
            let link = &mut self.links[at];
            let slot = &mut self.slots[slot_at];
            slot.fetch = Some(Fetch { id, link: link.key, since: now, progress_at: now, from: slot.got, headed: None, planned, judged_at: now, judged_got: slot.got, watched: true });
            link.carried_until = now + SHARE_HOLD;
            if limited {
                // The share is a rate over time: the next range waits out what this one takes.
                link.next_at = link.next_at.max(now.saturating_sub(JUDGED_AFTER)) + size * SECOND / WIRE_SHARE;
            }
            fx.push(Fx::Start(Request {
                id,
                generation: self.generation,
                source: link.who.address,
                first,
                last: slot.first + slot.len - 1,
                bounded: link.wire(),
            }));
        }
    }

    /// One line whenever the links that carry change, one has shown its rate, or one fails.
    /// None while a link has no rate to name.
    fn tell(&mut self, fx: &mut Vec<Fx>) {
        let carrying = || self.using.iter().filter_map(|&key| self.links.iter().find(|l| l.key == key));
        if carrying().any(|l| l.rate == 0) {
            return;
        }
        let shown = carrying().filter(|l| l.samples >= TRIAL_RANGES).count();
        if self.told.0 == self.using && self.told.1 == self.sharing && self.told.2 == self.failovers && self.told.3 == shown {
            return;
        }
        let links: Vec<String> = carrying()
            .map(|l| format!("{} {} Mbit/s{}", l.who.interface, l.rate * 8 / 1_000_000, if self.limited(l) { " (held to 70)" } else { "" }))
            .collect();
        fx.push(Fx::Log(format!("multilink: using {}; failovers {}", links.join(" + "), self.failovers)));
        self.told = (self.using.clone(), self.sharing, self.failovers, shown);
    }

    /// The fill under way is over: one line of what it brought, each link's part in interface
    /// order. A fill shorter than [`FILL_TOLD`] gets none.
    fn filled(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        let Some(fill) = self.fill.take() else { return };
        let took = now - fill.since;
        if took < FILL_TOLD || fill.bytes == 0 {
            return;
        }
        // Bits a microsecond are megabits a second.
        let mut links: Vec<(&str, u64)> = self.links.iter().map(|l| (l.who.interface.as_str(), l.filled * 8 / took)).collect();
        links.sort_unstable();
        let links: Vec<String> = links.iter().map(|(name, mbps)| format!("{name} {mbps}")).collect();
        fx.push(Fx::Log(format!(
            "multilink: fill {} Mbit/s ({}) reserve {}→{} ms in {} ms",
            fill.bytes * 8 / took,
            links.join(", "),
            fill.low_ms,
            self.demand.reserve_ms,
            took / MS
        )));
    }

    // ---- small things ------------------------------------------------------------------------

    /// A proven link other than the wire the default reader already uses, and room to carry.
    fn takes_over(&self) -> bool {
        !self.spoiled
            && self.head < self.total
            && self.demand.room >= ROOM_FLOOR
            && self.links.iter().any(|l| l.proven() && !l.who.control)
    }

    /// This link's share is limited now: it is the wire, and another link is carrying too.
    fn limited(&self, link: &Link) -> bool {
        self.sharing && link.wire()
    }

    fn idle(&self, key: u16) -> bool {
        !self.slots.iter().any(|s| s.fetch.as_ref().is_some_and(|f| f.link == key))
    }

    fn range_size(&self, link: &Link) -> u64 {
        let rate = if self.limited(link) { link.rate.min(WIRE_SHARE) } else { link.rate };
        (rate * RANGE_SPAN_MS / 1_000).clamp(RANGE_FLOOR, RANGE_CEIL)
    }

    /// What a range is planned at over `link`: its measured rate, and before its first range the
    /// best any link has shown, so that it is given one to measure.
    fn pace(&self, link: &Link) -> u64 {
        if link.rate > 0 {
            return link.rate;
        }
        self.links.iter().map(|l| l.rate).max().unwrap_or(0).max(RANGE_FLOOR * 1_000 / RANGE_SPAN_MS)
    }

    /// When link `key` would have `bytes` more fetched: after what it is fetching now, and after
    /// its share comes free. Never, while it is heard out.
    fn finish(&self, now: Micros, key: u16, bytes: u64) -> Micros {
        let Some(link) = self.links.iter().find(|l| l.key == key && l.doubted.is_none()) else { return Micros::MAX / 4 };
        let rate = self.pace(link);
        let mut free = now;
        for slot in &self.slots {
            if slot.fetch.as_ref().is_some_and(|f| f.link == key) {
                free += (slot.len - slot.got) * SECOND / rate;
            }
        }
        if self.limited(link) {
            free = free.max(link.next_at);
        }
        free + bytes * SECOND / rate
    }

    /// When the demuxer will want byte `at`: after the media already queued, and after the file
    /// between here and there at the film's rate.
    fn needed_at(&self, now: Micros, at: u64) -> Micros {
        let queued = Micros::from(self.demand.reserve_ms) * MS;
        let ahead = if self.demand.film_bps == 0 { 0 } else { at.saturating_sub(self.head) * 8 * SECOND / self.demand.film_bps };
        now + queued + ahead
    }

    fn name(&self, key: u16) -> &str {
        self.links.iter().find(|l| l.key == key).map_or("a link", |l| l.who.interface.as_str())
    }

    fn slot_of(&self, id: ReqId) -> Option<usize> {
        self.slots.iter().position(|s| s.fetch.as_ref().is_some_and(|f| f.id == id))
    }

    fn probing(&self, id: ReqId) -> Option<usize> {
        self.links.iter().position(|l| matches!(&l.proof, Proof::Probing(p) if p.id == id))
    }

    /// The link heard out by request `id`.
    fn doubting(&self, id: ReqId) -> Option<usize> {
        self.links.iter().position(|l| l.doubted.is_some_and(|(asked, _)| asked == id))
    }

    /// Cancel every range asked for, and every request a link is heard out by.
    fn cancel_fetches(&mut self, fx: &mut Vec<Fx>) {
        for slot in &mut self.slots {
            if let Some(fetch) = slot.fetch.take() {
                fx.push(Fx::Cancel(fetch.id));
            }
        }
        for link in &mut self.links {
            if let Some((id, _)) = link.doubted.take() {
                fx.push(Fx::Cancel(id));
            }
        }
    }

    /// Proofs under way are for a range the trusted path will no longer deliver.
    fn abandon_proofs(&mut self, now: Micros, fx: &mut Vec<Fx>) {
        for link in &mut self.links {
            if let Proof::Probing(probe) = &link.proof {
                if !probe.done {
                    fx.push(Fx::Cancel(probe.id));
                }
                link.proof = Proof::Waiting { from: now };
            }
        }
    }

    /// The read position moved to `to` with nothing in hand.
    fn jump(&mut self, now: Micros, to: u64, fx: &mut Vec<Fx>) {
        self.abandon_proofs(now, fx);
        self.calm_from = now + CALM;
        self.head = to;
        self.tail = to;
    }

    /// The machine was not stepped for `gap`, longer than a driver waiting on its transport
    /// leaves it: the reader was elsewhere, and what a connection held back or let pile up
    /// meanwhile is not its link's doing. No request's clock ran, and a range that was out is no
    /// measure of its link's rate.
    fn unwatched(&mut self, gap: Micros) {
        for slot in &mut self.slots {
            let Some(fetch) = &mut slot.fetch else { continue };
            fetch.watched = false;
            fetch.since += gap;
            fetch.progress_at += gap;
            fetch.judged_at += gap;
            fetch.headed = fetch.headed.map(|at| at + gap);
        }
        for link in &mut self.links {
            if let Proof::Probing(probe) = &mut link.proof {
                probe.since += gap;
                probe.progress_at += gap;
            }
            link.doubted = link.doubted.map(|(id, by)| (id, by + gap));
        }
    }

    /// Drop the links `gone` names, with what they were doing.
    fn forget(&mut self, gone: impl Fn(&Link) -> bool, fx: &mut Vec<Fx>) {
        for at in (0..self.links.len()).rev() {
            if !gone(&self.links[at]) {
                continue;
            }
            let link = self.links.remove(at);
            if let Proof::Probing(probe) = &link.proof {
                if !probe.done {
                    fx.push(Fx::Cancel(probe.id));
                }
            }
            if let Some((id, _)) = link.doubted {
                fx.push(Fx::Cancel(id));
            }
            for slot in &mut self.slots {
                if let Some(fetch) = slot.fetch.take_if(|f| f.link == link.key) {
                    fx.push(Fx::Cancel(fetch.id));
                }
            }
            self.using.retain(|&key| key != link.key);
            fx.push(Fx::Log(format!("multilink: {} is no longer offered", link.who.interface)));
        }
    }

    /// Stop carrying: nothing more is asked, and the default reader resumes after what is still
    /// read out, which is what is in hand in order when `keep`, and nothing otherwise.
    fn retire(&mut self, now: Micros, why: &str, keep: bool, fx: &mut Vec<Fx>) {
        self.filled(now, fx);
        self.cancel_fetches(fx);
        self.abandon_proofs(now, fx);
        if !keep {
            self.slots.clear();
        }
        let mut end = self.head;
        let mut kept = 0;
        for slot in &mut self.slots {
            end = slot.first + slot.got;
            kept += 1;
            if slot.got < slot.len {
                slot.len = slot.got;
                break;
            }
        }
        self.slots.drain(kept..);
        self.tail = end;
        self.mode = Mode::Draining;
        self.generation += 1;
        self.using.clear();
        self.sharing = false;
        self.told = (Vec::new(), false, self.failovers, 0);
        fx.push(Fx::Log(format!("multilink: handing back at byte {end}: {why}")));
    }

    /// An answer was not the object. Ranges already joined were each checked; nothing more is
    /// joined for this file.
    fn spoil(&mut self, now: Micros, why: &str, fx: &mut Vec<Fx>) {
        self.spoiled = true;
        fx.push(Fx::Log(format!("multilink: off for this file: {why}")));
        if self.mode == Mode::Carrying {
            self.retire(now, "an answer was not the file", true, fx);
        } else {
            self.abandon_proofs(now, fx);
        }
    }
}
