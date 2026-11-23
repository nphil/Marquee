//! The LumaTV helper's audio pass-through contract, version 1: loopback JSON on port 18731.
//!
//! The `audio.passthrough` topic says whether a client may start the carrier and how the carrier
//! is shaped ([`Carrier`]). `arm` hands the helper this process's audio renderer (its bypass tail
//! and the decoder input, ADEC, LG granted it), `route` says whether LG's mixer is bypassed for it
//! right now, and `release` gives the route back. A missing topic, a refusal or no answer all read
//! as not ready: the title plays on the normal path.
//!
//! The route is the helper's whole side: once the bypass has held for 3 s it connects the granted
//! ADEC to the main output (`audiooutput` `audio/connect`) and unmutes it, so the carrier is
//! audible about 6.7 s after `bypassing`. Release disconnects it and puts the TV back; the client
//! calls it only once its sink is closed, so the exact unmute is safe. This side never calls
//! `audiooutput` itself and never reads the sound card's controls: the route answer carries what
//! the controls would say.
use serde_json::Value;

use super::mat::ByteOrder;

const STATE_URL: &str = "http://127.0.0.1:18731/v1/state/audio.passthrough";
const ARM_URL: &str = "http://127.0.0.1:18731/v1/audio-passthrough/arm";
const ROUTE_URL: &str = "http://127.0.0.1:18731/v1/audio-passthrough/route";
const RELEASE_URL: &str = "http://127.0.0.1:18731/v1/audio-passthrough/release";
const TOPIC: &str = "audio.passthrough";

/// The topic and the route answer from a snapshot in well under a millisecond; a helper that has
/// stopped answering is not waited for.
const WIRE: plx_net::net::Timeouts =
    plx_net::net::Timeouts { connect_s: 1, total_s: 0, total_ms: 250, low_speed_bps: 0, low_speed_s: 0 };
/// An arm waits for LG's renderer to join the audio graph (2 s) and a converter to appear (2 s);
/// a release waits up to 2 s for the sink to close and then reads the TV back.
const CALL: plx_net::net::Timeouts =
    plx_net::net::Timeouts { connect_s: 1, total_s: 0, total_ms: 6000, low_speed_bps: 0, low_speed_s: 0 };
/// Every answer is a few hundred bytes.
const ANSWER_BYTES: usize = 16 << 10;

/// The caps dts_restore_plus re-negotiates LG's audio sink to for the carrier (its `lgroute.c`).
pub const UPSTREAM_SINK_CAPS: &str = "audio/x-raw,format=S16LE,layout=interleaved,rate=192000,channels=8,\
channel-mask=(bitmask)0xc3f,acmod=(uint)0,es=(boolean)true,codec-str=(string)mat,clock-x4=(boolean)true,\
data-rate=(int)192000,data-channels=(int)8,data-bps=(int)16";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The plugin is switched off.
    Off,
    /// The firmware is not one the helper knows; never ready.
    Refused,
    /// The output is not eARC pass-through, or eARC is off.
    NotReady,
    /// A client may arm.
    Ready,
    /// A session holds the route.
    Armed,
    /// The route is gone and LG's mix setting is back: the client stays silent and falls back.
    Lost,
}

/// How the carrier is shaped and how LG's audio sink is told about it. Which form gets the
/// receiver to lock is a property of the firmware, so the helper names it and this side only
/// follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carrier {
    /// `byteOrder`: `"be"` (the default) or `"le"`.
    pub order: ByteOrder,
    /// `sinkCaps`: caps the sink is re-negotiated to when its caps arrive; `null` leaves LG's own.
    pub sink_caps: Option<String>,
    /// `startAsBitstream`: upstream's `pcm-multi-ch` order, on with the caps, off for the first
    /// buffer until the route is armed (3 s at most), on from the second, and no ES flag on any
    /// buffer. Off leaves the property and the flags alone.
    pub start_as_bitstream: bool,
}

