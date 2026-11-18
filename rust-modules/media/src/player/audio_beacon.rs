//! **The audio beacon: what sound is leaving the app, told to the LumaTV shell.**
//!
//! The shell draws one pill of format marks over a picture (resolution, range, sound). Its sound mark
//! comes from LG's own announcements and from HDMI inputs, so it knows nothing of audio the app
//! decodes or converts itself. The shell's helper listens on loopback for the app to say it:
//!
//! * `POST http://127.0.0.1:18731/v1/media/audio`, JSON, with `X-Luma-Client: com.beb.plxnative`;
//! * `DELETE …/v1/media/audio?app=com.beb.plxnative` when the playback ends.
//!
//! The helper answers 204, or 400 with a reason, and drops an entry 75 s after its last POST.
//!
//! **The fields describe the DELIVERED format, not the file.** The server converting a track
//! (`server_converted`) changes the codec, and the app mixing a surround track down to PCM
//! (`app_stereo_mix`, the track menu's "played in stereo") changes both codec and channel count; only
//! a track played as it is (`source`) keeps the file's codec, profile, Atmos flag and layout. The two
//! conversion facts are the track menu's own ([`crate::route::audio_conversion`]); Atmos is what the
//! Load declares ([`crate::route::stream_immersive`]).
//!
//! **Sans-IO core, thin driver.** [`Core`] decides, from the clock and what is playing, which request
//! is due: a POST at the start and on any change of format or context, one every [`KEEP_ALIVE_MS`]
//! while it stands, and a DELETE when playback ends. [`Beacon`] runs those on a worker, off the frame
//! thread, with short deadlines. A helper that is not there (connection refused) is met with silence
//! and a growing back-off: no user-visible error, and one log line per playback.

use plx_data::metadata::Stream;
use serde_json::json;
use std::sync::{Arc, Condvar, Mutex};

const ENDPOINT: &str = "http://127.0.0.1:18731/v1/media/audio";
/// The app id the contract names, whichever install runs: the shell keys its entry on it.
const APP: &str = plx_base::paths::STABLE_APP_ID;
/// A POST this often while a format stands; the helper forgets an entry after 75 s.
pub const KEEP_ALIVE_MS: u32 = 30_000;
/// The wait after a failed POST, doubling to [`BACKOFF_MAX_MS`]; a success resets it.
const BACKOFF_MIN_MS: u32 = 30_000;
const BACKOFF_MAX_MS: u32 = 240_000;
/// Loopback answers in well under a millisecond; a helper that has stopped answering is not waited for.
const WIRE: plx_net::net::Timeouts =
    plx_net::net::Timeouts { connect_s: 1, total_s: 0, total_ms: 1500, low_speed_bps: 0, low_speed_s: 0 };
/// How often [`Beacon::tick`] looks at what plays.
const ASK_EVERY_MS: u32 = 250;
/// How long the exit clear may hold the process (the loopback answer is immediate).
const EXIT_WAIT_MS: u64 = 400;

/// What happened to the sound on its way out of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivered {
    Source,
    ServerConverted,
    AppStereoMix,
}

impl Delivered {
    fn name(self) -> &'static str {
        match self {
            Delivered::Source => "source",
            Delivered::ServerConverted => "server_converted",
            Delivered::AppStereoMix => "app_stereo_mix",
        }
    }
}

/// The sound leaving the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    /// `ac3|eac3|truehd|dts|aac|pcm|opus|flac|mp3|other`
    pub codec: &'static str,
    /// Free text such as `dts-hd ma`; `None` when the track names none or the sound was converted.
    pub profile: Option<String>,
    pub atmos: bool,
    pub channels: i64,
    pub layout: Option<String>,
    pub delivered: Delivered,
}

/// One format and the pipeline it plays through (`None` while the Load has not named it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub format: Format,
    pub context: Option<String>,
}

impl Want {
    fn body(&self) -> String {
        let f = &self.format;
        json!({
            "app": APP,
            "context": self.context,
            "codec": f.codec,
            "profile": f.profile,
            "atmos": f.atmos,
            "channels": f.channels,
            "layout": f.layout,
            "delivered": f.delivered.name(),
        })
        .to_string()
    }
}

