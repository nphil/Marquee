//! What the television's sound chain can hand an AV receiver untouched: the sound output, the
//! digital-out mode, and what the receiver says over eARC it can decode. Two read-only platform
//! calls (`settingsservice/getSystemSettings` for `soundOutput`, `soundOutputDigital`;
//! `arccontroller/getARCState` for the link and the receiver's capability list); the bus calls are
//! the port's (`webos::audio_chain`), this is the vocabulary, the parse and the one door.
//!
//! Measured on the OLED65C4 (webOS 10.2.1) with a Denon AVR-S760H: `external_arc`, `passThrough`,
//! `arcActivation: "eARC"`, and `earcCapability` PCM, DD, DTS, DD+, MAT2PCM, TrueHD, DTSX_P1,
//! DTSHD_MA, DTSHD. What the Load can then send is a separate question: [`Chain::passes_dts_hd`]
//! for a DTS-HD stream fed whole, [`Chain::passes_truehd`] for TrueHD in the MAT carrier (the
//! pipeline itself has no TrueHD or MAT input).
//!
//! **Blocking.** One or two bus round trips, so [`chain`] runs on a worker, never the frame
//! thread (it asserts that). The answer is kept for [`FRESH`], so a plan and the Load that follows
//! it read the bus once.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the bus answered: the sound settings and the ARC state, raw.
#[cfg_attr(any(feature = "hostsim", test, feature = "test-support"), allow(dead_code))] // Reply/Unavailable: TV arm only
pub enum Reply { NoPlatform, Unavailable, Reply { settings: String, arc: String } }

/// How the television reaches the receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Link { #[default] None, Arc, Earc }

/// The sound chain as the platform reported it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Chain {
    /// `soundOutput`: `external_arc` when an HDMI ARC/eARC receiver is the output.
    pub output: String,
    /// `soundOutputDigital` is `passThrough` (bitstream), not `pcm`.
    pub pass_through: bool,
    pub link: Link,
    /// `earcCapability`, as the platform spells the entries (`DTSHD_MA`, `TrueHD`, ...).
    pub capability: Vec<String>,
}

impl Chain {
    fn advertises(&self, name: &str) -> bool {
        self.capability.iter().any(|c| c.eq_ignore_ascii_case(name))
    }

    /// The receiver is on eARC and takes the bitstream untouched.
    fn bypasses(&self) -> bool {
        self.output == "external_arc" && self.pass_through && self.link == Link::Earc
    }

    /// The receiver is on eARC, takes the bitstream untouched and says it decodes DTS-HD: a DTS-HD
    /// stream fed whole reaches it as DTS-HD (the stock Plex app does the same on this set).
    pub fn passes_dts_hd(&self) -> bool {
        self.bypasses() && (self.advertises("DTSHD_MA") || self.advertises("DTSHD"))
    }

    /// The same chain with a receiver that decodes TrueHD: what the MAT carrier needs past LG's
    /// mixer.
    pub fn passes_truehd(&self) -> bool {
        self.bypasses() && self.advertises("TrueHD")
    }
}

/// The reduced reply. PURE; `None` when either answer is not a successful platform reply.
pub fn parse(settings: &str, arc: &str) -> Option<Chain> {
    let s: serde_json::Value = serde_json::from_str(settings).ok()?;
    let a: serde_json::Value = serde_json::from_str(arc).ok()?;
    if s["returnValue"].as_bool() != Some(true) {
        return None;
    }
    let output = s["settings"]["soundOutput"].as_str().unwrap_or_default().to_string();
    let pass_through = s["settings"]["soundOutputDigital"].as_str() == Some("passThrough");
    // A refused ARC call is a link with nothing known about it, not a failed read.
    let (link, capability) = if a["returnValue"].as_bool() == Some(true) {
        let link = match a["arcActivation"].as_str() {
            Some("eARC") => Link::Earc,
            Some("ARC") => Link::Arc,
            _ => Link::None,
        };
        let caps = a["earcCapability"]
            .as_array()
            .map(|l| l.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        (link, caps)
    } else {
        (Link::None, Vec::new())
    };
    Some(Chain { output, pass_through, link, capability })
}

/// How long a read stays good. A plan and the Load it leads to are seconds apart; the user changing
/// the TV's output mid-title is picked up by the next play.
const FRESH: Duration = Duration::from_secs(10);

static CACHE: Mutex<Option<(Instant, Option<Chain>)>> = Mutex::new(None);

/// The last read, whatever its age; never blocks (`None` before the first read). For the planner,
/// which runs where a bus call must not: the playback's open and [`prime`] keep it current.
pub fn cached() -> Option<Chain> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|(_, chain)| chain.clone())
}

