//! The seek-preview generator's FFmpeg half (`crate::seekprev`): a demuxer of the film's Part
//! through [`SideIo`] and a software H.264/HEVC decoder, opened for one batch and closed after it.
//! It decodes the keyframe at or before a movie time and shrinks the picture to the scrub card's
//! size in Rust (the bundled FFmpeg carries no swscale). A `#[path]` child of `ff.rs`, like
//! `ff_subside.rs`.
//!
//! No URL, host or token is ever logged from here.

use super::subside::{side_read_cb, side_seek_cb, SideIo, SideOpenErr, SideStop};
use super::*;
use std::sync::Arc;

/// The scrub card's still size (`appkit::scrub_preview::REQ_W` × `REQ_H`): a still fits inside it.
const OUT_W: usize = 320;
const OUT_H: usize = 180;
/// Packets read after a seek before giving up on a keyframe of the video stream there. A seek by
/// timestamp in MPEG-TS lands up to a group of pictures early, audio interleaved.
const KEY_SEARCH_PACKETS: usize = 2_000;
const AVIO_BUF: usize = 65_536;
/// Milliseconds, as an FFmpeg time base.
const MS_TB: AVRational = AVRational { num: 1, den: 1000 };

/// How a batch ended, for the generator's back-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BatchEnd {
    /// The server served it.
    Done,
    /// Its stop was raised: says nothing about the server.
    Stopped,
    /// The server or the network failed it.
    Refused,
    /// The film's video cannot be decoded here.
    Unusable,
}

/// One decoded picture, shrunk to fit the card.
pub(crate) struct Still {
    /// The movie time of the keyframe it was decoded from.
    pub(crate) at_ms: i64,
    pub(crate) w: i32,
    pub(crate) h: i32,
    /// `w`×`h` RGBA, tightly packed.
    pub(crate) rgba: Vec<u8>,
}

/// The decoded picture formats the shrink reads, looked up by name: an enum value is not part of
/// what the ABI gate checks.
#[derive(Clone, Copy)]
struct PixFmts {
    p8: c_int,
    j8: c_int,
    p10: c_int,
}

impl PixFmts {
    unsafe fn lookup() -> PixFmts {
        PixFmts {
            p8: av_get_pix_fmt(c"yuv420p".as_ptr()),
            j8: av_get_pix_fmt(c"yuvj420p".as_ptr()),
            p10: av_get_pix_fmt(c"yuv420p10le".as_ptr()),
        }
    }
}

/// A demuxer and a decoder over one Part, for one batch. Dropping it closes both and the
/// connection.
pub(crate) struct Reader {
    fmt: *mut AVFormatContext,
    avio: *mut AVIOContext,
    io: *mut SideIo,
    pkt: *mut AVPacket,
    ctx: *mut AVCodecContext,
    frame: *mut c_void,
    stop: Arc<SideStop>,
    vi: c_int,
    tb: AVRational,
    /// The stream's clock at movie time 0, in `tb` ticks: for a start-code stream (MPEG-TS) its
    /// first video timestamp, since the playback's demuxer moves such a stream's start to 0
    /// (`stream_origin_ns`); 0 for every other.
    origin: i64,
    /// The sample aspect ratio, which widens the coded width to the display shape.
    sar: f32,
    fmts: PixFmts,
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: each pointer is either null or exclusively owned by this struct, and is freed
        // exactly once, here: the decoder first, the AVIO after the context that read through it,
        // and the callbacks' state last.
        unsafe {
            if !self.frame.is_null() {
                av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                avcodec_free_context(&mut self.ctx);
            }
            if !self.pkt.is_null() {
                av_packet_free(&mut self.pkt);
            }
            if !self.fmt.is_null() {
                avformat_close_input(&mut self.fmt);
            }
            free_avio(self.avio);
            if !self.io.is_null() {
                drop(Box::from_raw(self.io));
            }
        }
    }
}

