//! TrueHD access units in MAT frames inside IEC 61937 bursts, the form a receiver takes TrueHD in.
//!
//! [`Muxer`] is FFmpeg 9.0.2's `spdif_header_truehd` (libavformat/spdifenc.c) line for line: each
//! access unit is laid into a 61424-byte MAT frame after zero padding worked out from its 16-bit
//! input timing, around three fixed MAT codes, and a full frame leaves as one 61440-byte burst
//! (preamble, the frame, eight zero bytes) in 16-bit words of either [`ByteOrder`]: 20 ms of the
//! 192 kHz, 8-channel, 16-bit carrier.
//!
//! [`Packer`] wraps it in dts_restore_plus PR 45. FFmpeg caps the padding at half a MAT frame and
//! lays a larger timing gap in with none, after which the stream plays late and the receiver keeps
//! showing TrueHD over silence. At 16 access units a gap exceeds that cap even after the previous
//! access unit is subtracted, so a major sync that far from the last timing starts a fresh muxer,
//! dropping the partial burst. Bursts carry the time of the access unit that started their
//! sequence plus 20 ms each.
use std::collections::VecDeque;

/// One burst: 20 ms of the carrier.
pub const BURST_BYTES: usize = 61440;
/// A burst's duration.
pub const BURST_NS: i64 = 20_000_000;
/// The MAT frame inside a burst.
const MAT_FRAME: usize = 61424;

/// Pa Pb (sync), Pc 0x16 (TrueHD), Pd 61424 (the frame's length), as big-endian words.
const PREAMBLE: [u8; 8] = [0xF8, 0x72, 0x4E, 0x1F, 0x00, 0x16, 0xEF, 0xF0];

/// The byte order of the carrier's 16-bit words. LG's DSP takes the first byte of a word as its
/// high byte: on the C4 a little-endian carrier (FFmpeg's default) reached the receiver as PCM
/// silence, and upstream feeds FFmpeg's `spdif_flags=be`. The helper may name the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ByteOrder {
    #[default]
    Big,
    Little,
}

/// The carrier's silence: what the sink is fed whenever carrier bytes may not reach it.
pub static ZERO_BURST: [u8; BURST_BYTES] = [0; BURST_BYTES];

const START_CODE: [u8; 20] = [
    0x07, 0x9E, 0x00, 0x03, 0x84, 0x01, 0x01, 0x01, 0x80, 0x00, 0x56, 0xA5, 0x3B, 0xF4, 0x81, 0x83,
    0x49, 0x80, 0x77, 0xE0,
];
const MIDDLE_CODE: [u8; 12] = [0xC3, 0xC1, 0x42, 0x49, 0x3B, 0xFA, 0x82, 0x83, 0x49, 0x80, 0x77, 0xE0];
const END_CODE: [u8; 16] = [0xC3, 0xC2, 0xC0, 0xC4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x97, 0x11];

/// Where each code sits in the MAT frame.
const CODES: [(usize, &[u8]); 3] = [
    (0, &START_CODE),
    (30708, &MIDDLE_CODE),
    (MAT_FRAME - END_CODE.len(), &END_CODE),
];

/// Why the carrier cannot carry this stream: the request goes back to the normal audio path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A major sync too short to read its rate, or a rate outside the 48 kHz family: MAT has no
    /// 44.1 kHz carrier.
    Rate,
    /// An access unit FFmpeg's muxer refuses (shorter than 10 bytes, or an unreadable major sync).
    Invalid,
}

impl Fallback {
    pub fn reason(self) -> &'static str {
        match self {
            Fallback::Rate => "TrueHD rate outside the 48 kHz family",
            Fallback::Invalid => "an access unit the MAT muxer refuses",
        }
    }
}

/// FFmpeg's `spdif_header_truehd` state: one MAT frame being filled.
struct Muxer {
    frame: Box<[u8]>,
    filled: usize,
    samples_per_frame: u32,
    prev_size: i32,
    prev_time: u16,
}