/// The contract's codec family for a PMS audio codec id.
fn family(codec: &str) -> &'static str {
    match codec.to_ascii_lowercase().as_str() {
        "ac3" => "ac3",
        "eac3" | "ec3" | "ec-3" => "eac3",
        "truehd" | "mlp" => "truehd",
        "dts" | "dca" | "dtshd" => "dts",
        "aac" | "aac_latm" | "he-aac" => "aac",
        "opus" => "opus",
        "flac" => "flac",
        "mp3" => "mp3",
        c if c == "lpcm" || c == "pcm" || c.starts_with("pcm_") => "pcm",
        _ => "other",
    }
}

/// The delivered format of track `s`. `fed_codec` is the codec the pipeline is fed
/// ([`crate::route::stream_acodec`]), which differs from the track's only when the server converts it;
/// `immersive` is what the Load declares ([`crate::route::stream_immersive`]).
pub fn format_for(s: &Stream, delivered: Delivered, fed_codec: &str, immersive: bool) -> Format {
    let some = |t: &str| (!t.is_empty()).then(|| t.to_owned());
    match delivered {
        Delivered::AppStereoMix => {
            Format { codec: "pcm", profile: None, atmos: false, channels: 2, layout: Some("stereo".into()), delivered }
        }
        Delivered::ServerConverted => {
            let codec = family(if fed_codec.is_empty() { &s.codec } else { fed_codec });
            // AC-3 carries at most 5.1, whatever the source had
            let channels = if codec == "ac3" { s.channels.min(6) } else { s.channels };
            Format { codec, profile: None, atmos: false, channels, layout: None, delivered }
        }
        Delivered::Source => Format {
            codec: family(&s.codec),
            profile: some(&s.profile),
            atmos: immersive,
            channels: s.channels,
            layout: some(&s.layout),
            delivered,
        },
    }
}

/// What to tell the shell now: the playing track's delivered format while something plays (or waits
/// to), `None` otherwise: idle, resolving, a failed open, a detail-page preview, or a track the route
/// and the item's track list cannot name.
pub fn want(ps: &crate::route::PlaybackSession, audio: &[Stream]) -> Option<Want> {
    use super::shared::PlaybackState as S;
    if !matches!(super::state(ps), S::Buffering | S::Seeking | S::Playing | S::Paused) || crate::route::preview_request(ps) {
        return None;
    }
    let s = &audio[crate::route::playing_audio(ps, audio)?];
    let (converted, in_app) = crate::route::audio_conversion(ps, s);
    let delivered = if converted {
        Delivered::ServerConverted
    } else if in_app {
        Delivered::AppStereoMix
    } else {
        Delivered::Source
    };
    let immersive = delivered == Delivered::Source && crate::route::stream_immersive(ps);
    Some(Want { format: format_for(s, delivered, &crate::route::stream_acodec(ps), immersive), context: super::media_context() })
}

/// One request on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    Post(Want),
    Clear,
}

/// The helper's answer, as far as the app cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// It answered, and refused (400 with a reason, or any other status).
    Refused(String),
    /// Nothing answered: connection refused, or the deadline passed.
    Unreachable,
}

/// Verb, URL, headers and body of `req`. Pure, so the contract is held by tests.
fn parts(req: &Req) -> (&'static str, String, Vec<String>, Option<Vec<u8>>) {
    let client = format!("X-Luma-Client: {APP}");
    match req {
        Req::Post(want) => {
            ("POST", ENDPOINT.to_owned(), vec!["Content-Type: application/json".to_owned(), client], Some(want.body().into_bytes()))
        }
        Req::Clear => ("DELETE", format!("{ENDPOINT}?app={APP}"), vec![client], None),
    }
}

/// The helper's answer (status and body), or `None` when nothing answered.
fn outcome(reply: Option<(u16, &[u8])>) -> Outcome {
    match reply {
        None => Outcome::Unreachable,
        Some((204 | 200, _)) => Outcome::Accepted,
        Some((400, body)) => {
            Outcome::Refused(String::from_utf8_lossy(body).trim().chars().filter(|c| !c.is_control()).take(120).collect())
        }
        Some((status, _)) => Outcome::Refused(format!("status {status}")),
    }
}