impl Reader {
    /// Open the Part at `url` and a decoder for its first video stream. `Err` is how the batch
    /// ends.
    pub(crate) unsafe fn open(url: &str, stop: &Arc<SideStop>) -> Result<Reader, BatchEnd> {
        let io = match SideIo::open(url, stop) {
            Ok(io) => io,
            Err(SideOpenErr::Stopped) => return Err(BatchEnd::Stopped),
            Err(SideOpenErr::Failed) => return Err(BatchEnd::Refused),
        };
        // The main demuxer's gate (`ff::demux`): struct offsets built for another FFmpeg are never
        // read through. A batch runs only during a playback, after that demuxer has passed it.
        if !ABI_OK.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(BatchEnd::Unusable);
        }
        let mut r = Reader {
            fmt: std::ptr::null_mut(),
            avio: std::ptr::null_mut(),
            io: Box::into_raw(io),
            pkt: std::ptr::null_mut(),
            ctx: std::ptr::null_mut(),
            frame: std::ptr::null_mut(),
            stop: Arc::clone(stop),
            vi: -1,
            tb: MS_TB,
            origin: 0,
            sar: 1.0,
            fmts: PixFmts::lookup(),
        };
        let buf = av_malloc(AVIO_BUF) as *mut u8;
        if buf.is_null() {
            return Err(BatchEnd::Refused);
        }
        r.avio = avio_alloc_context(buf, AVIO_BUF as c_int, 0, r.io as *mut c_void, Some(side_read_cb), None, Some(side_seek_cb));
        if r.avio.is_null() {
            free_ptr(buf as *mut c_void);
            return Err(BatchEnd::Refused);
        }
        r.fmt = avformat_alloc_context();
        if r.fmt.is_null() {
            return Err(BatchEnd::Refused);
        }
        (*r.fmt).pb = r.avio;
        let mut fmt = r.fmt;
        let rc = avformat_open_input(&mut fmt, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
        r.fmt = fmt; // null on failure: open_input frees the context itself
        if rc < 0 || r.fmt.is_null() {
            return Err(r.read_failed());
        }
        r.pkt = av_packet_alloc();
        r.frame = av_frame_alloc();
        if r.pkt.is_null() || r.frame.is_null() {
            return Err(BatchEnd::Refused);
        }
        let streams = (*r.fmt).streams;
        for i in 0..(*r.fmt).nb_streams {
            if (*stream_codecpar(*streams.add(i as usize))).codec_type == AVMEDIA_TYPE_VIDEO {
                r.vi = i as c_int;
                break;
            }
        }
        if r.vi < 0 {
            return Err(BatchEnd::Unusable);
        }
        let st = *streams.add(r.vi as usize);
        let cp = stream_codecpar(st);
        let id = (*cp).codec_id;
        // The route gate read the server's description; this is the file's own. A size the header
        // does not carry (MPEG-TS before a probe) reads 0 and passes.
        if (id != AV_CODEC_ID_H264 && id != AV_CODEC_ID_HEVC) || (*cp).width > 1920 || (*cp).height > 1088 {
            return Err(BatchEnd::Unusable);
        }
        r.tb = stream_time_base(st);
        if r.tb.num <= 0 || r.tb.den <= 0 {
            return Err(BatchEnd::Unusable);
        }
        let sar = (*cp).sample_aspect_ratio;
        if sar.num > 0 && sar.den > 0 {
            r.sar = sar.num as f32 / sar.den as f32;
        }
        let codec = avcodec_find_decoder(id);
        if codec.is_null() {
            return Err(BatchEnd::Unusable);
        }
        r.ctx = avcodec_alloc_context3(codec);
        if r.ctx.is_null() || avcodec_parameters_to_context(r.ctx, cp) < 0 {
            return Err(BatchEnd::Unusable);
        }
        // One thread, so a batch never takes more than one core from the playback. The loop filter
        // is skipped: a card-sized still does not show it. `showall`: a keyframe that is an I-frame
        // without an IDR or a recovery point (broadcast H.264) is output too.
        let ctx = r.ctx as *mut c_void;
        av_opt_set(ctx, c"threads".as_ptr(), c"1".as_ptr(), 0);
        av_opt_set(ctx, c"skip_loop_filter".as_ptr(), c"all".as_ptr(), 0);
        av_opt_set(ctx, c"flags2".as_ptr(), c"+showall".as_ptr(), 0);
        if avcodec_open2(r.ctx, codec, std::ptr::null_mut()) < 0 {
            return Err(BatchEnd::Unusable);
        }
        let extradata = if (*cp).extradata.is_null() || (*cp).extradata_size <= 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts((*cp).extradata, (*cp).extradata_size as usize)
        };
        if NalInput::of_extradata(extradata) == NalInput::StartCodes {
            r.origin = r.first_ts()?;
        }
        Ok(r)
    }

    fn read_failed(&self) -> BatchEnd {
        if self.stop.is_set() {
            BatchEnd::Stopped
        } else {
            BatchEnd::Refused
        }
    }

    /// The first timestamp of the video stream from where the demuxer stands; 0 when none comes.
    unsafe fn first_ts(&mut self) -> Result<i64, BatchEnd> {
        for _ in 0..KEY_SEARCH_PACKETS {
            let rc = av_read_frame(self.fmt, self.pkt);
            if rc < 0 {
                return if rc == AVERROR_EOF && !self.stop.is_set() { Ok(0) } else { Err(self.read_failed()) };
            }
            let (index, pts, dts) = ((*self.pkt).stream_index, (*self.pkt).pts, (*self.pkt).dts);
            av_packet_unref(self.pkt);
            let t = if pts != AV_NOPTS_VALUE { pts } else { dts };
            if index == self.vi && t != AV_NOPTS_VALUE {
                return Ok(t.max(0));
            }
        }
        Ok(0)
    }

    /// The keyframe at or before `want_ms` (movie time), shrunk. `Ok(None)`: no picture there
    /// (no keyframe within reach, past the end, one the decoder refused).
    pub(crate) unsafe fn grab(&mut self, want_ms: i64) -> Result<Option<Still>, BatchEnd> {
        let ts = av_rescale_q(want_ms, MS_TB, self.tb).saturating_add(self.origin);
        if av_seek_frame(self.fmt, self.vi, ts, AVSEEK_FLAG_BACKWARD) < 0 {
            return if self.stop.is_set() { Err(BatchEnd::Stopped) } else { Ok(None) };
        }
        for _ in 0..KEY_SEARCH_PACKETS {
            let rc = av_read_frame(self.fmt, self.pkt);
            if rc < 0 {
                return if rc == AVERROR_EOF && !self.stop.is_set() { Ok(None) } else { Err(self.read_failed()) };
            }
            let (index, flags, pts, dts) = ((*self.pkt).stream_index, (*self.pkt).flags, (*self.pkt).pts, (*self.pkt).dts);
            if index != self.vi || flags & AV_PKT_FLAG_KEY == 0 {
                av_packet_unref(self.pkt);
                continue;
            }
            let t = if pts != AV_NOPTS_VALUE { pts } else { dts };
            let sent = avcodec_send_packet(self.ctx, self.pkt);
            av_packet_unref(self.pkt);
            // The empty packet drains the decoder, so the keyframe's picture comes out without the
            // pictures that would follow it.
            let decoded = sent >= 0
                && avcodec_send_packet(self.ctx, std::ptr::null()) >= 0
                && avcodec_receive_frame(self.ctx, self.frame) >= 0;
            let still = if decoded && t != AV_NOPTS_VALUE {
                self.still(av_rescale_q(t - self.origin, self.tb, MS_TB).max(0))
            } else {
                Ok(None)
            };
            av_frame_unref(self.frame);
            avcodec_flush_buffers(self.ctx);
            return still;
        }
        Ok(None)
    }

    /// The decoded picture in [`Reader::frame`], shrunk. `Err(Unusable)` for a picture format the
    /// shrink does not read (4:2:2, 4:4:4, 12-bit); `Ok(None)` for planes it cannot trust.
    unsafe fn still(&self, at_ms: i64) -> Result<Option<Still>, BatchEnd> {
        let f = self.frame as *const u8;
        let int = |off: usize| std::ptr::read_unaligned(f.add(off) as *const c_int);
        let format = int(OFF_FRAME_FORMAT);
        let (ten_bit, full) = match format {
            x if x < 0 => return Ok(None),
            x if x == self.fmts.p8 => (false, false),
            x if x == self.fmts.j8 => (false, true),
            x if x == self.fmts.p10 => (true, false),
            _ => return Err(BatchEnd::Unusable),
        };
        let line = |plane: usize| int(OFF_FRAME_LINESIZE + plane * std::mem::size_of::<c_int>());
        let data = |plane: usize| std::ptr::read_unaligned((f.add(OFF_FRAME_DATA) as *const *const u8).add(plane));
        let (w, h, y_stride, c_stride) = (int(OFF_FRAME_WIDTH), int(OFF_FRAME_HEIGHT), line(0), line(1));
        let bytes = if ten_bit { 2 } else { 1 };
        if w < 2
            || h < 2
            || y_stride < w * bytes
            || c_stride < (w + 1) / 2 * bytes
            || line(2) != c_stride
            || (0..3).any(|p| data(p).is_null())
        {
            return Ok(None);
        }
        let (w, h, y_stride, c_stride) = (w as usize, h as usize, y_stride as usize, c_stride as usize);
        let rows = h.div_ceil(2);
        let planes = Planes {
            y: std::slice::from_raw_parts(data(0), y_stride * h),
            u: std::slice::from_raw_parts(data(1), c_stride * rows),
            v: std::slice::from_raw_parts(data(2), c_stride * rows),
            y_stride,
            c_stride,
            w,
            h,
            ten_bit,
        };
        let (dw, dh) = fit(w, h, self.sar);
        let rgba = shrink(&planes, dw, dh, &Matrix::new(h >= 720, full));
        Ok(Some(Still { at_ms, w: dw as i32, h: dh as i32, rgba }))
    }
}