impl Muxer {
    fn new() -> Muxer {
        Muxer { frame: vec![0; MAT_FRAME].into_boxed_slice(), filled: 0, samples_per_frame: 0, prev_size: 0, prev_time: 0 }
    }

    fn reset(&mut self) {
        self.filled = 0;
        self.samples_per_frame = 0;
        self.prev_size = 0;
        self.prev_time = 0;
    }

    /// Lays `pkt` in; `emit` gets every MAT frame this completes.
    fn push(&mut self, pkt: &[u8], emit: &mut impl FnMut(&[u8])) -> Result<(), Fallback> {
        if pkt.len() < 10 {
            return Err(Fallback::Invalid);
        }
        if be24(&pkt[4..]) == 0xf8726f {
            let ratebits = match pkt[7] {
                0xba => pkt[8] >> 4,
                0xbb => pkt[9] >> 4,
                _ => return Err(Fallback::Invalid),
            };
            self.samples_per_frame = 40 << (ratebits & 3);
        }
        if self.samples_per_frame == 0 {
            return Err(Fallback::Invalid);
        }
        let timing = be16(&pkt[2..]);
        let mut padding = 0i32;
        let mut total = pkt.len() as i32;
        if self.prev_size != 0 {
            // 2560 bytes of carrier per 1/1200 s, whatever the rate in the 48 kHz family.
            let delta = i32::from(timing.wrapping_sub(self.prev_time));
            padding = delta * 2560 / self.samples_per_frame as i32 - self.prev_size;
            if !(0..(MAT_FRAME / 2) as i32).contains(&padding) {
                padding = 0; // FFmpeg's "Unusual frame timing"
            }
        }
        let mut next = CODES.iter().position(|c| self.filled <= c.0).ok_or(Fallback::Invalid)?;
        let mut data = pkt;
        while padding != 0 || !data.is_empty() || CODES[next].0 == self.filled {
            if CODES[next].0 == self.filled {
                let code = CODES[next].1;
                self.frame[self.filled..self.filled + code.len()].copy_from_slice(code);
                self.filled += code.len();
                let mut code_left = code.len() as i32;
                next += 1;
                if next == CODES.len() {
                    next = 0;
                    emit(&self.frame);
                    self.filled = 0;
                    // the burst's preamble and padding count like a code
                    code_left += (BURST_BYTES - MAT_FRAME) as i32;
                }
                let as_padding = padding.min(code_left);
                padding -= as_padding;
                total += code_left - as_padding;
            }
            if padding != 0 {
                let n = ((CODES[next].0 - self.filled) as i32).min(padding);
                self.frame[self.filled..self.filled + n as usize].fill(0);
                self.filled += n as usize;
                padding -= n;
                if padding != 0 {
                    continue;
                }
            }
            if !data.is_empty() {
                let n = (CODES[next].0 - self.filled).min(data.len());
                self.frame[self.filled..self.filled + n].copy_from_slice(&data[..n]);
                self.filled += n;
                data = &data[n..];
            }
        }
        self.prev_size = total;
        self.prev_time = timing;
        Ok(())
    }
}

/// A finished burst and the time it plays at.
pub struct Burst {
    pub pts: i64,
    pub bytes: Box<[u8]>,
}

/// Access units in, timed bursts out (PR 45's `truehdmat` element without GStreamer).
pub struct Packer {
    mux: Muxer,
    order: ByteOrder,
    /// Samples per access unit; zero until a major sync this carrier takes.
    samples_per_unit: u32,
    last_timing: u16,
    /// The time of the access unit that started this sequence, and the bursts it has made since.
    anchor: Option<i64>,
    made: i64,
    ready: VecDeque<Burst>,
    spare: Vec<Box<[u8]>>,
}

