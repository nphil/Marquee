//! Live tuner tunes: how long a channel change takes, and how little the demuxer probes for one.
//!
//! A tuner source (an HDHomeRun-style `/auto/v<channel>` URL on a LumaTV tuner) is MPEG-TS with
//! known codecs and a PMT in the first few packets. FFmpeg's default probe reads up to 5 MB or 5 s
//! of it, and keeps going while any stream (an EPG or teletext PID that never yields parameters)
//! is unfinished, so a tune waited for the probe limit rather than for the first picture.
//! [`probe_limits`] caps it for those URLs only; a film is probed as before.
//!
//! [`begin`] stamps the tap, the demuxer reports its probe and first keyframe, and the first
//! presented frame completes the one `livetune:` line the TV's log carries per tune.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 1 MiB: past the PMT and the first I-frame of a 19 Mbit/s ATSC feed (~0.45 s of it).
pub const PROBE_SIZE_BYTES: i64 = 1 << 20;
/// 500 ms of stream time, enough for the first AC-3 frame and one GOP header.
pub const ANALYZE_DURATION_US: i64 = 500_000;

/// What the demuxer may read before it must settle the stream list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeLimits {
    pub probesize: i64,
    pub analyze_duration_us: i64,
}

/// `Some` for a tuner URL's path (and query), `None` for everything else.
pub fn probe_limits(path: &str) -> Option<ProbeLimits> {
    is_tuner_path(path).then_some(ProbeLimits { probesize: PROBE_SIZE_BYTES, analyze_duration_us: ANALYZE_DURATION_US })
}

/// The HDHomeRun lineup URL shape: `/auto/v<channel>` (`/auto/v7.1`, `/auto/v12`) or a named
/// tuner's `/tuner<N>/v<channel>`, with or without a query (`?transcode=mobile`).
pub fn is_tuner_path(path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let Some(rest) = path.strip_prefix("/auto/v").or_else(|| {
        let n = path.strip_prefix("/tuner")?;
        let digits = n.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0).then(|| n[digits..].strip_prefix("/v")).flatten()
    }) else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// The path of an `http://host:port/path?query` URL, or `""`.
fn path_of(url: &str) -> &str {
    let after = url.split_once("://").map_or(url, |(_, rest)| rest);
    after.find('/').map_or("", |i| &after[i..])
}

struct Tune {
    tap: Instant,
    probe_ms: Option<u64>,
    key_ms: Option<u64>,
}

static ARMED: AtomicBool = AtomicBool::new(false);
static TUNE: Mutex<Option<Tune>> = Mutex::new(None);

fn tune() -> std::sync::MutexGuard<'static, Option<Tune>> {
    TUNE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A playback of `url` starts now: a tuner URL arms the timer, anything else disarms it.
pub fn begin(url: &str) {
    let live = is_tuner_path(path_of(url));
    *tune() = live.then(|| Tune { tap: Instant::now(), probe_ms: None, key_ms: None });
    ARMED.store(live, Ordering::Release);
}

/// The demuxer's stream probe took `took`.
pub fn note_probe(took: Duration) {
    if ARMED.load(Ordering::Acquire) {
        if let Some(t) = tune().as_mut() {
            t.probe_ms = Some(took.as_millis() as u64);
        }
    }
}

/// A video keyframe reached the lane: only the first counts.
pub fn note_keyframe() {
    if ARMED.load(Ordering::Acquire) {
        if let Some(t) = tune().as_mut() {
            t.key_ms.get_or_insert_with(|| t.tap.elapsed().as_millis() as u64);
        }
    }
}

/// The first presented frame of an armed tune: the log line, once; `None` otherwise.
pub fn first_frame() -> Option<String> {
    if !ARMED.swap(false, Ordering::AcqRel) {
        return None;
    }
    let t = tune().take()?;
    Some(line(t.tap.elapsed().as_millis() as u64, t.probe_ms, t.key_ms))
}

fn line(total_ms: u64, probe_ms: Option<u64>, key_ms: Option<u64>) -> String {
    let ms = |v: Option<u64>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
    format!("livetune: tap→first frame {total_ms} ms (probe {} ms, first keyframe {} ms)", ms(probe_ms), ms(key_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuner_paths_are_the_lineup_shapes_only() {
        for p in ["/auto/v5", "/auto/v7.1", "/auto/v12?transcode=mobile", "/tuner0/v7.1", "/tuner12/v3?x=1"] {
            assert!(is_tuner_path(p), "{p}");
        }
        for p in ["", "/auto/v", "/auto/vx", "/auto/v5/extra", "/tuner/v5", "/tunerX/v5", "/library/parts/9/file.mkv", "/video/:/transcode/universal/start.mkv"] {
            assert!(!is_tuner_path(p), "{p}");
        }
    }

    #[test]
    fn a_film_keeps_ffmpegs_default_probe() {
        assert_eq!(probe_limits("/library/parts/9/file.mkv"), None);
        let l = probe_limits("/auto/v5").expect("tuner");
        assert_eq!((l.probesize, l.analyze_duration_us), (1 << 20, 500_000));
    }

    #[test]
    fn the_url_path_is_found_past_host_and_port() {
        assert_eq!(path_of("http://192.0.2.24:5004/auto/v7.1?transcode=mobile"), "/auto/v7.1?transcode=mobile");
        assert_eq!(path_of("http://host"), "");
    }

    #[test]
    fn one_line_per_tune_names_the_parts() {
        assert_eq!(
            line(1210, Some(180), Some(940)),
            "livetune: tap→first frame 1210 ms (probe 180 ms, first keyframe 940 ms)"
        );
        assert_eq!(line(500, None, None), "livetune: tap→first frame 500 ms (probe ? ms, first keyframe ? ms)");
    }
}