/// The transport: one request, answered. The real one is loopback HTTP; a test brings its own.
pub trait Wire: Send + Sync + 'static {
    fn send(&self, req: &Req) -> Outcome;
}

struct Loopback;

impl Wire for Loopback {
    fn send(&self, req: &Req) -> Outcome {
        let (verb, url, headers, body) = parts(req);
        let reply = plx_net::net::request(&url, &headers, verb, body.as_deref(), WIRE, false, Some(512), None);
        outcome(reply.as_ref().map(|r| (r.status, r.body.as_slice())))
    }
}

/// The decisions. No clock reads, no I/O.
#[derive(Debug)]
pub struct Core {
    /// What the helper holds for the app, and when it was told.
    told: Option<(Want, u32)>,
    in_flight: bool,
    /// No request before this, after a failure.
    quiet_until: Option<u32>,
    backoff_ms: u32,
    /// A playback is open, and its failure has been logged.
    open: bool,
    logged: bool,
}

/// `now` has reached `at`, across the clock's wrap.
fn reached(now: u32, at: u32) -> bool {
    now.wrapping_sub(at) as i32 >= 0
}

impl Core {
    pub fn new() -> Self {
        Core { told: None, in_flight: false, quiet_until: None, backoff_ms: BACKOFF_MIN_MS, open: false, logged: false }
    }

    /// The request due at `now` for what plays (`want`), if any. At most one is ever in flight; the
    /// next is decided when its answer is in.
    pub fn step(&mut self, now: u32, want: Option<&Want>) -> Option<Req> {
        if want.is_none() {
            self.open = false;
            self.logged = false;
        } else {
            self.open = true;
        }
        if self.in_flight {
            return None;
        }
        if self.quiet_until.is_some_and(|until| !reached(now, until)) {
            return None;
        }
        self.quiet_until = None;
        match want {
            Some(w) => {
                let due = self.told.as_ref().map_or(true, |(t, at)| t != w || reached(now, at.wrapping_add(KEEP_ALIVE_MS)));
                if !due {
                    return None;
                }
                self.told = Some((w.clone(), now));
                self.in_flight = true;
                Some(Req::Post(w.clone()))
            }
            None if self.told.is_some() => {
                self.told = None;
                self.in_flight = true;
                Some(Req::Clear)
            }
            None => None,
        }
    }

    /// The answer to `req`. Returns a line for the log: the first failure of a playback, nothing else.
    pub fn done(&mut self, now: u32, req: &Req, out: &Outcome) -> Option<String> {
        self.in_flight = false;
        if matches!(req, Req::Clear) {
            return None;
        }
        match out {
            Outcome::Accepted => {
                self.backoff_ms = BACKOFF_MIN_MS;
                None
            }
            failed => {
                self.told = None;
                self.quiet_until = Some(now.wrapping_add(self.backoff_ms));
                self.backoff_ms = (self.backoff_ms * 2).min(BACKOFF_MAX_MS);
                (self.open && !std::mem::replace(&mut self.logged, true)).then(|| match failed {
                    Outcome::Refused(why) => format!("audio beacon: the shell refused the format ({why})"),
                    _ => "audio beacon: the shell's helper is not answering; the format is not shown there".to_owned(),
                })
            }
        }
    }