impl Packer {
    pub fn new(order: ByteOrder) -> Packer {
        Packer {
            mux: Muxer::new(),
            order,
            samples_per_unit: 0,
            last_timing: 0,
            anchor: None,
            made: 0,
            ready: VecDeque::new(),
            spare: Vec::new(),
        }
    }

    /// Packs one access unit timed `pts`. `Ok(Some(delta))`: this major sync restarted the muxer
    /// `delta` samples after the last timing. Access units before the first major sync are dropped.
    pub fn push(&mut self, au: &[u8], pts: i64) -> Result<Option<u16>, Fallback> {
        let major = au.len() >= 8 && be32(&au[4..]) == 0xf8726fba;
        if major && (au.len() < 10 || au[8] >> 4 > 2) {
            return Err(Fallback::Rate);
        }
        if !major && self.samples_per_unit == 0 {
            return Ok(None);
        }
        if au.len() < 10 {
            return Err(Fallback::Invalid);
        }
        let timing = be16(&au[2..]);
        let mut resync = None;
        if major {
            let samples_per_unit = 40u32 << (au[8] >> 4);
            let delta = timing.wrapping_sub(self.last_timing);
            if self.samples_per_unit != 0 && u32::from(delta) >= 16 * samples_per_unit {
                self.mux.reset();
                self.anchor = None;
                resync = Some(delta);
            }
            self.samples_per_unit = samples_per_unit;
        }
        self.last_timing = timing;
        let anchor = *self.anchor.get_or_insert_with(|| {
            self.made = 0;
            pts
        });
        let Packer { mux, order, made, ready, spare, .. } = self;
        mux.push(au, &mut |frame: &[u8]| {
            let mut bytes = spare.pop().unwrap_or_else(|| vec![0; BURST_BYTES].into_boxed_slice());
            lay_burst(frame, &mut bytes, *order);
            ready.push_back(Burst { pts: anchor + *made * BURST_NS, bytes });
            *made += 1;
        })?;
        Ok(resync)
    }

    /// The oldest burst not yet taken.
    pub fn front(&self) -> Option<&Burst> {
        self.ready.front()
    }

    /// The front burst reached the sink.
    pub fn pop(&mut self) {
        if let Some(b) = self.ready.pop_front() {
            self.spare.push(b.bytes);
        }
    }

    /// A seek or flush: the next sequence starts at the next major sync, and nothing made before
    /// it plays.
    pub fn flush(&mut self) {
        self.mux.reset();
        self.samples_per_unit = 0;
        self.anchor = None;
        while self.front().is_some() {
            self.pop();
        }
    }
}

