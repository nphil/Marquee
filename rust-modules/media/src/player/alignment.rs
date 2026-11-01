//! Bounded, off-thread transport for SpeechDock's subtitle alignment protocol.
use std::collections::HashSet;
use std::sync::{Condvar, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::thread;

use serde::{Deserialize, Serialize};

const MAX_CUES: usize = 32;
const MAX_TEXT: usize = 4096;
const MAX_REQUEST: usize = 32 * 1024;
const MAX_REPLY: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    pub server_machine_id: String,
    pub rating_key: String,
    pub media_id: String,
    pub part_id: String,
    pub part_key: String,
    pub audio_stream_id: String,
    pub subtitle_stream_id: String,
    pub subtitle_codec: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Cue {
    pub cue_id: String,
    pub ordinal: u64,
    pub authored_start_ms: i64,
    pub authored_end_ms: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Request {
    pub protocol: u32,
    pub request_id: String,
    pub generation: u64,
    pub playback_id: String,
    pub identity: Identity,
    pub subtitle_source_kind: String,
    pub subtitle_hash_kind: String,
    pub subtitle_source_sha256: String,
    pub language: String,
    pub subtitle_offset_ms: i64,
    pub timeline: String,
    pub media_origin_pts_ms: i64,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub cues: Vec<Cue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Word {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct CueResult {
    pub cue_id: String,
    pub status: String,
    pub reason: String,
    pub words: Vec<Word>,
}

#[derive(Debug)]
pub(super) struct Response {
    pub request: Request,
    pub source_revision: Option<String>,
    pub cues: Vec<CueResult>,
}

#[derive(Deserialize)]
struct ResponsePayload {
    source_revision: Option<String>,
    algorithm: String,
    model_sha256: String,
    timing_provenance: String,
    cues: Vec<CueResult>,
}

fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn decimal(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn bounded_string(s: &str) -> bool {
    !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control)
}

fn valid_request(r: &Request) -> bool {
    let id = &r.identity;
    if r.protocol != 1 || !bounded_string(&r.request_id) || !bounded_string(&r.playback_id)
        || !bounded_string(&id.server_machine_id) || !bounded_string(&id.part_key)
        || !bounded_string(&id.subtitle_codec) || !decimal(&id.rating_key)
        || !decimal(&id.media_id) || !decimal(&id.part_id) || !decimal(&id.audio_stream_id)
        || !decimal(&id.subtitle_stream_id) || !digest(&r.subtitle_source_sha256)
        || !matches!(r.subtitle_source_kind.as_str(), "embedded" | "sidecar")
        || !matches!(r.subtitle_hash_kind.as_str(), "source_bytes_sha256" | "window_manifest_sha256")
        || !matches!(r.language.as_str(), "en" | "eng") || r.timeline != "container_pts"
        || r.window_start_ms < 0 || r.window_end_ms <= r.window_start_ms
        || r.window_end_ms.checked_sub(r.window_start_ms).map_or(true, |n| n > 30_000)
        || r.cues.is_empty() || r.cues.len() > MAX_CUES {
        return false;
    }
    let mut ids = HashSet::new();
    let mut ordinals = HashSet::new();
    let mut bytes = 0usize;
    for cue in &r.cues {
        bytes = match bytes.checked_add(cue.text.len()) { Some(n) => n, None => return false };
        let authored_start = cue.authored_start_ms.checked_add(r.subtitle_offset_ms);
        let authored_end = cue.authored_end_ms.checked_add(r.subtitle_offset_ms);
        if bytes > MAX_TEXT || !bounded_string(&cue.cue_id) || !ids.insert(&cue.cue_id)
            || !ordinals.insert(cue.ordinal) || cue.text.trim().is_empty()
            || cue.authored_end_ms <= cue.authored_start_ms || cue.end_ms <= cue.start_ms
            || cue.authored_end_ms.checked_sub(cue.authored_start_ms).map_or(true, |n| n > 15_000)
            || authored_start.map_or(true, |t| cue.start_ms < t)
            || authored_end.map_or(true, |t| cue.end_ms > t)
            || cue.end_ms.checked_sub(cue.start_ms).map_or(true, |n| n > 15_000)
            || cue.start_ms < r.window_start_ms || cue.end_ms > r.window_end_ms {
            return false;
        }
    }
    true
}

// Every non-whitespace byte of the original cue must be accounted for, including punctuation.
fn valid_words(cue: &Cue, words: &[Word]) -> bool {
    if words.is_empty() || words.len() > cue.text.len() { return false; }
    let mut byte = 0;
    let mut time = cue.start_ms;
    for word in words {
        if word.start_byte < byte || word.end_byte <= word.start_byte
            || word.end_byte > cue.text.len() || !cue.text.is_char_boundary(word.start_byte)
            || !cue.text.is_char_boundary(word.end_byte)
            || !cue.text[byte..word.start_byte].chars().all(char::is_whitespace)
            || cue.text[word.start_byte..word.end_byte].trim().is_empty()
            || word.start_ms < time || word.end_ms <= word.start_ms || word.end_ms > cue.end_ms {
            return false;
        }
        byte = word.end_byte;
        time = word.end_ms;
    }
    cue.text[byte..].chars().all(char::is_whitespace)
}

fn decode_response(body: &[u8], request: &Request) -> Option<Response> {
    let mut json: serde_json::Value = serde_json::from_slice(body).ok()?;
    let payload: ResponsePayload = serde_json::from_value(json.clone()).ok()?;
    let map = json.as_object_mut()?;
    if !map.contains_key("source_revision") { return None; }
    let echoed_cues = map.remove("request_cues")?;
    map.insert("cues".to_owned(), echoed_cues);
    let echoed: Request = serde_json::from_value(json).ok()?;
    if &echoed != request || payload.cues.len() != request.cues.len()
        || payload.source_revision.as_deref().is_some_and(|s| !digest(s))
        || !bounded_string(&payload.algorithm) || !bounded_string(&payload.timing_provenance)
        || !digest(&payload.model_sha256) {
        return None;
    }
    let mut seen = HashSet::new();
    for result in &payload.cues {
        let cue = request.cues.iter().find(|c| c.cue_id == result.cue_id)?;
        if !seen.insert(&result.cue_id) || result.reason.len() > 4096 { return None; }
        match result.status.as_str() {
            "aligned" if payload.source_revision.is_some() && digest(&payload.model_sha256)
                && payload.algorithm == "wav2vec2-base-960h-int8-ctc-viterbi-greedy-guard-v1"
                && payload.timing_provenance == "cpu_acoustic_ctc;independent_unprompted_greedy_exact_lexical_check;20ms_frames;uncalibrated_scores"
                && valid_words(cue, &result.words) => (),
            "estimated" if result.words.is_empty() => (),
            _ => return None,
        }
    }
    Some(Response { request: echoed, source_revision: payload.source_revision, cues: payload.cues })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlaybackSnapshot {
    pub endpoint: String,
    pub playback_id: String,
    pub identity: Identity,
    pub subtitle_source_kind: String,
    pub language: String,
    pub subtitle_offset_ms: i64,
    pub position_ms: i64,
    pub track: i32,
    pub style_generation: u32,
}
impl PlaybackSnapshot {
    fn same_context(&self, other: &Self) -> bool {
        self.endpoint == other.endpoint && self.playback_id == other.playback_id
            && self.identity == other.identity && self.subtitle_source_kind == other.subtitle_source_kind
            && self.language == other.language && self.subtitle_offset_ms == other.subtitle_offset_ms
            && self.track == other.track && self.style_generation == other.style_generation
    }
    fn window(&self) -> Option<(i64, i64)> {
        let start = self.position_ms.saturating_sub(15_000).max(0);
        Some((start, start.checked_add(30_000)?))
    }
}
#[derive(Default)]
struct State {
    generation: u64,
    current: Option<PlaybackSnapshot>,
    clear_sidecar: bool,
    disabled: bool,
}
struct Shared { state: Mutex<State>, wake: Condvar }
static EPOCH: AtomicU64 = AtomicU64::new(1);
pub(super) fn epoch() -> u64 { EPOCH.load(Ordering::Acquire) }
fn current_generation(generation: u64) -> bool { generation == epoch() }
fn advance_epoch() -> u64 {
    let mut current = EPOCH.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(1) else { return u64::MAX; };
        match EPOCH.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return next,
            Err(observed) => current = observed,
        }
    }
}
static SHARED: OnceLock<Shared> = OnceLock::new();

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| {
        let started = thread::Builder::new().name("subtitle-alignment".into()).spawn(run).is_ok();
        Shared { state: Mutex::new(State { disabled: !started, ..State::default() }), wake: Condvar::new() }
    })
}

/// Publish cheap playback facts only; source access and HTTP are confined to the worker.
pub(super) fn update(snapshot: PlaybackSnapshot) {
    let shared = shared();
    let Ok(mut state) = shared.state.try_lock() else { return; };
    if state.disabled { return; }
    let changed_context = state.generation != epoch() || state.current.as_ref().map_or(true, |old| !old.same_context(&snapshot));
    let changed_window = state.current.as_ref().map_or(true, |old| old.position_ms / 1_000 != snapshot.position_ms / 1_000);
    if changed_context {
        let next = advance_epoch();
        if next == u64::MAX { state.disabled = true; state.current = None; return; }
        state.generation = next;
        state.clear_sidecar = true;
    }
    if changed_context || changed_window {
        state.current = Some(snapshot);
        shared.wake.notify_one();
    }
}

/// Call for every seek, reset and selection change, including repeated identical seeks.
pub(crate) fn invalidate() {
    advance_epoch();
    let Some(shared) = SHARED.get() else { return; };
    if let Ok(mut state) = shared.state.try_lock() {
        state.current = None;
        state.clear_sidecar = true;
        shared.wake.notify_one();
    }
}

enum Source {
    Embedded(super::ass_source::EmbeddedSnapshot),
    Sidecar(super::sidecar::AlignmentSnapshot),
}
fn manifest_hash(cues: &[Cue]) -> Option<String> {
    let manifest: Vec<_> = cues.iter().map(|c| (c.ordinal, c.authored_start_ms, c.authored_end_ms, &c.text)).collect();
    let bytes = serde_json::to_vec(&manifest).ok()?;
    Some(plx_base::sha256::sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}
fn prepare(snapshot: &PlaybackSnapshot, generation: u64) -> Option<(Request, Source)> {
    let (start, end) = snapshot.window()?;
    let raw_start = start.checked_sub(snapshot.subtitle_offset_ms)?;
    let raw_end = end.checked_sub(snapshot.subtitle_offset_ms)?;
    let (source, mut cues, hash_kind, source_hash) = if snapshot.subtitle_source_kind == "sidecar" {
        let source = super::sidecar::alignment_snapshot(raw_start, raw_end, MAX_CUES)?;
        if source.stream_id.to_string() != snapshot.identity.subtitle_stream_id { return None; }
        let cues = source.cues.iter().map(|c| Cue { cue_id: c.ordinal.to_string(), ordinal: c.ordinal,
            authored_start_ms: c.authored_start_ms, authored_end_ms: c.authored_end_ms,
            start_ms: c.start_ms, end_ms: c.end_ms, text: c.text.clone() }).collect();
        let hash = source.source_sha256.clone();
        (Source::Sidecar(source), cues, "source_bytes_sha256", hash)
    } else if snapshot.subtitle_source_kind == "embedded" {
        let source = super::ass_source::alignment_snapshot(snapshot.track, raw_start, raw_end)?;
        let cues: Vec<Cue> = source.cues.iter().map(|c| Cue { cue_id: c.cue_id.clone(), ordinal: c.ordinal,
            authored_start_ms: c.authored_start_ms, authored_end_ms: c.authored_end_ms,
            start_ms: c.start_ms, end_ms: c.end_ms, text: c.text.clone() }).collect();
        (Source::Embedded(source), cues, "window_manifest_sha256", String::new())
    } else { return None; };
    // ASS rounds sidecar visibility to centiseconds; never ask for audio outside authored bounds.
    let mut text_bytes = 0usize;
    cues.retain_mut(|c| {
        if c.authored_end_ms.checked_sub(c.authored_start_ms).map_or(true, |n| n <= 0 || n > 15_000) { return false; }
        let raw_start = c.start_ms.max(c.authored_start_ms);
        let raw_end = c.end_ms.min(c.authored_end_ms);
        if raw_end <= raw_start { return false; }
        let Some(shifted_start) = raw_start.checked_add(snapshot.subtitle_offset_ms) else { return false };
        let Some(shifted_end) = raw_end.checked_add(snapshot.subtitle_offset_ms) else { return false };
        c.start_ms = shifted_start;
        c.end_ms = shifted_end;
        let Some(total) = text_bytes.checked_add(c.text.len()) else { return false };
        if c.start_ms < start || c.end_ms > end || c.end_ms <= c.start_ms
            || c.end_ms.saturating_sub(c.start_ms) > 15_000 || total > MAX_TEXT { return false; }
        text_bytes = total;
        true
    });
    let source_hash = if hash_kind == "window_manifest_sha256" { manifest_hash(&cues)? } else { source_hash };
    let request = Request { protocol: 1, request_id: format!("{}-{generation}", snapshot.playback_id), generation,
        playback_id: snapshot.playback_id.clone(), identity: snapshot.identity.clone(),
        subtitle_source_kind: snapshot.subtitle_source_kind.clone(), subtitle_hash_kind: hash_kind.into(),
        subtitle_source_sha256: source_hash, language: snapshot.language.clone(),
        subtitle_offset_ms: snapshot.subtitle_offset_ms, timeline: "container_pts".into(), media_origin_pts_ms: 0,
        window_start_ms: start, window_end_ms: end, cues };
    valid_request(&request).then_some((request, source))
}

#[cfg(test)]
pub(super) fn prepare_request_for_test(snapshot: &PlaybackSnapshot) -> Option<Request> {
    prepare(snapshot, epoch()).map(|(request, _)| request)
}

fn exchange(endpoint: &str, request: &Request) -> Option<Response> {
    let root = endpoint.trim_end_matches('/');
    if !(root.starts_with("https://") || root.starts_with("http://")) || root.len() > 2048
        || root.chars().any(char::is_control) || root.contains('?') || root.contains('#') || root.contains('@') { return None; }
    let body = serde_json::to_vec(request).ok().filter(|b| b.len() <= MAX_REQUEST)?;
    let headers = ["Content-Type: application/json".to_owned()];
    let timeout = plx_net::net::Timeouts { connect_s: 5, total_s: 20, total_ms: 0,
        low_speed_bps: 0, low_speed_s: 0 };
    let reply = plx_net::net::request(&format!("{root}/api/alignment/windows"), &headers, "POST",
        Some(&body), timeout, false, Some(MAX_REPLY), None)?;
    (reply.status == 200).then(|| decode_response(&reply.body, request)).flatten()
}
fn apply(source: Source, response: Response, offset: i64, generation: u64) -> Option<()> {
    let converted = |result: &CueResult| -> Option<Vec<super::reveal::AlignedWord>> {
        result.words.iter().map(|w| Some(super::reveal::AlignedWord { start_byte: w.start_byte,
            end_byte: w.end_byte, start_ms: w.start_ms.checked_sub(offset)?, end_ms: w.end_ms.checked_sub(offset)? })).collect()
    };
    match source {
        Source::Embedded(snapshot) => {
            let results: Option<Vec<_>> = response.cues.iter()
                .map(|r| Some(super::ass_source::EmbeddedResult { cue_id: r.cue_id.clone(), words: converted(r)? })).collect();
            if !super::ass_source::apply_alignment(&snapshot, &results?, response.source_revision.as_deref()) { return None; }
        }
        Source::Sidecar(snapshot) => {
            let updates: Option<Vec<_>> = response.cues.iter().map(|r| {
                let ordinal = response.request.cues.iter().find(|c| c.cue_id == r.cue_id)?.ordinal;
                Some(super::sidecar::AlignmentUpdate { ordinal, words: converted(r)? })
            }).collect();
            if !super::sidecar::apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, generation, updates?, response.source_revision.as_deref()) { return None; }
        }
    }
    Some(())
}
fn run() {
    // Initialization finishes before this worker can acquire the shared state.
    let shared = shared();
    let mut completed: Option<(u64, String)> = None;
    let mut last_attempt = Instant::now() - Duration::from_secs(3);
    loop {
        let (snapshot, generation, clear) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.current.is_none() && !state.clear_sidecar && !state.disabled {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.disabled { return; }
            let clear = std::mem::take(&mut state.clear_sidecar);
            (state.current.clone(), state.generation, clear)
        };
        if clear { super::sidecar::invalidate_alignment(); }
        if let Some(snapshot) = snapshot.filter(|_| current_generation(generation)) {
            if let Some((request, source)) = prepare(&snapshot, generation) {
                let key = manifest_hash(&request.cues).unwrap_or_default();
                if completed.as_ref() != Some(&(generation, key.clone())) && last_attempt.elapsed() >= Duration::from_secs(2) {
                    last_attempt = Instant::now();
                    if !current_generation(generation) || match &source {
                        Source::Sidecar(sidecar) => !super::sidecar::alignment_ready(sidecar.generation, sidecar.source_id, sidecar.source_revision),
                        Source::Embedded(embedded) => !super::ass_source::alignment_ready(embedded),
                    } { continue; }
                    if let Some(response) = exchange(&snapshot.endpoint, &request) {
                        if current_generation(generation) && apply(source, response, snapshot.subtitle_offset_ms, generation).is_some()
                            && current_generation(generation) {
                            completed = Some((generation, key));
                        }
                    }
                }
            }
        }
        let state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = shared.wake.wait_timeout(state, Duration::from_secs(2)).unwrap_or_else(|e| e.into_inner());
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn regression_estimated_result_reaches_embedded_consumer() {
        use super::super::{ass, ass_source};
        let _guard = plx_base::testlock::serial();
        super::super::sidecar::reset();
        super::super::request_subtitle(0);
        let generation = ass_source::begin("consumer-estimate", vec![(0, ass_source::TrackHeader::Text(super::super::text_ass::TextDialect::Srt))], vec![]);
        ass_source::push_text(generation, 0, 1_000_000_000, 4_000_000_000, "hello", 1);
        let render = || {
            let source = ass_source::try_selected(0).unwrap().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                ass::request(source.clone(), 1500, 320, 180, 320, 180);
                if ass::source_revision_readiness(source.id, source.revision) == ass::SourceReadiness::Ready { break; }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        render();
        let snapshot = ass_source::alignment_snapshot(0, 0, 5000).unwrap();
        let cue_id = snapshot.cues[0].cue_id.clone();
        let epoch = snapshot.epoch;
        let result = CueResult { cue_id: cue_id.clone(), status: "aligned".into(), reason: "acoustic_match".into(),
            words: vec![Word { start_byte: 0, end_byte: 5, start_ms: 1100, end_ms: 1500 }] };
        assert!(apply(Source::Embedded(snapshot), Response { request: request(), source_revision: Some("a".repeat(64)), cues: vec![result] }, 0, epoch).is_some());
        render();
        let snapshot = ass_source::alignment_snapshot(0, 0, 5000).unwrap();
        let previous_id = snapshot.source_id;
        let result = CueResult { cue_id, status: "estimated".into(), reason: "source_revision_changed".into(), words: vec![] };
        assert!(apply(Source::Embedded(snapshot), Response { request: request(), source_revision: None, cues: vec![result] }, 0, epoch).is_some());
        assert_ne!(ass_source::try_selected(0).unwrap().unwrap().id, previous_id, "the old aligned frame must be retired");
        super::super::request_subtitle(-1);
        ass_source::reset();
        ass::clear();
    }

    fn request() -> Request {
        Request { protocol: 1, request_id: "r".into(), generation: 1, playback_id: "p".into(),
            identity: Identity { server_machine_id: "machine".into(), rating_key: "1".into(),
                media_id: "2".into(), part_id: "3".into(), part_key: "/part".into(),
                audio_stream_id: "4".into(), subtitle_stream_id: "5".into(), subtitle_codec: "srt".into() },
            subtitle_source_kind: "sidecar".into(), subtitle_hash_kind: "source_bytes_sha256".into(),
            subtitle_source_sha256: "a".repeat(64), language: "en".into(), subtitle_offset_ms: 0,
            timeline: "container_pts".into(), media_origin_pts_ms: 0, window_start_ms: 0,
            window_end_ms: 5000, cues: vec![Cue { cue_id: "a".into(), ordinal: 0,
                authored_start_ms: 100, authored_end_ms: 2000, start_ms: 100, end_ms: 2000,
                text: "hé you!".into() }] }
    }
    fn response(r: &Request) -> serde_json::Value {
        let mut v = serde_json::to_value(r).unwrap();
        let map = v.as_object_mut().unwrap();
        let cues = map.remove("cues").unwrap();
        map.insert("request_cues".into(), cues);
        map.insert("source_revision".into(), json!("b".repeat(64)));
        map.insert("algorithm".into(), json!("wav2vec2-base-960h-int8-ctc-viterbi-greedy-guard-v1"));
        map.insert("model_sha256".into(), json!("c".repeat(64)));
        map.insert("timing_provenance".into(), json!("cpu_acoustic_ctc;independent_unprompted_greedy_exact_lexical_check;20ms_frames;uncalibrated_scores"));
        map.insert("cues".into(), json!([{ "cue_id":"a", "status":"aligned", "reason":"",
            "words":[{"start_byte":0,"end_byte":3,"start_ms":100,"end_ms":400},
                     {"start_byte":4,"end_byte":8,"start_ms":500,"end_ms":900}] }]));
        v
    }
    fn accepted(v: &serde_json::Value, r: &Request) -> bool {
        decode_response(&serde_json::to_vec(v).unwrap(), r).is_some()
    }
    #[test]
    fn accepts_complete_echo_and_rejects_changed_identity() {
        let r = request(); assert!(valid_request(&r));
        let mut v = response(&r); assert!(accepted(&v, &r));
        v["identity"]["part_id"] = json!("99"); assert!(!accepted(&v, &r));
    }
    #[test]
    fn rejects_bad_utf8_boundaries_missing_text_and_outside_times() {
        let r = request();
        for (key, value) in [("end_byte", json!(2)), ("start_ms", json!(99)), ("end_ms", json!(2001))] {
            let mut v = response(&r); v["cues"][0]["words"][0][key] = value;
            assert!(!accepted(&v, &r));
        }
        let mut v = response(&r); v["cues"][0]["words"] = json!([]); assert!(!accepted(&v, &r));
    }
    #[test]
    fn rejects_mismatched_cues_and_untrusted_identifiers() {
        let mut r = request(); let mut v = response(&r);
        v["request_cues"][0]["text"] = json!("other"); assert!(!accepted(&v, &r));
        r.identity.audio_stream_id = "../4".into(); assert!(!valid_request(&r));
    }
    #[test]
    fn estimated_never_carries_words() {
        let r = request(); let mut v = response(&r);
        v["cues"][0]["status"] = json!("estimated"); assert!(!accepted(&v, &r));
        v["cues"][0]["words"] = json!([]); assert!(accepted(&v, &r));
    }
    #[test]
    fn rejects_stale_generation_and_incomplete_results() {
        let r = request();
        let mut v = response(&r); v["generation"] = json!(2); assert!(!accepted(&v, &r));
        let mut v = response(&r); v["cues"] = json!([]); assert!(!accepted(&v, &r));
        let mut v = response(&r); v["algorithm"] = json!("estimated"); assert!(!accepted(&v, &r));
        let mut v = response(&r); v["timing_provenance"] = json!("other"); assert!(!accepted(&v, &r));
        let mut v = response(&r); v["cues"][0]["cue_id"] = json!("other"); assert!(!accepted(&v, &r));
    }
    #[test]
    fn embedded_hash_uses_compact_utf8_manifest() {
        let r = request();
        let expected: String = plx_base::sha256::sha256("[[0,100,2000,\"hé you!\"]]".as_bytes())
            .iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(manifest_hash(&r.cues), Some(expected));
    }
    #[test]
    fn rejects_oversized_windows_and_duplicate_cues() {
        let mut r = request(); r.window_end_ms = 30_001; assert!(!valid_request(&r));
        let mut r = request(); r.cues.push(r.cues[0].clone()); assert!(!valid_request(&r));
    }
    #[test]
    fn visible_cues_crossing_old_window_boundaries_fit_whole() {
        let r = request();
        let mut snapshot = PlaybackSnapshot { endpoint: "https://speechdock.example".into(),
            playback_id: r.playback_id, identity: r.identity, subtitle_source_kind: "sidecar".into(),
            language: "en".into(), subtitle_offset_ms: 0, position_ms: 9_000, track: 0, style_generation: 1 };
        let original = snapshot.clone();
        for position in [9_000, 10_000, 15_500, 20_000, 21_999] {
            snapshot.position_ms = position;
            let (start, end) = snapshot.window().unwrap();
            assert!(start <= 9_000 && end >= 22_000 && end - start <= 30_000);
            assert!(original.same_context(&snapshot), "moving the window must not fence rendered words");
        }
    }
    #[test]
    fn longest_eligible_cue_fits_at_each_visible_position() {
        let r = request();
        let mut snapshot = PlaybackSnapshot { endpoint: "https://speechdock.example".into(),
            playback_id: r.playback_id, identity: r.identity, subtitle_source_kind: "embedded".into(),
            language: "en".into(), subtitle_offset_ms: 0, position_ms: 100_000, track: 0, style_generation: 1 };
        for position in [100_000, 105_000, 110_000, 114_999] {
            snapshot.position_ms = position;
            let (start, end) = snapshot.window().unwrap();
            assert!(start <= 100_000 && end >= 115_000);
        }
    }
}