    /// Whether the helper may still hold an entry for the app (what the exit clear is owed for).
    fn holds_entry(&self) -> bool {
        self.told.is_some()
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

type Mailbox = Arc<Mutex<Option<(Req, Outcome)>>>;
type Spawn = fn(Box<dyn FnOnce() + Send>) -> bool;

fn spawn_worker(work: Box<dyn FnOnce() + Send>) -> bool {
    plx_base::task::spawn_small("audio beacon", work)
}

/// The driver: [`Core`] plus a worker for each request it asks for.
pub struct Beacon {
    core: Core,
    wire: Arc<dyn Wire>,
    spawn: Spawn,
    mailbox: Mailbox,
    /// When `what` was last asked: it builds strings, so it is asked a few times a second, not every frame.
    asked: Option<u32>,
}

impl Beacon {
    pub fn new() -> Self {
        Self::with(Arc::new(Loopback), spawn_worker)
    }

    /// A beacon over another wire and spawner: the seam the tests drive with a fake of each.
    pub fn with(wire: Arc<dyn Wire>, spawn: Spawn) -> Self {
        Beacon { core: Core::new(), wire, spawn, mailbox: Arc::new(Mutex::new(None)), asked: None }
    }

    /// Once a frame, from the main loop. Never blocks: the request runs on a worker and its answer is
    /// read here on a later frame. `what` says what plays ([`want`]); it is asked every
    /// [`ASK_EVERY_MS`], which is also how late a change or an end can be noticed.
    pub fn tick(&mut self, now: u32, what: impl FnOnce() -> Option<Want>) {
        let answered = self.mailbox.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((req, out)) = answered {
            if let Some(line) = self.core.done(now, &req, &out) {
                super::log(&line);
            }
        }
        if self.asked.is_some_and(|at| !reached(now, at.wrapping_add(ASK_EVERY_MS))) {
            return;
        }
        self.asked = Some(now);
        let Some(req) = self.core.step(now, what().as_ref()) else { return };
        let (wire, mailbox, sent) = (Arc::clone(&self.wire), Arc::clone(&self.mailbox), req.clone());
        let spawned = (self.spawn)(Box::new(move || {
            let out = wire.send(&sent);
            *mailbox.lock().unwrap_or_else(|e| e.into_inner()) = Some((sent, out));
        }));
        if !spawned {
            if let Some(line) = self.core.done(now, &req, &Outcome::Unreachable) {
                super::log(&line);
            }
        }
    }

    /// At exit: clear the helper's entry now rather than leave it to expire. Bounded, since the
    /// process is going and the helper may not be there.
    pub fn clear_for_exit(&mut self) {
        if !self.core.holds_entry() {
            return;
        }
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let (wire, signal) = (Arc::clone(&self.wire), Arc::clone(&done));
        let spawned = (self.spawn)(Box::new(move || {
            wire.send(&Req::Clear);
            *signal.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            signal.1.notify_all();
        }));
        if spawned {
            let held = done.0.lock().unwrap_or_else(|e| e.into_inner());
            let _ = done.1.wait_timeout_while(held, std::time::Duration::from_millis(EXIT_WAIT_MS), |finished| !*finished);
        }
    }
}

impl Default for Beacon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(codec: &str, channels: i64, profile: &str, layout: &str) -> Stream {
        Stream { codec: codec.into(), channels, profile: profile.into(), layout: layout.into(), ..Default::default() }
    }

    fn want(codec: &'static str) -> Want {
        Want {
            format: Format { codec, profile: None, atmos: false, channels: 6, layout: None, delivered: Delivered::Source },
            context: Some("_L3SB1p9Zxvl5rm".into()),
        }
    }

    #[test]
    fn a_source_track_keeps_the_files_own_facts() {
        let s = stream("eac3", 6, "dolby digital plus + dolby atmos", "5.1(side)");
        let f = format_for(&s, Delivered::Source, "eac3", true);
        assert_eq!(
            f,
            Format {
                codec: "eac3",
                profile: Some("dolby digital plus + dolby atmos".into()),
                atmos: true,
                channels: 6,
                layout: Some("5.1(side)".into()),
                delivered: Delivered::Source,
            }
        );
        let dts = format_for(&stream("dca", 8, "dts-hd ma", ""), Delivered::Source, "dca", false);
        assert_eq!((dts.codec, dts.profile.as_deref(), dts.atmos, dts.layout), ("dts", Some("dts-hd ma"), false, None));
    }

    #[test]
    fn a_converted_track_is_named_by_what_the_server_sends() {
        let s = stream("dca", 8, "dts-hd ma", "7.1");
        let f = format_for(&s, Delivered::ServerConverted, "ac3", true);
        assert_eq!((f.codec, f.profile, f.atmos, f.channels, f.layout), ("ac3", None, false, 6, None), "AC-3 stops at 5.1; no Atmos survives");
        assert_eq!(format_for(&s, Delivered::ServerConverted, "", false).codec, "dts", "no fed codec yet: the track's own");
    }

    #[test]
    fn a_mixed_down_track_leaves_as_stereo_pcm() {
        let s = stream("dca", 6, "dts", "5.1");
        let f = format_for(&s, Delivered::AppStereoMix, "dca", false);
        assert_eq!((f.codec, f.channels, f.atmos, f.profile.as_deref(), f.delivered), ("pcm", 2, false, None, Delivered::AppStereoMix));
    }

    #[test]
    fn codec_ids_fall_into_the_contracts_families() {
        let cases = [
            ("AC3", "ac3"), ("eac3", "eac3"), ("truehd", "truehd"), ("dca", "dts"), ("dts", "dts"), ("aac", "aac"),
            ("pcm_s24le", "pcm"), ("lpcm", "pcm"), ("opus", "opus"), ("flac", "flac"), ("mp3", "mp3"), ("vorbis", "other"), ("alac", "other"),
        ];
        for (id, want) in cases {
            assert_eq!(family(id), want, "{id}");
        }
    }

    #[test]
    fn the_request_is_the_contracts_json() {
        let mut w = want("eac3");
        w.format.atmos = true;
        w.format.layout = Some("5.1(side)".into());
        let (verb, url, headers, body) = parts(&Req::Post(w));
        assert_eq!((verb, url.as_str()), ("POST", "http://127.0.0.1:18731/v1/media/audio"));
        assert_eq!(headers, ["Content-Type: application/json", "X-Luma-Client: com.beb.plxnative"]);
        let v: serde_json::Value = serde_json::from_slice(&body.unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"app":"com.beb.plxnative","context":"_L3SB1p9Zxvl5rm","codec":"eac3","profile":null,"atmos":true,"channels":6,"layout":"5.1(side)","delivered":"source"})
        );
        let mut unknown = want("aac");
        unknown.context = None;
        assert!(parts(&Req::Post(unknown)).3.is_some_and(|b| String::from_utf8_lossy(&b).contains("\"context\":null")));
        let (verb, url, headers, body) = parts(&Req::Clear);
        assert_eq!((verb, url.as_str(), body), ("DELETE", "http://127.0.0.1:18731/v1/media/audio?app=com.beb.plxnative", None));
        assert_eq!(headers, ["X-Luma-Client: com.beb.plxnative"]);
    }

    #[test]
    fn answers_map_to_outcomes() {
        assert_eq!(outcome(Some((204, b""))), Outcome::Accepted);
        assert_eq!(outcome(Some((400, b" bad codec\n"))), Outcome::Refused("bad codec".into()));
        assert_eq!(outcome(Some((500, b""))), Outcome::Refused("status 500".into()));
        assert_eq!(outcome(None), Outcome::Unreachable);
    }

    /// Sends at the start, keeps alive every 30 s, resends on a change, clears at the end.
    #[test]
    fn it_posts_keeps_alive_follows_changes_and_clears() {
        let mut c = Core::new();
        let a = want("eac3");
        let b = want("truehd");
        assert_eq!(c.step(0, None), None, "nothing plays, nothing is said");
        assert_eq!(c.step(100, Some(&a)), Some(Req::Post(a.clone())), "at the start");
        assert_eq!(c.step(101, Some(&a)), None, "one request at a time");
        c.done(150, &Req::Post(a.clone()), &Outcome::Accepted);
        assert_eq!(c.step(10_000, Some(&a)), None, "a format that stands is not repeated");
        assert_eq!(c.step(29_999, Some(&a)), None);
        assert_eq!(c.step(30_100, Some(&a)), Some(Req::Post(a.clone())), "30 s after the last POST");
        c.done(30_110, &Req::Post(a.clone()), &Outcome::Accepted);
        assert_eq!(c.step(31_000, Some(&b)), Some(Req::Post(b.clone())), "a track change goes at once");
        c.done(31_010, &Req::Post(b.clone()), &Outcome::Accepted);
        let mut other_pipeline = b.clone();
        other_pipeline.context = Some("_other".into());
        assert_eq!(c.step(32_000, Some(&other_pipeline)), Some(Req::Post(other_pipeline.clone())), "so does a new context");
        c.done(32_010, &Req::Post(other_pipeline), &Outcome::Accepted);
        assert_eq!(c.step(33_000, None), Some(Req::Clear), "playback ended");
        c.done(33_010, &Req::Clear, &Outcome::Accepted);
        assert_eq!(c.step(34_000, None), None, "and it is cleared once");
    }

    #[test]
    fn a_clock_wrap_does_not_stop_the_keep_alive() {
        let mut c = Core::new();
        let a = want("ac3");
        let late = u32::MAX - 1_000;
        assert!(c.step(late, Some(&a)).is_some());
        c.done(late, &Req::Post(a.clone()), &Outcome::Accepted);
        assert_eq!(c.step(late.wrapping_add(29_000), Some(&a)), None);
        assert!(c.step(late.wrapping_add(30_000), Some(&a)).is_some());
    }

    #[test]
    fn a_missing_helper_gets_silence_a_growing_wait_and_one_log_line() {
        let mut c = Core::new();
        let a = want("eac3");
        let post = Req::Post(a.clone());
        assert!(c.step(0, Some(&a)).is_some());
        let line = c.done(5, &post, &Outcome::Unreachable);
        assert!(line.is_some_and(|l| l.contains("not answering")));
        assert_eq!(c.step(20_000, Some(&a)), None, "quiet for 30 s");
        assert!(c.step(30_005, Some(&a)).is_some());
        assert_eq!(c.done(30_010, &post, &Outcome::Unreachable), None, "the second failure of a playback is not logged");
        assert_eq!(c.step(80_000, Some(&a)), None, "the wait has doubled to 60 s");
        assert!(c.step(90_010, Some(&a)).is_some());
        c.done(90_020, &post, &Outcome::Accepted);
        assert_eq!(c.step(120_100, None), Some(Req::Clear), "the entry exists once a POST has landed");
        c.done(120_110, &Req::Clear, &Outcome::Accepted);
        // a failed POST leaves nothing to clear
        assert!(c.step(130_000, Some(&a)).is_some());
        c.done(130_010, &post, &Outcome::Unreachable);
        assert_eq!(c.step(130_020, None), None);
        // the next playback logs again
        assert!(c.step(300_000, Some(&a)).is_some());
        assert!(c.done(300_010, &post, &Outcome::Refused("bad".into())).is_some_and(|l| l.contains("refused")));
    }

    struct Fake(Mutex<Vec<Req>>, Outcome);
    impl Wire for Fake {
        fn send(&self, req: &Req) -> Outcome {
            self.0.lock().unwrap().push(req.clone());
            self.1.clone()
        }
    }
    fn inline(work: Box<dyn FnOnce() + Send>) -> bool {
        work();
        true
    }

    /// The driver over a fake wire and a fake clock: nothing waits.
    #[test]
    fn the_driver_sends_what_the_core_asks_for_and_clears_at_exit() {
        let wire = Arc::new(Fake(Mutex::new(Vec::new()), Outcome::Accepted));
        let mut b = Beacon::with(wire.clone(), inline);
        let a = want("dts");
        b.tick(0, || Some(a.clone()));
        b.tick(10, || unreachable!("asked again within 250 ms"));
        b.tick(300, || Some(a.clone()));
        b.tick(30_000, || Some(a.clone()));
        b.tick(30_300, || None);
        b.tick(30_600, || None);
        assert_eq!(*wire.0.lock().unwrap(), [Req::Post(a.clone()), Req::Post(a.clone()), Req::Clear]);
        b.tick(40_000, || Some(a.clone()));
        b.clear_for_exit();
        assert_eq!(wire.0.lock().unwrap().last(), Some(&Req::Clear));
        let before = wire.0.lock().unwrap().len();
        let mut idle = Beacon::with(wire.clone(), inline);
        idle.clear_for_exit();
        assert_eq!(wire.0.lock().unwrap().len(), before, "nothing told, nothing to clear");
    }
}