/// Read the chain once on a worker at boot, so the first play's plan already knows it.
pub fn prime() {
    let _ = plx_base::task::spawn("sound-chain", || {
        chain();
    });
}

/// The sound chain now, or `None` off the TV or when the platform did not answer. Blocks.
pub fn chain() -> Option<Chain> {
    let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("LS2 sound chain") });
    // The lock is not held across the bus call: `cached` is read on the frame thread.
    if let Some((at, chain)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < FRESH {
            return chain.clone();
        }
    }
    let started = Instant::now();
    let read = match (super::port().audio_chain)() {
        Reply::NoPlatform => return None,
        Reply::Unavailable => None,
        Reply::Reply { settings, arc } => parse(&settings, &arc),
    };
    plx_base::eventlog::log(&format!(
        "sound: chain {} read in {}ms",
        read.as_ref().map_or_else(
            || "unreadable".to_string(),
            |c| format!(
                "output={} passThrough={} link={:?} capability=[{}] dts-hd={} truehd={}",
                c.output, c.pass_through, c.link, c.capability.join(","), c.passes_dts_hd(), c.passes_truehd()
            ),
        ),
        started.elapsed().as_millis()
    ));
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), read.clone()));
    read
}

/// Stand in for a read of the chain, so a test need not have a television.
#[cfg(any(test, feature = "test-support"))]
pub fn set_cached_for_test(chain: Option<Chain>) {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = chain.map(|c| (Instant::now(), Some(c)));
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = r#"{"category":"sound","settings":{"soundOutputDigital":"passThrough","eArcSupport":"on","soundOutput":"external_arc"},"returnValue":true}"#;
    const ARC: &str = r#"{"returnValue":true,"earcCapability":["PCM","DD","DTS","DD+","MAT2PCM","TrueHD","DTSX_P1","DTSHD_MA","DTSHD"],"arcActivation":"eARC"}"#;

    #[test]
    fn the_measured_c4_chain_passes_dts_hd_and_truehd() {
        let c = parse(SETTINGS, ARC).unwrap();
        assert_eq!(c.link, Link::Earc);
        assert!(c.pass_through && c.passes_dts_hd() && c.passes_truehd());
    }

    #[test]
    fn every_missing_link_in_the_chain_keeps_the_core() {
        let no = |settings: &str, arc: &str| !parse(settings, arc).unwrap().passes_dts_hd();
        assert!(no(&SETTINGS.replace("passThrough", "pcm"), ARC), "decoded output");
        assert!(no(&SETTINGS.replace("external_arc", "tv_speaker"), ARC), "TV speakers");
        assert!(no(SETTINGS, &ARC.replace("eARC", "ARC")), "plain ARC");
        assert!(no(SETTINGS, &ARC.replace("eARC", "")), "no link");
        assert!(no(SETTINGS, &ARC.replace(",\"DTSHD_MA\",\"DTSHD\"", "")), "a receiver that does not list DTS-HD");
        assert!(no(SETTINGS, r#"{"returnValue":false}"#), "a refused ARC call");
    }

    #[test]
    fn a_failed_settings_read_is_no_chain() {
        assert!(parse(r#"{"returnValue":false}"#, ARC).is_none());
        assert!(parse("not json", ARC).is_none());
        assert!(parse(SETTINGS, "not json").is_none());
    }
}
