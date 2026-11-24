//! The audio lane's TrueHD side: the demuxer's packets in, one 20 ms burst per feed out, carrying
//! TrueHD only while the gate opens and zeros otherwise.
use std::time::Instant;

use super::contract::Carrier;
use super::gate::{Decision, Gate, OpeningRewind, PlayHold};
use super::mat::{Packer, ZERO_BURST};
use super::session::Session;

pub struct Feeder {
    packer: Packer,
    gate: Gate,
    /// `None` once the carrier is given up: the route goes back with it.
    session: Option<Session>,
    /// Bursts fed since the gate last changed its answer, for the log.
    run: u32,
    carrying: bool,
    /// The clock's hold while the route is not audible ([`Feeder::holds_play`]).
    hold: PlayHold,
    /// The first start's repeat of its opening ([`OpeningRewind`]).
    opening: OpeningRewind,
}

impl Feeder {
    /// A carrier session for this Load; one whose route session cannot start has given up already.
    pub fn start(carrier: Carrier) -> Feeder {
        crate::player::log(&format!(
            "truehd: carrier for this Load ({:?}-endian, sink caps {}, pcm-multi-ch {})",
            carrier.order,
            carrier.sink_caps.as_deref().unwrap_or("LG's own"),
            if carrier.start_as_bitstream { "toggled as upstream" } else { "left alone" }
        ));
        let packer = Packer::new(carrier.order);
        let session = Session::start(carrier);
        if session.is_none() {
            super::give_up("the route worker cannot start");
        }
        Feeder { packer, gate: Gate::new(), session, run: 0, carrying: false, hold: PlayHold::default(), opening: OpeningRewind::default() }
    }

    /// A feeder with a session that has no helper (`Session::for_test`): zeros while the gate arms.
    #[cfg(test)]
    pub fn for_test() -> Feeder {
        let carrier = Carrier::default();
        Feeder { packer: Packer::new(carrier.order), gate: Gate::new(), session: Some(Session::for_test(carrier)), run: 0, carrying: false, hold: PlayHold::default(), opening: OpeningRewind::default() }
    }

    pub fn gave_up(&self) -> bool {
        self.session.is_none()
    }

    /// Pack one demuxer packet; false when the stream cannot be carried.
    pub fn push(&mut self, packet: &[u8], pts: i64) -> bool {
        for au in access_units(packet) {
            match self.packer.push(au, pts) {
                Ok(Some(delta)) => crate::player::log(&format!("truehd: MAT resync at major sync, timing delta {delta}")),
                Ok(None) => {}
                Err(fallback) => {
                    self.stop(fallback.reason());
                    return false;
                }
            }
        }
        true
    }

    /// The next burst's time, once one is packed.
    pub fn front_pts(&self) -> Option<i64> {
        self.packer.front().map(|b| b.pts)
    }

    /// A burst too old to feed (a backward jump).
    pub fn skip(&mut self) {
        self.packer.pop();
    }

    /// The bytes of the next burst: TrueHD when the gate opens, zeros while it does not; `None`
    /// once the carrier is given up.
    pub fn next(&mut self, now: Instant) -> Option<&[u8]> {
        let session = self.session.as_ref()?;
        if session.take_flush() {
            self.restart();
        }
        let facts = self.session.as_ref()?.shared().facts();
        if let Some((start_ns, silent)) = self.opening.poll(now, &facts, self.gate.epoch()) {
            super::request_rewind(start_ns, silent.as_millis() as u64);
        }
        let decision = self.gate.decide(now, &facts);
        let carrying = match decision {
            Decision::Carrier => true,
            Decision::Zeros => false,
            Decision::GiveUp(why) => {
                self.stop(why);
                return None;
            }
        };
        if carrying != self.carrying {
            let what = if carrying { "carrier on" } else { "zeros" };
            crate::player::log(&format!("truehd: {what} after {} bursts", self.run));
            self.carrying = carrying;
            self.run = 0;
        }
        match self.packer.front() {
            Some(burst) if carrying => Some(&burst.bytes[..]),
            _ => Some(&ZERO_BURST[..]),
        }
    }

    /// The sink took the burst [`next`](Self::next) gave (zeros or TrueHD; the packed burst goes
    /// either way, so the timeline keeps its pace).
    pub fn fed(&mut self) {
        self.run += 1;
        self.packer.pop();
    }

    /// The pipeline accepted the Load's Play, from `start_ns` in the content's time.
    pub fn note_played(&mut self, now: Instant, start_ns: i64) {
        self.opening.note_played(now, start_ns);
    }

    /// Whether the clock must stay stopped: the carrier plays this start, seek or resume and the
    /// receiver does not hear it yet, for [`super::gate::PLAY_HOLD_CAP`] at most. False once the
    /// carrier is given up; the gate's own give-ups are the only ones.
    pub fn holds_play(&mut self, now: Instant) -> bool {
        let Some(session) = &self.session else { return false };
        let facts = session.shared().facts();
        self.hold.waits(now, &facts, self.gate.epoch())
    }

    /// A seek, a resume or a flush: the packets queued are the old position's, and the route is
    /// confirmed again before TrueHD flows.
    pub fn restart(&mut self) {
        self.packer.flush();
        self.gate.restart();
        self.hold.restart();
        self.opening.end();
        if let Some(session) = &self.session {
            session.rearm(self.gate.epoch());
        }
    }

    fn stop(&mut self, why: &'static str) {
        if self.session.take().is_some() {
            crate::player::log(&format!("truehd: gave up ({why})"));
            super::give_up(why);
        }
    }
}

/// The access units in a demuxer packet: each starts with its length in 16-bit words in the low
/// 12 bits of its first two bytes. A Matroska block may hold several; a length that does not fit
/// leaves the rest as one unit for the muxer to judge.
fn access_units(mut packet: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if packet.is_empty() {
            return None;
        }
        let words = packet.get(..2).map_or(0, |h| usize::from(u16::from_be_bytes([h[0], h[1]]) & 0x0fff));
        let len = if words >= 2 && words * 2 <= packet.len() { words * 2 } else { packet.len() };
        let (au, rest) = packet.split_at(len);
        packet = rest;
        Some(au)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Load's first start does not hold: the route cannot arm until Play builds LG's renderer.
    #[test]
    fn the_first_start_is_not_held_for_a_route_that_has_not_armed() {
        let mut feeder = Feeder::for_test();
        assert!(!feeder.holds_play(Instant::now()));
    }

    #[test]
    fn a_packet_is_split_at_each_access_unit_s_length() {
        let packet = [0x00, 0x02, 0xaa, 0xbb, 0x00, 0x03, 1, 2, 3, 4];
        let units: Vec<&[u8]> = access_units(&packet).collect();
        assert_eq!(units, [&packet[..4], &packet[4..10]]);
        let short = [0x00, 0x09, 1, 2];
        assert_eq!(access_units(&short).collect::<Vec<_>>(), [&short[..]], "a length past the end stays whole");
    }
}