/// The planes of one decoded 4:2:0 picture.
struct Planes<'a> {
    y: &'a [u8],
    u: &'a [u8],
    v: &'a [u8],
    y_stride: usize,
    c_stride: usize,
    w: usize,
    h: usize,
    /// 10-bit samples, two little-endian bytes each.
    ten_bit: bool,
}

impl Planes<'_> {
    /// The 8-bit value of the sample at (`x`, `y`) of `plane`.
    fn sample(&self, plane: &[u8], stride: usize, x: usize, y: usize) -> i32 {
        if self.ten_bit {
            let i = y * stride + 2 * x;
            i32::from(u16::from_le_bytes([plane[i], plane[i + 1]]) >> 2)
        } else {
            i32::from(plane[y * stride + x])
        }
    }
}

/// Y′CbCr to RGB in 1/1024ths: luma gain, luma offset, then Cr→R, Cb→G, Cr→G, Cb→B.
struct Matrix([i32; 6]);

impl Matrix {
    /// BT.709 for an HD picture, BT.601 below it (what an untagged stream means in practice);
    /// limited range unless the decoder says full (`yuvj`).
    fn new(hd: bool, full: bool) -> Matrix {
        let (kr, kb) = if hd { (0.2126, 0.0722) } else { (0.299, 0.114) };
        let kg = 1.0 - kr - kb;
        let (gain, chroma, offset) = if full { (1.0, 1.0, 0) } else { (255.0 / 219.0, 255.0 / 224.0, 16) };
        let q = |x: f64| (x * 1024.0).round() as i32;
        Matrix([
            q(gain),
            offset,
            q(2.0 * (1.0 - kr) * chroma),
            q(-2.0 * kb * (1.0 - kb) / kg * chroma),
            q(-2.0 * kr * (1.0 - kr) / kg * chroma),
            q(2.0 * (1.0 - kb) * chroma),
        ])
    }