/// Preamble and MAT frame in `order`, then zeros to the burst's length. Both are big-endian as
/// laid; little-endian swaps every word.
fn lay_burst(frame: &[u8], out: &mut [u8], order: ByteOrder) {
    let (head, rest) = out.split_at_mut(PREAMBLE.len());
    let (body, tail) = rest.split_at_mut(MAT_FRAME);
    match order {
        ByteOrder::Big => {
            head.copy_from_slice(&PREAMBLE);
            body.copy_from_slice(frame);
        }
        ByteOrder::Little => {
            let words = head.chunks_exact_mut(2).zip(PREAMBLE.chunks_exact(2));
            for (o, i) in words.chain(body.chunks_exact_mut(2).zip(frame.chunks_exact(2))) {
                o[0] = i[1];
                o[1] = i[0];
            }
        }
    }
    tail.fill(0);
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be24(b: &[u8]) -> u32 {
    u32::from_be_bytes([0, b[0], b[1], b[2]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 160 access units of FFmpeg 5.1's TrueHD encoding of 48 kHz 5.1 silence: a major
    /// sync every 16, timing 65496 then 0 and up by 40 (tests/fixtures/README.md).
    const FIXTURE: &[u8] = include_bytes!("../../../../tests/fixtures/truehd-silence-160.thd");
    const BOUNDARY: usize = 64;

    fn frames() -> Vec<&'static [u8]> {
        let mut out = Vec::new();
        let mut rest = FIXTURE;
        while !rest.is_empty() {
            let n = usize::from(be16(rest) & 0xfff) * 2;
            out.push(&rest[..n]);
            rest = &rest[n..];
        }
        assert_eq!(out.len(), 160);
        assert_eq!((be16(&out[0][2..]), be16(&out[1][2..])), (65496, 0));
        assert_eq!(be32(&out[BOUNDARY][4..]), 0xf8726fba);
        out
    }

    /// Feeds frame `i` with its timing replaced (and its rate code, on a major sync), at i/1200 s.
    fn feed(p: &mut Packer, frame: &[u8], i: usize, timing: u16, rate_code: u8) -> Result<Option<u16>, Fallback> {
        let mut au = frame.to_vec();
        au[2..4].copy_from_slice(&timing.to_be_bytes());
        if rate_code != 0 && be32(&au[4..]) == 0xf8726fba {
            au[8] = (au[8] & 0x0f) | rate_code << 4;
        }
        p.push(&au, i as i64 * 1_000_000_000 / 1200)
    }

    fn shifted(p: &mut Packer, frame: &[u8], i: usize, shift: u16) -> Option<u16> {
        feed(p, frame, i, be16(&frame[2..]).wrapping_add(shift), 0).unwrap()
    }

    fn drain(p: &mut Packer) -> Vec<(i64, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some(b) = p.front() {
            out.push((b.pts, b.bytes.to_vec()));
            p.pop();
        }
        out
    }

    #[test]
    fn bursts_match_ffmpeg_spdif_output_in_either_order() {
        // `ffmpeg -f truehd -i truehd-silence-160.thd -c copy [-spdif_flags be] -f spdif` (FFmpeg 5.1.9)
        for (order, sha) in [
            (ByteOrder::Little, "0957c22facc5654ae96f1ce2dde9ccb2fb872d0c78eda121e1bd1d351583a714"),
            (ByteOrder::Big, "7a523e5e4ae06abafd07436e1d9a97b73665b21709059607a097bc7996be929b"),
        ] {
            let mut p = Packer::new(order);
            for (i, f) in frames().iter().enumerate() {
                assert_eq!(shifted(&mut p, f, i, 0), None);
            }
            let bursts = drain(&mut p);
            let pts: Vec<i64> = bursts.iter().map(|b| b.0).collect();
            assert_eq!(pts, (0..6).map(|k| k * BURST_NS).collect::<Vec<_>>());
            let all: Vec<u8> = bursts.into_iter().flat_map(|b| b.1).collect();
            let hex: String = plx_base::sha256::sha256(&all).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, sha, "{order:?}");
        }
    }

    #[test]
    fn a_wrap_or_short_gap_at_major_sync_keeps_the_muxer() {
        let fr = frames();
        // the 16-bit wrap moved onto the boundary's major sync, then an 80-sample gap there
        let wrap = 0u16.wrapping_sub(be16(&fr[BOUNDARY][2..]));
        for gap in [None, Some(40)] {
            let mut p = Packer::new(ByteOrder::Big);
            for (i, f) in fr.iter().enumerate() {
                let shift = gap.map_or(wrap, |g| if i < BOUNDARY { 0 } else { g });
                assert_eq!(shifted(&mut p, f, i, shift), None);
            }
            assert!(drain(&mut p).len() >= 5);
        }
    }

    #[test]
    fn a_1770_sample_jump_restarts_like_a_fresh_packer() {
        let fr = frames();
        let (mut actual, mut fresh) = (Packer::new(ByteOrder::Big), Packer::new(ByteOrder::Big));
        for (i, f) in fr[..BOUNDARY].iter().enumerate() {
            shifted(&mut actual, f, i, 0);
        }
        assert_eq!(drain(&mut actual).len(), 2);
        let mut resyncs = Vec::new();
        for (i, f) in fr.iter().enumerate().skip(BOUNDARY) {
            resyncs.extend(shifted(&mut actual, f, i, 1770 - 40));
            shifted(&mut fresh, f, i, 1770 - 40);
        }
        assert_eq!(resyncs, [1770]);
        let (a, f) = (drain(&mut actual), drain(&mut fresh));
        assert!(!f.is_empty());
        assert!(a == f, "the same bursts at the same times");
        assert_eq!(a[0].0, BOUNDARY as i64 * 1_000_000_000 / 1200);
    }

    #[test]
    fn sixteen_access_units_at_major_sync_restart_at_each_rate() {
        let fr = frames();
        for (rate_code, step, below) in [(0u8, 40u16, 639u16), (1, 80, 1279), (2, 160, 2559)] {
            for gap in [below, below + 1] {
                let shift = gap - step;
                let mut actual = Packer::new(ByteOrder::Big);
                for (i, f) in fr[..BOUNDARY].iter().enumerate() {
                    assert_eq!(feed(&mut actual, f, i, i as u16 * step, rate_code), Ok(None));
                }
                assert!(!drain(&mut actual).is_empty());
                let mut fresh = Packer::new(ByteOrder::Big);
                let mut resyncs = Vec::new();
                for (i, f) in fr.iter().enumerate().skip(BOUNDARY) {
                    let timing = (i as u16 * step).wrapping_add(shift);
                    resyncs.extend(feed(&mut actual, f, i, timing, rate_code).unwrap());
                    feed(&mut fresh, f, i, timing, rate_code).unwrap();
                }
                let (a, f) = (drain(&mut actual), drain(&mut fresh));
                assert!(!a.is_empty());
                if gap == below {
                    assert!(resyncs.is_empty(), "{gap} samples at rate code {rate_code} keeps the muxer");
                } else {
                    assert_eq!(resyncs, [gap], "rate code {rate_code}");
                    assert!(a == f, "rate code {rate_code}: the same bursts as a fresh packer");
                }
            }
        }
    }

    #[test]
    fn a_flush_waits_for_the_next_major_sync() {
        let fr = frames();
        let (mut actual, mut fresh) = (Packer::new(ByteOrder::Big), Packer::new(ByteOrder::Big));
        for (i, f) in fr[..BOUNDARY].iter().enumerate() {
            shifted(&mut actual, f, i, 0);
        }
        actual.flush();
        assert!(actual.front().is_none(), "nothing made before the flush plays");
        for (i, f) in fr.iter().enumerate().skip(BOUNDARY - 1) {
            assert_eq!(shifted(&mut actual, f, i, 1730), None);
            shifted(&mut fresh, f, i, 1730);
        }
        let (a, f) = (drain(&mut actual), drain(&mut fresh));
        assert!(!a.is_empty() && a == f);
        assert_eq!(a[0].0, BOUNDARY as i64 * 1_000_000_000 / 1200, "the minor unit before it is dropped");
    }

    #[test]
    fn a_short_or_44k_major_sync_falls_back() {
        let fr = frames();
        let short: &[u8] = &[0, 4, 0, 0, 0xf8, 0x72, 0x6f, 0xba];
        let rate_44k: &[u8] = &[0, 5, 0, 0, 0xf8, 0x72, 0x6f, 0xba, 0x80, 0];
        for bad in [short, rate_44k] {
            let mut p = Packer::new(ByteOrder::Big);
            for (i, f) in fr[..BOUNDARY].iter().enumerate() {
                shifted(&mut p, f, i, 0);
            }
            drain(&mut p);
            assert_eq!(p.push(bad, 0), Err(Fallback::Rate));
            assert!(p.front().is_none());
        }
        assert_eq!(Packer::new(ByteOrder::Big).push(&[0, 4, 0, 0, 0, 0, 0, 0], 0), Ok(None), "a minor unit before any sync waits");
    }
}