impl Default for Carrier {
    fn default() -> Carrier {
        Carrier { order: ByteOrder::Big, sink_caps: Some(UPSTREAM_SINK_CAPS.to_owned()), start_as_bitstream: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topic {
    pub state: State,
    pub ready: bool,
    /// The process holding the route, while armed.
    pub session_pid: Option<u32>,
    /// The decoder input the session holds, while armed.
    pub session_adec: Option<u8>,
    pub reason: String,
    /// What the TV read like once the last session was put back, when it differs from what that
    /// session found; `None` when it matches.
    pub after: Option<String>,
    pub carrier: Carrier,
    /// How long the helper says this reading holds, from when it was taken; 0 when it names none.
    pub valid_for_ms: u64,
}

impl Topic {
    /// Process `pid` may start the carrier: the route is free and ready, or already this process's.
    pub fn admits(&self, pid: u32) -> bool {
        match self.state {
            State::Ready => self.ready,
            State::Armed => self.session_pid == Some(pid),
            _ => false,
        }
    }
}

/// The helper's answer to an arm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Armed {
    pub armed: bool,
    pub reason: String,
}

/// The route as the helper last sampled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub armed: bool,
    pub bypassing: bool,
    pub mixed: bool,
    /// The granted decoder is connected to the main output.
    pub connected: bool,
    /// The receiver is being sent the stream: about 6.7 s after `bypassing`.
    pub audible: bool,
    /// How old the helper's sample was when it answered.
    pub age_ms: u64,
}

/// The topic's value, bare or inside the helper's `{"ok","topic","value"}` envelope.
pub fn parse_topic(body: &[u8]) -> Option<Topic> {
    let root: Value = serde_json::from_slice(body).ok()?;
    let v = match root.get("value") {
        Some(value) if value.is_object() => {
            if root.get("topic").and_then(Value::as_str).is_some_and(|t| t != TOPIC) {
                return None;
            }
            value
        }
        _ => &root,
    };
    if v.get("schema").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let state = match v.get("state")?.as_str()? {
        "off" => State::Off,
        "refused" => State::Refused,
        "not-ready" => State::NotReady,
        "ready" => State::Ready,
        "armed" => State::Armed,
        "lost" => State::Lost,
        _ => return None,
    };
    let named = v.get("carrier");
    let mut carrier = Carrier::default();
    match named.and_then(|c| c.get("byteOrder")) {
        None => {}
        Some(order) => {
            carrier.order = match order.as_str()? {
                "be" => ByteOrder::Big,
                "le" => ByteOrder::Little,
                _ => return None,
            }
        }
    }
    match named.and_then(|c| c.get("sinkCaps")) {
        None => {}
        Some(Value::Null) => carrier.sink_caps = None,
        Some(caps) => carrier.sink_caps = Some(caps.as_str()?.to_owned()),
    }
    if let Some(start) = named.and_then(|c| c.get("startAsBitstream")) {
        carrier.start_as_bitstream = start.as_bool()?;
    }
    let session = |key: &str| v.get("session").and_then(|s| s.get(key)).and_then(Value::as_u64);
    Some(Topic {
        state,
        ready: v.get("ready").and_then(Value::as_bool).unwrap_or(false),
        session_pid: session("pid").and_then(|p| u32::try_from(p).ok()),
        session_adec: session("adec").and_then(|a| u8::try_from(a).ok()),
        reason: text(v, "reason"),
        after: Some(text(v, "after")).filter(|note| !note.is_empty()),
        carrier,
        valid_for_ms: v.get("validForMs").and_then(Value::as_u64).unwrap_or(0),
    })
}

pub fn parse_arm(body: &[u8]) -> Option<Armed> {
    let v: Value = serde_json::from_slice(body).ok()?;
    Some(Armed { armed: v.get("armed")?.as_bool()?, reason: text(&v, "reason") })
}

pub fn parse_route(body: &[u8]) -> Option<Route> {
    let v: Value = serde_json::from_slice(body).ok()?;
    Some(Route {
        armed: v.get("armed")?.as_bool()?,
        bypassing: v.get("bypassing")?.as_bool()?,
        mixed: v.get("mixed").and_then(Value::as_bool).unwrap_or(false),
        connected: v.get("connected").and_then(Value::as_bool).unwrap_or(false),
        audible: v.get("audible").and_then(Value::as_bool).unwrap_or(false),
        age_ms: v.get("ageMs")?.as_u64()?,
    })
}

/// The route's phase and reason as one line, for the log when it changes.
pub fn route_note(body: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return String::new() };
    let phase = v.get("phase").and_then(Value::as_str).unwrap_or("no session");
    let reason = text(&v, "reason");
    let sep = if reason.is_empty() { "" } else { ": " };
    format!("route {phase}{sep}{reason}")
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

pub fn arm_body(pid: u32, tail: u32, adec: u8) -> String {
    format!(r#"{{"pid":{pid},"tail":"{tail:08x}","adec":{adec}}}"#)
}

pub fn release_body(pid: u32) -> String {
    format!(r#"{{"pid":{pid}}}"#)
}

/// The helper's calls. The session and the readiness watch run against [`Loopback`]; tests run
/// them against a fake.
pub trait Helper: Send + Sync {
    fn topic(&self) -> Option<Topic>;
    /// `adec` is the decoder input LG granted the renderer whose bypass tail is `tail`.
    fn arm(&self, pid: u32, tail: u32, adec: u8) -> Option<Armed>;
    /// The route and its log line.
    fn route(&self) -> Option<(Route, String)>;
    /// Synchronous: the TV is put back before it answers. The sink must be closed first.
    fn release(&self, pid: u32);
}

/// The helper on this television.
pub struct Loopback;

impl Loopback {
    fn call(url: &str, body: Option<&str>, wire: plx_net::net::Timeouts) -> Option<Vec<u8>> {
        let mut headers = vec!["X-Luma-Client: com.beb.plxnative".to_owned()];
        if body.is_some() {
            headers.push("Content-Type: application/json".to_owned());
        }
        let verb = if body.is_some() { "POST" } else { "GET" };
        let reply = plx_net::net::request(url, &headers, verb, body.map(str::as_bytes), wire, false, Some(ANSWER_BYTES), None)?;
        (reply.status == 200).then_some(reply.body)
    }
}

impl Helper for Loopback {
    fn topic(&self) -> Option<Topic> {
        parse_topic(&Self::call(STATE_URL, None, WIRE)?)
    }

    fn arm(&self, pid: u32, tail: u32, adec: u8) -> Option<Armed> {
        parse_arm(&Self::call(ARM_URL, Some(&arm_body(pid, tail, adec)), CALL)?)
    }

    fn route(&self) -> Option<(Route, String)> {
        let body = Self::call(ROUTE_URL, None, WIRE)?;
        Some((parse_route(&body)?, route_note(&body)))
    }

    fn release(&self, pid: u32) {
        Self::call(RELEASE_URL, Some(&release_body(pid)), CALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topic_admits_a_ready_route_or_this_process_s_own() {
        let ready = br#"{"ok":true,"topic":"audio.passthrough","api":1,"value":{"schema":1,"enabled":true,
            "state":"ready","ready":true,"reason":"","carrier":{"format":"S16LE","byteOrder":"be","rate":192000,
            "channels":8,"burstBytes":61440,"burstMs":20},"session":null,"after":null}}"#;
        let t = parse_topic(ready).unwrap();
        assert!(t.admits(7));
        assert_eq!(t.carrier, Carrier::default(), "the helper's carrier is big-endian with upstream's caps and toggle");
        assert_eq!((t.session_pid, t.session_adec, t.after), (None, None, None));

        let armed = br#"{"schema":1,"state":"armed","ready":false,"session":{"pid":7,"adec":3,"tail":"0000002a"},
            "after":"the decoder's line differs"}"#;
        let t = parse_topic(armed).unwrap();
        assert!(t.admits(7) && !t.admits(8));
        assert_eq!((t.session_pid, t.session_adec), (Some(7), Some(3)));
        assert_eq!(t.after.as_deref(), Some("the decoder's line differs"));

        for state in ["off", "refused", "not-ready", "lost"] {
            let body = format!(r#"{{"schema":1,"state":"{state}","ready":true}}"#);
            assert!(!parse_topic(body.as_bytes()).unwrap().admits(7), "{state}");
        }
        assert_eq!(parse_topic(br#"{"schema":2,"state":"ready","ready":true}"#), None);
        assert_eq!(parse_topic(br#"{"ok":true,"topic":"network.multi-link","value":{"schema":1,"state":"ready"}}"#), None);
        assert_eq!(parse_topic(b"not json"), None);
    }

    #[test]
    fn the_carrier_s_shape_is_the_helper_s_to_name() {
        let named = br#"{"schema":1,"state":"ready","ready":true,
            "carrier":{"byteOrder":"le","sinkCaps":"audio/x-raw,rate=48000,clock-x4=(boolean)true","startAsBitstream":false}}"#;
        let carrier = parse_topic(named).unwrap().carrier;
        assert_eq!(carrier.order, ByteOrder::Little);
        assert_eq!(carrier.sink_caps.as_deref(), Some("audio/x-raw,rate=48000,clock-x4=(boolean)true"));
        assert!(!carrier.start_as_bitstream);
        let unchanged = br#"{"schema":1,"state":"ready","ready":true,"carrier":{"sinkCaps":null}}"#;
        let carrier = parse_topic(unchanged).unwrap().carrier;
        assert_eq!(carrier.sink_caps, None, "null keeps LG's caps");
        assert!(carrier.start_as_bitstream && carrier.order == ByteOrder::Big);
        assert_eq!(parse_topic(br#"{"schema":1,"state":"ready","carrier":{"sinkCaps":5}}"#), None);
        assert_eq!(parse_topic(br#"{"schema":1,"state":"ready","carrier":{"byteOrder":"middle"}}"#), None);
    }

    #[test]
    fn arm_and_route_answers() {
        assert_eq!(arm_body(1234, 0x2a, 1), r#"{"pid":1234,"tail":"0000002a","adec":1}"#);
        assert_eq!(parse_arm(br#"{"armed":false,"converter":false,"reason":"no converter"}"#).unwrap().reason, "no converter");
        let body = br#"{"armed":true,"phase":"connected","bypassing":true,"mixed":false,"converterPresent":true,
            "connected":true,"audible":false,"reason":"ok","ageMs":40}"#;
        let route = Route { armed: true, bypassing: true, mixed: false, connected: true, audible: false, age_ms: 40 };
        assert_eq!(parse_route(body), Some(route));
        assert_eq!(route_note(body), "route connected: ok");
        assert_eq!(route_note(br#"{"armed":false,"phase":null,"reason":""}"#), "route no session");
        assert_eq!(parse_route(br#"{"armed":true,"bypassing":true}"#), None, "an answer without its age is not one");
    }
}