    fn rgba(&self, y: i32, u: i32, v: i32) -> [u8; 4] {
        let [gain, offset, rv, gu, gv, bu] = self.0;
        let (c, u, v) = ((y - offset) * gain + 512, u - 128, v - 128);
        let px = |n: i32| (n >> 10).clamp(0, 255) as u8;
        [px(c + rv * v), px(c + gu * u + gv * v), px(c + bu * u), 255]
    }
}

/// The still's size: the picture's display shape (`sar` widens its coded width) inside the card.
fn fit(w: usize, h: usize, sar: f32) -> (usize, usize) {
    let shown_w = w as f32 * sar;
    let scale = (OUT_W as f32 / shown_w).min(OUT_H as f32 / h as f32);
    (((shown_w * scale).round() as usize).clamp(2, OUT_W), ((h as f32 * scale).round() as usize).clamp(2, OUT_H))
}

/// Shrink to `dw`×`dh` RGBA: each output pixel averages a 4×4 grid of samples spread over the
/// source area it covers.
fn shrink(p: &Planes, dw: usize, dh: usize, m: &Matrix) -> Vec<u8> {
    let spread = |n: usize, out: usize| -> Vec<usize> { (0..out * 4).map(|i| (2 * i + 1) * n / (out * 8)).collect() };
    let (xs, ys) = (spread(p.w, dw), spread(p.h, dh));
    let mut rgba = Vec::with_capacity(dw * dh * 4);
    for rows in ys.chunks_exact(4) {
        for cols in xs.chunks_exact(4) {
            let (mut y, mut u, mut v) = (0, 0, 0);
            for &sy in rows {
                for &sx in cols {
                    y += p.sample(p.y, p.y_stride, sx, sy);
                    u += p.sample(p.u, p.c_stride, sx / 2, sy / 2);
                    v += p.sample(p.v, p.c_stride, sx / 2, sy / 2);
                }
            }
            rgba.extend_from_slice(&m.rgba(y / 16, u / 16, v / 16));
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(samples: usize, v: u8, ten_bit: bool) -> Vec<u8> {
        if ten_bit {
            (0..samples).flat_map(|_| (u16::from(v) << 2).to_le_bytes()).collect()
        } else {
            vec![v; samples]
        }
    }

    #[test]
    fn a_picture_shrinks_to_the_card_in_its_display_shape_and_colour() {
        assert_eq!(fit(1920, 800, 1.0), (320, 133));
        assert_eq!(fit(720, 480, 32.0 / 27.0), (320, 180));
        let (w, h) = (64, 36);
        for ten_bit in [false, true] {
            let b = if ten_bit { 2 } else { 1 };
            // BT.709 limited-range red.
            let (y, u, v) = (plane(w * h, 63, ten_bit), plane(w * h / 4, 102, ten_bit), plane(w * h / 4, 240, ten_bit));
            let p = Planes { y: &y, u: &u, v: &v, y_stride: w * b, c_stride: w / 2 * b, w, h, ten_bit };
            let rgba = shrink(&p, 32, 18, &Matrix::new(true, false));
            assert_eq!(rgba.len(), 32 * 18 * 4);
            assert!(rgba.chunks_exact(4).all(|px| px[0] == 255 && px[1] <= 2 && px[2] <= 2 && px[3] == 255), "{:?}", &rgba[..4]);
        }
    }
}
