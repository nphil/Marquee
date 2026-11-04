//! **External (sidecar) subtitles on the direct-play path** — the `.srt` beside the film.
//!
//! The demuxer only ever sees the streams INSIDE the container it opened, so a sidecar was
//! unreachable on direct play and the track menu hid it (`docs/parity-gaps.md`, "Sidecar
//! (external) subtitle streams are unreachable on the direct-play path"). This is the missing
//! producer: fetch the whole file from PMS once. ASS/SSA keeps its complete script for
//! [`ass_source`] and the native styled renderer (styles, overlapping events, drawings and embedded
//! fonts must never pass through a text parser). SubRip and WebVTT are converted on the fetch worker
//! to an ASS script of the viewer's synthesized style (`text_ass::script_from_srt`/`_vtt`, charset
//! by `sub_charset`), and render through the same libass worker as an embedded text track; the
//! script is restyled lazily when the viewer's size or position pick changes.
//!
//! # Why its own store, and not `ass_source`
//!
//! That store is a WINDOW the demuxer refills as it reads, and a backward seek empties it. A
//! sidecar arrives once, whole — 1,000-2,000 cues for a feature — and nothing re-reads it after a
//! seek. The converted script is kept intact (a long one is fed to libass a window at a time,
//! `ass_script`), and the clock a frame is rendered for is the screen's.
//!
//! # The clock
//!
//! A sidecar's timestamps are file time, and on DIRECT PLAY `playpos_ns` is file time too (the
//! rebase keeps it so across seeks). Where the server BURNS the selection into the picture (a
//! re-encode, or a remux the app may not draw over) [`ass_source`] answers nothing, and the caller
//! draws no [`notice`], while it says the selection is burned (`route::subtitles_burned`) — otherwise a direct play that becomes
//! a burn mid-film (a DTS audio pick) would show the line twice. Over a plain remux nothing is
//! burned and a sidecar is drawn from here. The fact is PASSED IN rather than read: the playback session is
//! the frame's publication, and the draw that calls this already holds it.
//!
//! Over a REMUX the playhead is not file time: PMS starts the remux on a Part keyframe EARLIER
//! than or at the requested offset — not always the last one (a 917 s request started on 914.914 s
//! while 916.916 s existed; see `ff_subside`'s `sidecar_clock_at_917…` test) — and the engine
//! labels that frame with the requested offset (`disp_base`), so the playhead leads the content by
//! the snap (0.6 s to 2.2 s measured). The side reader finds that keyframe clock-only
//! (`subside::CLOCK_ONLY`: no cue read, byte-exact anchor match only) and
//! `player::sidecar_clock_ns` adds its delta to the position the sidecar is drawn at, for a
//! sidecar over a live remux only. Without an exact match, or off the reader's local-link and
//! bitrate gate, nothing is added and the sidecar keeps the remux's clock. Until the measurement
//! lands (a moment after each remux start), nothing is added either: the sidecar is drawn exactly
//! as before for that moment, and a cue already shown early can appear a second time once the
//! position drops.
//!
//! The screen samples a sidecar at that position (`player::sidecar_clock_ns`; `player::subtitle_draw`
//! pairs source and position the same way) and subtracts the viewer's timing offset from the clock it
//! samples there, so the offset composes with the correction. Because the whole file is here, a sidecar is the one kind of track
//! that can be ADVANCED as well as delayed — [`selected`] is what `player::subtitle_offset_range_ms`
//! reads to offer it the sidecar offset range (see `subtitle_offset_range_ms`), where an embedded track
//! gets a delay only.
//!
//! # Threading
//!
//! [`select`]/[`deselect`]/[`active`]/[`ass_source`] are main-thread calls; the fetch runs on a `task` worker and
//! lands under the one mutex, fenced by a generation so an answer for a pick the viewer has
//! already moved off is dropped rather than installed. One worker drains a single latest-pick
//! slot, so repeated picks cannot accumulate threads or downloads.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::text_ass;
use super::text_style::TextStyle;

/// Why nothing is being drawn for a selected sidecar — said ON SCREEN for a few seconds, in the
/// caption's own place, because a picked subtitle that silently shows nothing is
/// indistinguishable from a film with a quiet first minute.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Failure {
    Fetch,
    Empty,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Failure::Fetch => plx_platform::i18n::msg::widgets_sidecar_fetch_failed(),
            Failure::Empty => plx_platform::i18n::msg::widgets_sidecar_empty(),
        }
    }
}

/// Stream ids belong to a server; the codec controls how that server is asked for the file.
/// Keep all of that identity beside the cache so identical ids on two servers cannot alias.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    server: plx_plex::plex::ServerId,
    stream_id: i64,
    key: String,
    codec: String,
}

/// A loaded file as the renderer takes it.
struct Content {
    source: Arc<super::ass::Source>,
    font_generation: Option<u64>,
    /// Set for a converted SubRip/WebVTT file, whose header follows the viewer's picks.
    text: Option<TextHold>,
}

/// What a converted file needs to be restyled: the script is the synthesized header followed by
/// the converted `Dialogue:` lines, so the events are the bytes past `header_len` and are not kept
/// twice. The exception is "Reveal as spoken": its tags are written into those lines, so the plain
/// lines are kept beside them (`plain`) for the restyle that turns it off or changes it.
struct TextHold {
    header_len: usize,
    /// The picks (`TextStyle::generation`) the held header was synthesized for.
    style_generation: u32,
    /// Cues that survived conversion.
    cues: usize,
    /// The `Dialogue:` lines before any reveal tags, held only while the script carries them.
    plain: Option<Arc<str>>,
    source_sha256: String,
    script: Arc<text_ass::TextScript>,
    aligned: Arc<std::collections::BTreeMap<u64, Vec<super::reveal::AlignedWord>>>,
    verified_source_revision: Option<String>,
    alignment_revision: u64,
    published_alignment_revision: u64,
    alignment_epoch: u64,
    estimated: Option<Arc<super::ass::Source>>,
    /// A worker is already building the script for newer picks (one at a time; a later pick that
    /// lands meanwhile is taken up by the next call once this one has published).
    restyling: bool,
}

struct State {
    want: Option<Selection>,
    generation: u64,
    alignment_generation: u64,
    pending: Option<(u64, Selection)>,
    running: bool,
    /// The complete selected file, kept across Off→On so re-picking is instant.
    loaded: Option<(Selection, Content)>,
    failed: Option<(Selection, Failure, Instant)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    want: None,
    generation: 0,
    alignment_generation: 0,
    pending: None,
    running: false,
    loaded: None,
    failed: None,
});

/// Signalled, under [`STATE`], whenever the download worker goes idle or a restyle worker has
/// finished: what the tests wait on instead of the clock.
static SETTLED: std::sync::Condvar = std::sync::Condvar::new();

/// How long a [`Failure`] stays on screen.
const FAILURE_SHOWN: Duration = Duration::from_secs(6);
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) struct AlignmentSnapshot {
    pub generation: u64,
    pub source_id: u64,
    pub source_revision: u64,
    pub stream_id: i64,
    pub source_sha256: String,
    pub cues: Vec<AlignmentCue>,
}

pub(super) struct AlignmentCue {
    pub ordinal: u64,
    pub authored_start_ms: i64,
    pub authored_end_ms: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub(super) struct AlignmentUpdate {
    pub ordinal: u64,
    pub words: Vec<super::reveal::AlignedWord>,
}

fn ready_source(content: &Content) -> bool {
    let source = selected_source(content);
    super::ass::source_revision_readiness(source.id, source.revision) == super::ass::SourceReadiness::Ready
}

/// Worker-only: recheck immediately before submitting a captured window.
pub(super) fn alignment_ready(generation: u64, source_id: u64, source_revision: u64) -> bool {
    let st = state();
    if st.alignment_generation != generation { return false; }
    let Some((selection, content)) = st.loaded.as_ref() else { return false };
    let source = selected_source(content);
    st.want.as_ref() == Some(selection) && content.text.is_some()
        && source.id == source_id && source.revision == source_revision && ready_source(content)
}

/// Worker-only: clone a bounded window of the exact text that produced the selected script.
pub(super) fn alignment_snapshot(start_ms: i64, end_ms: i64, limit: usize) -> Option<AlignmentSnapshot> {
    if end_ms <= start_ms || limit == 0 { return None; }
    let (generation, source_id, source_revision, stream_id, source_sha256, script) = {
        let st = state();
        let (selection, content) = st.loaded.as_ref()?;
        if st.want.as_ref() != Some(selection) || !ready_source(content) { return None; }
        let hold = content.text.as_ref()?;
        let source = selected_source(content);
        (st.alignment_generation, source.id, source.revision, selection.stream_id, hold.source_sha256.clone(), hold.script.clone())
    };
    let cues = script.source_cues.iter()
        .filter(|cue| cue.end_ms > start_ms && cue.start_ms < end_ms && cue.text.len() <= 4096)
        .take(limit.min(32))
        .map(|cue| AlignmentCue { ordinal: cue.ordinal, authored_start_ms: cue.authored_start_ms,
            authored_end_ms: cue.authored_end_ms, start_ms: cue.start_ms, end_ms: cue.end_ms,
            text: cue.text.clone() }).collect();
    Some(AlignmentSnapshot { generation, source_id, source_revision, stream_id, source_sha256, cues })
}

impl TextHold {
    /// Merge a realignment answered for `epoch`. True, with the alignment revision bumped, only when
    /// a cue's words changed: the worker asks again every few seconds of dialogue, mostly for words
    /// already held, and each bump rebuilds the whole script and has the renderer parse it again.
    fn realign(&mut self, epoch: u64, updates: Vec<AlignmentUpdate>, verified_revision: Option<&str>) -> bool {
        let mut changed = false;
        if self.alignment_epoch != epoch || verified_revision.is_none() || self.verified_source_revision.as_deref() != verified_revision {
            // Words of another epoch or source revision go, which changes the script if any were held.
            changed = !self.aligned.is_empty();
            self.aligned = Arc::new(std::collections::BTreeMap::new());
        }
        self.verified_source_revision = verified_revision.map(str::to_owned);
        self.alignment_epoch = epoch;
        for update in updates {
            let held = self.aligned.get(&update.ordinal);
            if held.map_or(update.words.is_empty(), |words| *words == update.words)
                || !self.script.source_cues.iter().any(|cue| cue.ordinal == update.ordinal) { continue; }
            let aligned = Arc::make_mut(&mut self.aligned);
            if update.words.is_empty() { aligned.remove(&update.ordinal); } else { aligned.insert(update.ordinal, update.words); }
            changed = true;
        }
        if changed { self.alignment_revision = self.alignment_revision.wrapping_add(1); }
        changed
    }
}

/// Worker-only: the orchestrator validates identity, provenance and response before this call.
pub(super) fn apply_alignment(generation: u64, source_id: u64, source_revision: u64, epoch: u64, updates: Vec<AlignmentUpdate>, verified_revision: Option<&str>) -> bool {
    let style = TextStyle::current();
    let mut st = state();
    if st.alignment_generation != generation { return false; }
    if verified_revision.is_none() && updates.iter().any(|update| !update.words.is_empty()) { return false; }
    if super::alignment::epoch() != epoch { return false; }
    let State { want, loaded, .. } = &mut *st;
    let Some((selection, content)) = loaded.as_mut() else { return false };
    let source = selected_source(content);
    if want.as_ref() != Some(selection) || source.id != source_id || source.revision != source_revision
        || !ready_source(content) { return false; }
    let Some(hold) = content.text.as_mut() else { return false };
    if hold.realign(epoch, updates, verified_revision) {
        start_restyle(content, style);
    }
    true
}

/// Worker-only: invalidate in-flight answers and remove times from the previous playback context.
pub(super) fn invalidate_alignment() {
    let style = TextStyle::current();
    let mut st = state();
    st.alignment_generation = st.alignment_generation.wrapping_add(1);
    if let Some((_, content)) = st.loaded.as_mut() {
        if let Some(hold) = content.text.as_mut() {
            if !hold.aligned.is_empty() {
                hold.aligned = Arc::new(std::collections::BTreeMap::new());
                hold.alignment_epoch = super::alignment::epoch();
                hold.verified_source_revision = None;
                hold.alignment_revision = hold.alignment_revision.wrapping_add(1);
                start_restyle(content, style);
            }
        }
    }
}

/// Select the sidecar `stream_id`, fetching `key` from `server` unless that file is already
/// loaded. MAIN THREAD.
pub fn select(server: plx_plex::plex::ServerId, stream_id: i64, key: String, codec: String) {
    select_with_fetch(server, stream_id, key, codec, |server, key, codec| {
        let Some(client) = plx_plex::plex::client_for(server) else {
            super::log("sidecar: no client for the playing server");
            return None;
        };
        client.sidecar_subtitle_detailed(key, codec)
            .map_err(|why| super::log(&format!("sidecar: fetch failed: {why}")))
            .ok()
    });
}

fn select_with_fetch(
    server: plx_plex::plex::ServerId, stream_id: i64, key: String, codec: String,
    fetch: impl Fn(plx_plex::plex::ServerId, &str, &str) -> Option<Vec<u8>> + Send + 'static,
) {
    if stream_id <= 0 || key.is_empty() {
        deselect();
        return;
    }
    let selection = Selection { server, stream_id, key, codec: codec.to_ascii_lowercase() };
    {
        let mut st = state();
        let selection_changed = st.want.as_ref() != Some(&selection);
        st.generation = st.generation.wrapping_add(1);
        st.alignment_generation = st.alignment_generation.wrapping_add(1);
        st.want = Some(selection.clone());
        st.failed = None;
        st.pending = None;
        if matches!(&st.loaded, Some((source, _)) if *source == selection) {
            if selection_changed {
                // A cached script retains its source ID; retire the old selection's frame.
                super::ass::clear();
            }
            return;
        }
        st.loaded = None;
        st.pending = Some((st.generation, selection.clone()));
        if st.running { return; }
        st.running = true;
    }
    let spawned = plx_base::task::spawn_small("sidecar", move || loop {
        let (gen, selection) = {
            let mut st = state();
            let Some(request) = st.pending.take() else {
                st.running = false;
                SETTLED.notify_all();
                return;
            };
            request
        };
        let stream_id = selection.stream_id;
        super::log(&format!("sidecar: fetching stream {stream_id}"));
        let body = fetch(selection.server, &selection.key, &selection.codec);
        let outcome = match body {
            None => Err(Failure::Fetch),
            Some(bytes) => decode(bytes, &selection.codec),
        };
        let mut st = state();
        if st.generation != gen {
            continue; // check and publication share the selection/reset lock
        }
        match outcome {
            Ok(content) => {
                match &content.text {
                    Some(hold) => super::log(&format!("sidecar: stream {stream_id} ready, {} cues", hold.cues)),
                    None => super::log(&format!("sidecar: stream {stream_id} ready, styled ASS")),
                }
                st.loaded = Some((selection, content));
            }
            Err(why) => {
                super::log(&format!("sidecar: stream {stream_id} FAILED ({why:?})"));
                st.failed = Some((selection, why, Instant::now()));
            }
        }
        plx_machine::present::wake_from_worker();
    });
    if !spawned {
        let mut st = state();
        st.running = false;
        st.pending = None;
        st.failed = Some((selection, Failure::Fetch, Instant::now()));
        SETTLED.notify_all();
    }
}

/// Stop drawing a sidecar (Off, or an embedded track was picked). The loaded file is KEPT, so
/// turning the same one back on is instant. MAIN THREAD.
pub fn deselect() {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.alignment_generation = st.alignment_generation.wrapping_add(1);
    st.pending = None;
    st.want = None;
    st.failed = None;
}

/// Is a sidecar the selected subtitle? True from the pick, before its file has arrived — the
/// question is which KIND of track the viewer chose (it decides the timing offset's range,
/// `player::subtitle_offset_range_ms`), not whether a cue is ready.
pub fn selected() -> bool {
    state().want.is_some()
}

/// Read the selected file and its renderer publication without starting any work.
pub(crate) fn activation(server: plx_plex::plex::ServerId, stream_id: i64) -> crate::route::SubtitleActivation {
    use crate::route::SubtitleActivation;
    let st = state();
    let Some(want) = st.want.as_ref() else { return SubtitleActivation::Inactive };
    if want.server != server || want.stream_id != stream_id {
        return SubtitleActivation::Inactive;
    }
    if st.failed.as_ref().is_some_and(|(selection, _, _)| selection == want) {
        return SubtitleActivation::Failed;
    }
    let Some((selection, content)) = st.loaded.as_ref() else { return SubtitleActivation::Loading };
    if selection != want { return SubtitleActivation::Loading; }
    match super::ass::source_readiness(selected_source(content).id) {
        super::ass::SourceReadiness::Pending => SubtitleActivation::Loading,
        super::ass::SourceReadiness::Ready => SubtitleActivation::Active,
        super::ass::SourceReadiness::Failed => SubtitleActivation::Failed,
    }
}

/// Put the test-only sidecar state in place: `stream_id` selected with `codec`, and `script`
/// installed as the loaded file when given. The one door into the private state for tests.
#[cfg(any(test, feature = "test-support"))]
fn select_for_test(stream_id: i64, codec: &str, script: Option<&[u8]>) {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.alignment_generation = st.alignment_generation.wrapping_add(1);
    st.pending = None;
    st.failed = None;
    let selection = Selection {
        server: plx_plex::plex::ServerId::UNSET, stream_id, key: String::new(), codec: codec.to_owned(),
    };
    st.want = Some(selection.clone());
    st.loaded = script.map(|s| {
        let content = decode(s.to_vec(), codec).expect("fixture: an authored script");
        (selection, content)
    });
}

/// Mark `stream_id` as the selected sidecar without fetching anything — for tests of what the
/// selection KIND decides (the timing offset's range), which never draw a cue.
#[cfg(any(test, feature = "test-support"))]
pub fn select_without_fetch_for_test(stream_id: i64) {
    select_for_test(stream_id, "", None);
}

/// Select `stream_id` as a sidecar and install `script` as its loaded file, without fetching: for
/// tests of what the DRAW is handed for a sidecar over a remux (`player::subtitle_draw`). The
/// script must be an authored ASS/SSA one, so nothing restyles it on a worker.
#[cfg(any(test, feature = "test-support"))]
pub fn select_loaded_for_test(stream_id: i64, script: &[u8]) {
    select_for_test(stream_id, "ass", Some(script));
}

/// Select `stream_id` as a sidecar whose conversion FAILED `age` ago, without fetching anything: the
/// failure notice is up for [`FAILURE_SHOWN`] from then. For the player screen's frame-fingerprint
/// tests, which cannot wait six seconds and cannot reach the private state.
#[cfg(any(test, feature = "test-support"))]
pub fn fail_for_test(stream_id: i64, age: Duration) {
    select_without_fetch_for_test(stream_id);
    let mut st = state();
    let want = st.want.clone().expect("just selected");
    st.failed = Some((want, Failure::Empty, Instant::now().checked_sub(age).unwrap_or_else(Instant::now)));
}

/// A new item is starting: nothing of the previous one's may survive. MAIN THREAD.
pub fn reset() {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.alignment_generation = st.alignment_generation.wrapping_add(1);
    st.pending = None;
    st.want = None;
    st.loaded = None;
    st.failed = None;
}

/// **Honour the sidecar this part shows from the start**: `planned`, the stream id the resolve chose
/// for this episode (`Plan::memory_sidecar`: the profile's track memory, or an automatic pick that
/// follows the audio over the server's), else the one the SERVER already has selected for this part
/// (picked here in an earlier session, or on another Plex client). Either way the key and codec are
/// looked up in the PLAYING item, so a remembered external subtitle uses the file THIS episode has,
/// never the one it was picked on. The embedded twin is `route::pick_dp_subtitle`, which leaves an
/// external selection off because nothing could render it; now something can. `planned` must still
/// be a sidecar the app can draw (`Stream::sidecar_renderable`) or it is ignored. Direct play, or a
/// plain remux start (the caller's gate): a start that burns keeps subtitles off, as before.
pub fn restore_selection(
    server: plx_plex::plex::ServerId, meta: plx_data::metadata::MetadataView<'_>, planned: Option<i64>,
) -> Option<i64> {
    let item = meta.playing()?;
    let planned = planned.and_then(|id| item.subs.iter().find(|s| s.id == id && s.sidecar_renderable()));
    let (s, whose) = match planned {
        Some(s) => (s, "planned"),
        None => (plx_data::metadata::server_selected_sidecar(item)?, "server-selected"),
    };
    super::log(&format!("{whose} sidecar subtitle: sid={}", s.id));
    select(server, s.id, s.key.clone(), s.codec.clone());
    Some(s.id)
}

/// The failure notice to draw, if a sidecar is selected: why nothing is being drawn for it, while
/// that is worth saying on screen — for [`FAILURE_SHOWN`] after its fetch or conversion failed. The
/// cues themselves are libass's ([`ass_source`]); this is the one thing the renderer cannot say,
/// that a picked file produced nothing. The caller applies the burned rule (the server BURNS the
/// selection; drawing the notice too would double the line).
pub fn notice() -> Option<&'static str> {
    let st = state();
    let want = st.want.as_ref()?;
    match &st.failed {
        Some((source, why, at)) if source == want && at.elapsed() < FAILURE_SHOWN => Some(why.message()),
        _ => None,
    }
}

/// Is [`notice`] up right now? The player's frame fingerprint reads it, so the instant the notice
/// expires still owes a frame (nothing else on the screen changes then).
pub fn notice_shown() -> bool {
    notice().is_some()
}

/// The selected script, unless the server burns it. It is the same immutable source through seeks
/// and Off→On; render requests carry the current clock, so no cue is lost on a backward seek. A
/// converted SubRip/WebVTT file whose header was synthesized for other picks than the viewer's now
/// has its restyle started here, lazily, on a worker; the old look stays up until the worker
/// publishes the restyled script (an unselected file is never restyled).
pub fn ass_source(burned: bool) -> Option<Arc<super::ass::Source>> {
    ass_source_styled(burned, TextStyle::current())
}

/// [`ass_source`] for explicit picks (the tests restyle without touching the preferences).
fn ass_source_styled(burned: bool, style: TextStyle) -> Option<Arc<super::ass::Source>> {
    if burned { return None; }
    let mut st = state();
    let State { want, loaded, .. } = &mut *st;
    let want = want.as_ref()?;
    let (selection, content) = loaded.as_mut()?;
    if selection != want {
        return None;
    }
    // The file can arrive before its movie's demuxer discovers attachments. Preserve both buffers
    // by Arc and revise only when that immutable font publication, or the picks, changed. The font
    // store never acquires the sidecar lock, so this short snapshot has one lock order; no native
    // work runs under either lock.
    let (generation, fonts) = super::ass_source::font_snapshot();
    let restyled = content.text.as_ref().is_some_and(|hold| hold.style_generation != style.generation()
        || hold.published_alignment_revision != hold.alignment_revision);
    if restyled {
        // The new script is built off this thread (it is the whole file again, megabytes for a long
        // one) and published under this lock when ready; until then the old look stays up.
        plx_base::diag::spans::span("restyle", || start_restyle(content, style));
    }
    if content.font_generation != Some(generation) {
        let super::ass::Content::Script { bytes, .. } = &content.source.content else { return None; };
        content.source = Arc::new(super::ass::Source {
            id: content.source.id, revision: content.source.revision.wrapping_add(1),
            alignment_epoch: content.source.alignment_epoch,
            cue_timings: content.source.cue_timings.clone(),
            origin: content.source.origin, content: super::ass::Content::Script { bytes: bytes.clone(), fonts: fonts.clone() },
        });
        if let Some(hold) = content.text.as_mut() {
            if let Some(estimated) = hold.estimated.as_mut() {
                if let super::ass::Content::Script { bytes, .. } = &estimated.content {
                    *estimated = Arc::new(super::ass::Source { id: estimated.id,
                        revision: estimated.revision.wrapping_add(1), origin: estimated.origin,
                        alignment_epoch: None,
                        cue_timings: estimated.cue_timings.clone(),
                        content: super::ass::Content::Script { bytes: bytes.clone(), fonts } });
                }
            }
        }
        content.font_generation = Some(generation);
    }
    Some(selected_source(content).clone())
}

fn selected_source(content: &Content) -> &Arc<super::ass::Source> {
    if let Some(hold) = &content.text {
        // Only a script carrying SpeechDock word times is stamped with its alignment epoch; one
        // without them is valid in every epoch, so a restyle keeps its identity and the renderer.
        if !super::ass::alignment_epoch_valid(content.source.alignment_epoch)
            || hold.published_alignment_revision != hold.alignment_revision {
            if let Some(estimated) = &hold.estimated { return estimated; }
        }
    }
    &content.source
}

/// Begin building `content`'s script for `style` on a worker, unless one is already running.
/// FRAME THREAD, under the sidecar lock: it copies two `Arc`s and starts a thread, whatever the size
/// of the file. The worker publishes with the font snapshot current by then, so a font arriving
/// meanwhile is not lost.
fn start_restyle(content: &mut Content, style: TextStyle) {
    let Some(hold) = content.text.as_mut().filter(|hold| !hold.restyling) else { return };
    let super::ass::Content::Script { bytes, .. } = &content.source.content else { return };
    let (id, base_generation, header_len, bytes, plain) =
        (content.source.id, hold.style_generation, hold.header_len, bytes.clone(), hold.plain.clone());
    let (text_script, aligned, alignment_revision) = (hold.script.clone(), hold.aligned.clone(), hold.alignment_revision);
    let alignment_epoch = hold.alignment_epoch;
    // The estimate follows the picks alone: a restyle for new words keeps the one built for these
    // picks, so it is neither rebuilt nor handed to the renderer as a new revision to parse.
    let estimate_held = hold.estimated.is_some() && base_generation == style.generation();
    hold.restyling = true;
    let started = plx_base::task::spawn_small("sidecar restyle", move || {
        let header = text_ass::header(&style);
        let held: &str = match &plain {
            Some(plain) => plain,
            None => std::str::from_utf8(&bytes[header_len..]).unwrap_or(""),
        };
        let revealing = style.layout.reveal != plx_plex::plex::session::Reveal::Off;
        let (estimated, script, cue_timings) = if revealing {
            let params = super::reveal::Params::of(&style.layout);
            let estimated = (!estimate_held).then(|| {
                let (events, timings) = text_script.aligned_events(params, &std::collections::BTreeMap::new());
                (joined(header.as_bytes(), events.as_bytes()), timings)
            });
            let (events, cue_timings) = text_script.aligned_events(params, &aligned);
            (estimated, joined(header.as_bytes(), events.as_bytes()), cue_timings)
        } else {
            let script = joined(header.as_bytes(), held.as_bytes());
            ((!estimate_held).then(|| (script.clone(), Vec::new())), script, Vec::new())
        };
        let plain = revealing.then(|| plain.clone().unwrap_or_else(|| Arc::from(held)));
        let stamped = (revealing && !aligned.is_empty()).then_some(alignment_epoch);
        drop(bytes);
        let current_style = TextStyle::current();
        let mut st = state();
        if let Some((_, content)) = st.loaded.as_mut() {
            publish_restyle(content, id, base_generation, alignment_revision, stamped, style, header.len(), script, cue_timings, estimated, plain);
            if content.text.as_ref().is_some_and(|hold| hold.alignment_revision != hold.published_alignment_revision) {
                start_restyle(content, current_style);
            }
        }
        SETTLED.notify_all();
        drop(st);
        plx_machine::present::wake_from_worker();
    });
    if !started {
        super::log("sidecar: restyle worker refused");
        hold.restyling = false;
        SETTLED.notify_all();
    }
}

/// Publish a script built for `style` if `content` is still the file it was built from. The
/// fonts are the ones the content holds now, so a restyle never undoes a font publication.
/// `estimated` is `None` when the estimate held was already built for `style`.
fn publish_restyle(
    content: &mut Content,
    id: u64,
    base_generation: u32,
    alignment_revision: u64,
    alignment_epoch: Option<u64>,
    style: TextStyle,
    header_len: usize,
    script: Arc<[u8]>,
    cue_timings: Vec<super::ass::CueTiming>,
    estimated: Option<(Arc<[u8]>, Vec<super::ass::CueTiming>)>,
    plain: Option<Arc<str>>,
) {
    let (revision, origin, fonts) = match (&content.source.content, content.text.as_mut()) {
        (super::ass::Content::Script { fonts, .. }, Some(hold)) if content.source.id == id => {
            hold.restyling = false;
            if hold.style_generation != base_generation || hold.alignment_revision != alignment_revision {
                return;
            }
            (hold.header_len, hold.style_generation, hold.plain) = (header_len, style.generation(), plain);
            hold.published_alignment_revision = alignment_revision;
            if let Some((estimated, estimated_timings)) = estimated {
                let estimate_id = hold.estimated.as_ref().map_or_else(super::ass::next_source_id, |source| source.id);
                let estimate_revision = hold.estimated.as_ref().map_or(1, |source| source.revision.wrapping_add(1));
                hold.estimated = Some(Arc::new(super::ass::Source { id: estimate_id, revision: estimate_revision,
                    alignment_epoch: None,
                    cue_timings: estimated_timings.into(),
                    origin: super::ass::Origin::Text(style), content: super::ass::Content::Script {
                        bytes: estimated, fonts: fonts.clone() } }));
            }
            (content.source.revision.wrapping_add(1), super::ass::Origin::Text(style), fonts.clone())
        }
        _ => return,
    };
    content.source = Arc::new(super::ass::Source {
        alignment_epoch,
        cue_timings: cue_timings.into(),
        id, revision, origin, content: super::ass::Content::Script { bytes: script, fonts },
    });
}

/// `head` then `tail` as one shared buffer, allocated once at its exact size (the iterator is
/// exact-sized, so no intermediate `Vec` is built and copied).
fn joined(head: &[u8], tail: &[u8]) -> Arc<[u8]> {
    let mut script = Vec::with_capacity(head.len() + tail.len());
    script.extend_from_slice(head);
    script.extend_from_slice(tail);
    script.into()
}

fn is_ass_script(text: &str) -> bool {
    text.lines().any(|line| line.trim().eq_ignore_ascii_case("[Events]"))
        && text.lines().any(|line| line.trim_start().get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Dialogue:")))
}

/// Whether a text sidecar is WebVTT: it says so in its first line, or the server named the codec.
fn is_webvtt(text: &str, codec: &str) -> bool {
    codec.eq_ignore_ascii_case("vtt")
        || codec.eq_ignore_ascii_case("webvtt")
        || text.trim_start_matches('\u{feff}').starts_with("WEBVTT")
}

fn decode(bytes: Vec<u8>, codec: &str) -> Result<Content, Failure> {
    if bytes.len() > plx_plex::plex::SIDECAR_MAX_BYTES { return Err(Failure::Empty); }
    // The check's lossy copy (a whole file when it is not UTF-8) is gone before anything else is built.
    let authored = { let text = String::from_utf8_lossy(&bytes); is_ass_script(&text) };
    if authored {
        // Validate only the presence of events. libass owns the script grammar, including
        // style/format declarations, overlapping dialogue and vector drawing events.
        return Ok(Content {
            source: Arc::new(super::ass::Source {
                id: super::ass::next_source_id(), revision: 1, origin: super::ass::Origin::Authored,
                alignment_epoch: None,
                cue_timings: Arc::from([]),
                content: super::ass::Content::Script { bytes: bytes.into(), fonts: Arc::from([]) },
            }),
            font_generation: None,
            text: None,
        });
    }
    // A server conversion or error must not silently turn an ASS selection into plain text.
    if codec.eq_ignore_ascii_case("ass") || codec.eq_ignore_ascii_case("ssa") {
        return Err(Failure::Empty);
    }
    let source_sha256: String = plx_base::sha256::sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    // A sidecar file is whole-file text of unknown language here: the charset step is the BOM,
    // valid UTF-8, else the encoding `chardetng` guesses from the whole file (`sub_charset`).
    let text = super::sub_charset::subtitle_text_whole(&bytes, &super::sub_charset::TrackCharset::unknown());
    // A NUL is never subtitle text: padding is dropped, as the line readers always did.
    let text = if text.contains('\0') { std::borrow::Cow::Owned(text.replace('\0', "")) } else { text };
    let text_ref = text.trim_start_matches('\u{feff}');
    if is_ass_script(text_ref) {
        return Err(Failure::Empty);
    }
    let converted = if is_webvtt(text_ref, codec) { text_ass::script_from_vtt(text_ref) } else { text_ass::script_from_srt(text_ref) };
    // Only the converted events are needed from here: the decoded text and the file go first.
    drop(text);
    drop(bytes);
    if converted.cues == 0 {
        return Err(Failure::Empty);
    }
    let style = TextStyle::current();
    let header = text_ass::header(&style);
    let revealing = style.layout.reveal != plx_plex::plex::session::Reveal::Off;
    let (script, cue_timings) = if revealing {
        let (events, timings) = converted.aligned_events(super::reveal::Params::of(&style.layout), &std::collections::BTreeMap::new());
        (joined(header.as_bytes(), events.as_bytes()), timings)
    } else {
        (joined(header.as_bytes(), converted.events.as_bytes()), Vec::new())
    };
    let plain = revealing.then(|| Arc::<str>::from(converted.events.as_str()));
    Ok(Content {
        source: Arc::new(super::ass::Source {
            id: super::ass::next_source_id(), revision: 1, origin: super::ass::Origin::Text(style),
            alignment_epoch: None,
            cue_timings: cue_timings.into(),
            content: super::ass::Content::Script { bytes: script, fonts: Arc::from([]) },
        }),
        font_generation: None,
        text: Some(TextHold { header_len: header.len(), style_generation: style.generation(), cues: converted.cues, plain,
            source_sha256, script: Arc::new(converted), aligned: Arc::new(std::collections::BTreeMap::new()), verified_source_revision: None,
            alignment_revision: 0, published_alignment_revision: 0, alignment_epoch: super::alignment::epoch(),
            estimated: None, restyling: false }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::ass::{self, Origin};
    use super::super::ass_script::{ScriptIndex, WINDOW_BYTES, WINDOW_EVENTS};
    use plx_plex::plex::session::{SubtitlePosition, SubtitleSize};
    use std::sync::atomic::Ordering::Relaxed;

    /// A backstop against a hung worker only: the waits below end on the worker's own signal, so a
    /// loaded runner just takes longer. Hitting it means the worker never finished.
    const WORKER_HUNG: Duration = Duration::from_secs(120);

    /// Block until the download worker has gone idle ([`SETTLED`]), however long the host takes.
    fn finish_download() {
        let (st, timeout) = SETTLED
            .wait_timeout_while(state(), WORKER_HUNG, |st| st.running)
            .unwrap_or_else(|e| e.into_inner());
        assert!(!timeout.timed_out() && !st.running, "sidecar worker finished");
    }

    /// Block until no restyle worker is building a script for the loaded file ([`SETTLED`]).
    fn finish_restyle() {
        let (_st, timeout) = SETTLED
            .wait_timeout_while(state(), WORKER_HUNG, |st| {
                st.loaded.as_ref().and_then(|(_, c)| c.text.as_ref()).is_some_and(|hold| hold.restyling)
            })
            .unwrap_or_else(|e| e.into_inner());
        assert!(!timeout.timed_out(), "sidecar restyle worker finished");
    }

    const STYLED_SCRIPT: &[u8] = b"[Script Info]\nScriptType: v4.00+\n[Events]\n\
        Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,{\\pos(300,100)\\c&H0000FF&}SIGN\n";

    /// The script bytes of a loaded file.
    fn script_bytes(content: &Content) -> Arc<[u8]> {
        let ass::Content::Script { bytes, .. } = &content.source.content else { panic!("a sidecar is a script") };
        bytes.clone()
    }

    /// The `Dialogue:` lines of a loaded file, as libass is given them.
    fn dialogue(content: &Content) -> Vec<String> {
        String::from_utf8(script_bytes(content).to_vec()).unwrap()
            .lines().filter(|l| l.starts_with("Dialogue:")).map(String::from).collect()
    }

    /// The text field of each line.
    fn texts(content: &Content) -> Vec<String> {
        dialogue(content).iter().map(|l| l.splitn(10, ',').nth(9).unwrap().to_string()).collect()
    }

    fn converted(body: &str) -> Content {
        decode(body.as_bytes().to_vec(), "srt").expect("a usable file")
    }

    #[test]
    fn nul_bytes_in_a_text_sidecar_do_not_fail_the_whole_file() {
        let body = "1\n00:00:01,000 --> 00:00:02,000\nHel\0lo\n\n2\n00:00:03,000 --> 00:00:04,000\nWorld\n\0\0\0";
        let content = converted(body);
        assert_eq!(texts(&content), ["Hello", "World"]);
    }

    #[test]
    fn styled_sidecar_source_survives_seek_and_off_on_but_not_new_items() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let server = plx_plex::plex::ServerId::from_raw(0);
        select_with_fetch(server, 42, "/library/streams/42".into(), "ass".into(),
            |_, _, _| Some(STYLED_SCRIPT.to_vec()));
        finish_download();
        let first = ass_source(false).expect("native script available");
        assert_eq!(first.origin, Origin::Authored, "an authored script keeps its own look");
        assert!(ass_source(true).is_none(), "a server burn silences the native script");
        for _seek in 0..3 {
            assert_eq!(notice(), None, "a script has no notice");
            assert_eq!(ass_source(false).unwrap().id, first.id, "seek keeps the whole script");
        }
        deselect();
        assert!(!selected());
        assert!(ass_source(false).is_none());
        select_with_fetch(server, 42, "/library/streams/42".into(), "ASS".into(),
            |_, _, _| panic!("the same cached script must not be downloaded again"));
        assert!(Arc::ptr_eq(&ass_source(false).unwrap(), &first));
        reset();
        assert!(ass_source(false).is_none(), "new item drops the old source");
    }

    #[test]
    fn styled_sidecar_adopts_font_attachments_that_arrive_after_its_download() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        super::super::ass_source::reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42,
            "/library/streams/42".into(), "ass".into(), |_, _, _| Some(STYLED_SCRIPT.to_vec()));
        finish_download();
        let before = ass_source(false).unwrap();
        let font = super::super::ass::Font { name: "sign.ttf".into(), data: Arc::from(&b"font"[..]) };
        // Fonts belong to the media even when it has no embedded ASS track.
        super::super::ass_source::begin("fixture", Vec::new(), vec![font]);
        let after = ass_source(false).unwrap();
        assert_eq!(after.id, before.id, "the script's identity is stable");
        assert!(after.revision > before.revision, "new fonts invalidate native rasterization");
        let super::super::ass::Content::Script { bytes: old_bytes, fonts: old_fonts } = &before.content else { panic!() };
        let super::super::ass::Content::Script { bytes, fonts } = &after.content else { panic!() };
        assert!(old_fonts.is_empty());
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].name, "sign.ttf");
        assert!(Arc::ptr_eq(bytes, old_bytes), "script bytes must not be recopied per font update");
        assert!(Arc::ptr_eq(fonts, &super::super::ass_source::font_snapshot().1));
        assert!(Arc::ptr_eq(&ass_source(false).unwrap(), &after), "unchanged frames reuse the source");
        reset();
        super::super::ass_source::reset();
    }

    #[test]
    fn identical_stream_ids_on_different_servers_do_not_reuse_the_sidecar() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let key = "/library/streams/42";
        for server in [0, 1] {
            select_with_fetch(plx_plex::plex::ServerId::from_raw(server), 42, key.into(), "srt".into(),
                move |_, _, _| Some(format!("00:00:01 --> 00:00:03\nserver {server}\n").into_bytes()));
            finish_download();
            let source = ass_source(false).expect("the converted file");
            let ass::Content::Script { bytes, .. } = &source.content else { panic!() };
            let script = String::from_utf8_lossy(bytes);
            assert!(script.contains(&format!(",server {server}\n")), "{script}");
            assert!(!script.contains(&format!("server {}", 1 - server)));
        }
        reset();
    }

    #[test]
    fn sidecar_picks_share_one_worker_and_drop_abandoned_answers() {
        let _guard = plx_base::testlock::serial();
        reset();
        // A prior route test may have left its now-abandoned worker finishing a no-client
        // result. Let that worker retire before installing this test's controlled transport.
        finish_download();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Arc::new(Mutex::new(release_rx));
        let fetch = {
            let calls = calls.clone();
            move |_: plx_plex::plex::ServerId, key: &str, _: &str| {
                let n = calls.fetch_add(1, Relaxed);
                started_tx.send(()).unwrap();
                if n == 0 { release_rx.lock().unwrap().recv().unwrap(); }
                Some(format!("00:00:01 --> 00:00:03\n{key}\n").into_bytes())
            }
        };
        let sid = plx_plex::plex::ServerId::from_raw(0);
        select_with_fetch(sid, 1, "old".into(), "srt".into(), fetch.clone());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(activation(sid, 1), crate::route::SubtitleActivation::Loading);
        assert_eq!(activation(sid, 2), crate::route::SubtitleActivation::Inactive);
        assert_eq!(activation(plx_plex::plex::ServerId::from_raw(1), 1), crate::route::SubtitleActivation::Inactive);
        reset();
        select_with_fetch(sid, 1, "new".into(), "srt".into(), fetch);
        assert_eq!(activation(sid, 1), crate::route::SubtitleActivation::Loading);
        let parallel = started_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        release_tx.send(()).unwrap();
        let text_of = || ass_source(false).map(|s| {
            let ass::Content::Script { bytes, .. } = &s.content else { panic!() };
            String::from_utf8_lossy(bytes).into_owned()
        });
        finish_download();
        let script = text_of().expect("the latest pick landed");
        assert!(script.ends_with(",new\n"), "{script}");
        assert!(!script.contains(",old\n"));
        assert_eq!(activation(sid, 1), crate::route::SubtitleActivation::Loading, "a downloaded file is not a rendered frame");
        reset();
        assert!(!parallel, "an abandoned fetch must finish before another worker is started");
    }

    #[test]
    fn sidecar_download_refuses_an_oversized_body() {
        let mut body = b"00:00:01 --> 00:00:03\n".to_vec();
        body.resize(4 * 1024 * 1024 + 1, b'x');
        assert!(decode(body.clone(), "srt").is_err(), "oversized sidecars must not allocate a converted copy");
        assert!(decode(body, "ass").is_err(), "native scripts obey the same download bound");
    }

    /// A cue is never cut: a 10,000-character line reaches the script whole.
    #[test]
    fn sidecar_cue_text_is_whole() {
        let line = "é".repeat(10_000);
        let content = converted(&format!("00:00:01 --> 00:00:03\n{line}"));
        assert_eq!(texts(&content), [line]);
    }

    /// The ordinary file: counters, CRLF, a BOM, italics, a two-line cue — the counter line must
    /// never leak into the cue before it, and times are written to the centisecond.
    #[test]
    fn a_subrip_file_becomes_clean_sorted_events() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Hello</i>\r\nthere\r\n\r\n\
                   2\r\n00:01:00,250 --> 00:01:02,000\r\n{\\an8}Second line\r\n";
        let content = converted(srt);
        assert_eq!(content.text.as_ref().unwrap().cues, 2);
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\i1}Hello{\\i0}\\Nthere",
            "Dialogue: 0,0:01:00.25,0:01:02.00,An8,,0,0,0,,Second line",
        ]);
        assert_eq!(content.source.origin, Origin::Text(TextStyle::current()));
        let script = String::from_utf8(script_bytes(&content).to_vec()).unwrap();
        assert!(script.starts_with(&text_ass::header(&TextStyle::current())), "the synthesized header comes first");
    }

    /// PMS is asked for SubRip but may answer in whatever it has. WebVTT differs in a header,
    /// `.` before the milliseconds with the hours optional, cue settings after the end time and
    /// blocks that are not cues; none may cost a cue or reach the text, and `line:0` puts the cue
    /// at the top.
    #[test]
    fn a_webvtt_file_converts_and_line_zero_is_the_top() {
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n01:02.500 --> 01:05.000 line:0 position:50%\nHi\n\n\
                   1:00:00.000 --> 1:00:01.000\nAn hour in\n\n\
                   00:00:10.000 --> 00:00:11.000 line:-1\nBottom\n";
        let content = converted(vtt);
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:10.00,0:00:11.00,Default,,0,0,0,,Bottom",
            "Dialogue: 0,0:01:02.50,0:01:05.00,An8,,0,0,0,,Hi",
            "Dialogue: 0,1:00:00.00,1:00:01.00,Default,,0,0,0,,An hour in",
        ]);
        // the codec the server names is enough when the first line does not say so
        let bare = decode(b"00:01.000 --> 00:02.000 line:0\nTop\n".to_vec(), "vtt").unwrap();
        assert_eq!(dialogue(&bare), ["Dialogue: 0,0:00:01.00,0:00:02.00,An8,,0,0,0,,Top"]);
    }

    /// A merged feature-length file is well past any one window: every cue is converted, and the
    /// script is fed to libass in windows that hold the first cue, cue 25,000 and the last.
    #[test]
    fn a_long_file_converts_whole_and_is_fed_in_windows() {
        const N: i64 = 30_000;
        let stamp = |secs: i64| format!("{:02}:{:02}:{:02},000", secs / 3600, secs / 60 % 60, secs % 60);
        let mut body = String::new();
        for k in 0..N {
            // Cue k runs over [2k s, 2k s + 1 s) and says "cue k".
            body.push_str(&format!("{}\n{} --> {}\ncue {k}\n\n", k + 1, stamp(2 * k), stamp(2 * k + 1)));
        }
        let content = converted(&body);
        assert_eq!(content.text.as_ref().unwrap().cues, N as usize);
        let index = ScriptIndex::parse(&script_bytes(&content)).unwrap();
        assert_eq!(index.len(), N as usize);
        assert!(index.windowed());
        for k in [0, 25_000, N - 1] {
            let window = index.window(2 * k * 1000 + 500);
            assert!(window.covers(2 * k * 1000 + 500));
            let held = String::from_utf8_lossy(&window.text).into_owned();
            assert!(held.contains(&format!(",cue {k}\n")), "cue {k} is in its window");
            assert!(held.starts_with(&text_ass::header(&TextStyle::current())));
            assert!(held.matches("Dialogue:").count() <= WINDOW_EVENTS);
        }
    }

    /// The worst case the size rule exists for: a 4 MiB (`SIDECAR_MAX_BYTES`) Thai SubRip file in
    /// TIS-620 is one byte per letter and three once decoded to UTF-8, so a converted script is
    /// well past the 8 MiB a script loaded whole may be. It must convert, be fed in windows, and
    /// every window must stay within the bound. Two shapes: many ordinary cues, and a handful of
    /// enormous ones (more bytes than a window but fewer lines than one).
    #[test]
    fn a_4_mib_tis620_file_converts_and_windows_within_the_bound() {
        let thai = "สวัสดีครับ ผมชื่อสมชาย ยินดีที่ได้รู้จักทุกคน ";
        let (tis, _, bad) = encoding_rs::WINDOWS_874.encode(thai);
        assert!(!bad);
        let cue = |k: usize, repeats: usize| {
            let at = |s: usize| format!("{:02}:{:02}:{:02},000", s / 3600, s / 60 % 60, s % 60);
            let mut out = format!("{}\r\n{} --> {}\r\n", k + 1, at(k * 2), at(k * 2 + 1)).into_bytes();
            for _ in 0..repeats { out.extend_from_slice(&tis); }
            out.extend_from_slice(b"\r\n\r\n");
            out
        };
        let limit = plx_plex::plex::SIDECAR_MAX_BYTES;
        for (name, repeats) in [("many small cues", 2), ("few enormous cues", 10_000)] {
            let mut file = Vec::new();
            let mut k = 0;
            loop {
                let next = cue(k, repeats);
                if file.len() + next.len() > limit { break; }
                file.extend_from_slice(&next);
                k += 1;
            }
            assert!(file.len() <= limit && file.len() > limit - 512 * 1024, "{name}: {} bytes", file.len());
            let content = decode(file, "srt").unwrap_or_else(|why| panic!("{name}: {why:?}"));
            assert!(texts(&content)[0].contains("สวัสดีครับ"), "{name}: decoded as Thai, not mojibake");
            let bytes = script_bytes(&content);
            assert!(bytes.len() > WINDOW_BYTES, "{name}: {} bytes after conversion is the case under test", bytes.len());
            assert_eq!(ass::validate(&content.source), Ok(()), "{name}: a windowed script is bounded by its source");
            let index = ScriptIndex::parse(&bytes).unwrap();
            assert!(index.windowed(), "{name}");
            // walk it: every window is within the bound and covers its playhead; together they hold every cue
            let mut now = 0;
            let mut seen = std::collections::BTreeSet::new();
            loop {
                let window = index.window(now);
                assert!(window.text.len() <= WINDOW_BYTES, "{name}: a {} byte window", window.text.len());
                assert!(window.covers(now), "{name}");
                let held = String::from_utf8_lossy(&window.text).into_owned();
                for line in held.lines().filter(|l| l.starts_with("Dialogue:")) {
                    seen.insert(line.splitn(4, ',').nth(1).unwrap().to_string());
                }
                if window.hi_ms == i64::MAX { break; }
                now = window.hi_ms;
            }
            assert_eq!(seen.len(), index.len(), "{name}: every cue is in some window");
        }
    }

    /// Out-of-order files exist (merged fan subs); the script is in start order. A cue that ends
    /// before it starts, or says nothing, is dropped rather than drawn.
    #[test]
    fn events_are_sorted_and_the_unusable_cues_dropped() {
        let srt = "2\n00:00:10,000 --> 00:00:11,000\nlater\n\n1\n00:00:01,000 --> 00:00:02,000\nsooner\n\n\
                   3\n00:00:05,000 --> 00:00:04,000\nbackwards\n\n4\n00:00:20,000 --> 00:00:21,000\n<i></i>\n";
        assert_eq!(texts(&converted(srt)), ["sooner", "later"]);
    }

    /// Where two cues overlap the newer one wins: the older ends where the newer starts.
    #[test]
    fn an_overlap_is_clipped_to_the_newest_cue() {
        let content = converted("1\n00:00:01,000 --> 00:00:04,000\na\n\n2\n00:00:02,000 --> 00:00:04,000\nb\n");
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,a",
            "Dialogue: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,,b",
        ]);
    }

    /// Garbage in, nothing out — an HTML error page or an image subtitle must not panic and must
    /// not produce a script, because that is what turns into the on-screen explanation.
    #[test]
    fn a_body_that_is_not_a_subtitle_yields_no_script() {
        for body in [
            &b"<html><body>501 Not Implemented</body></html>"[..],
            &[0xff, 0xfe, 0x00, 0x01, 0x02],
            b"",
            b"1\n99:99:99,999 --> nonsense\ntext\n",
        ] {
            assert!(matches!(decode(body.to_vec(), "srt"), Err(Failure::Empty)), "{body:?}");
        }
    }

    #[test]
    fn styled_script_keeps_headers_overrides_and_overlapping_events() {
        let script = b"[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\nStyle: Sign,Example\n[Events]\n\
            Dialogue: 0,0:00:01.00,0:00:03.00,Sign,,0,0,0,,{\\pos(300,100)}SIGN\n\
            Dialogue: 1,0:00:01.00,0:00:03.00,Default,,0,0,0,,{\\k20}Dialogue\n";
        for codec in ["ass", "SSA"] {
            let content = decode(script.to_vec(), codec).unwrap();
            assert_eq!(script_bytes(&content).as_ref(), script);
            assert_eq!(content.source.origin, Origin::Authored);
            assert!(content.text.is_none(), "an authored script is never restyled");
        }
        assert!(decode(b"1\n00:00:01,000 --> 00:00:03,000\nSIGN\n".to_vec(), "ass").is_err(),
            "a converted response must not silently discard styling");
    }

    /// #708's legacy charsets reach the script as the same UTF-8 text the embedded path gets.
    #[test]
    fn legacy_charsets_convert_to_the_expected_event_body() {
        for (encoding, line) in [
            (encoding_rs::GBK, "你好，世界。今天天气很好，我们一起去公园散步吧。"),
            (encoding_rs::SHIFT_JIS, "こんにちは、世界。今日はいい天気ですね。一緒に散歩に行きましょう。"),
            (encoding_rs::EUC_KR, "안녕하세요, 세계. 오늘 날씨가 정말 좋네요. 같이 산책하러 가요."),
        ] {
            let (bytes, _, bad) = encoding.encode(line);
            assert!(!bad);
            let mut file = b"1\n00:00:01,000 --> 00:00:03,000\n".to_vec();
            file.extend_from_slice(&bytes);
            file.push(b'\n');
            let content = decode(file, "srt").expect("a usable file");
            assert_eq!(texts(&content), [line], "{}", encoding.name());
        }
    }

    /// Another pair of picks than `current`, in both size and position.
    fn other_picks(current: TextStyle) -> TextStyle {
        TextStyle::picks(if current.size == SubtitleSize::Large { SubtitleSize::Small } else { SubtitleSize::Large }, if current.position == SubtitlePosition::High { SubtitlePosition::Low } else { SubtitlePosition::High })
    }

    /// The source for `picks`, once the worker that builds a restyled script has published it.
    fn restyled_source(picks: TextStyle) -> Arc<ass::Source> {
        finish_restyle();
        ass_source_styled(false, picks).expect("the converted file")
    }

    /// A pick change must cost the frame thread nothing in proportion to the file: the call that
    /// notices it returns the source already up (the old look stays until the new script is built
    /// elsewhere), and the new one arrives with the events untouched and the revision bumped once.
    #[test]
    fn a_pick_change_returns_the_held_source_at_once_and_publishes_the_restyle_later() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let body: String = (0..20_000).map(|i| format!(
            "{n}\n00:{m:02}:{s:02},000 --> 00:{m:02}:{e:02},000\nA cue of some length number {n}\n\n",
            n = i + 1, m = (i / 30) % 60, s = (i % 30) * 2 % 60, e = ((i % 30) * 2 + 1) % 60)).collect();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 8, "/library/streams/8".into(), "srt".into(),
            move |_, _, _| Some(body.clone().into_bytes()));
        finish_download();
        let current = TextStyle::current();
        let picks = other_picks(current);
        let before = ass_source_styled(false, current).unwrap();
        let during = ass_source_styled(false, picks).unwrap();
        assert!(Arc::ptr_eq(&before, &during), "the old look is still up when the pick lands");
        let after = restyled_source(picks);
        assert_eq!((after.id, after.revision, after.origin), (before.id, before.revision.wrapping_add(1), Origin::Text(picks)));
        let (ass::Content::Script { bytes: old, .. }, ass::Content::Script { bytes: new, .. }) =
            (&before.content, &after.content) else { panic!() };
        assert_eq!(&old[text_ass::header(&current).len()..], &new[text_ass::header(&picks).len()..]);
        assert!(Arc::ptr_eq(&after, &ass_source_styled(false, picks).unwrap()), "and once only");
        reset();
    }

    /// Host timing of the restyle on the TV's repro file (4 MiB TIS-620, 35,735 cues). Run by hand:
    /// `cargo test -p plxnative-media --lib restyle_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn restyle_timing() {
        let bytes = std::fs::read("/tmp/libass7b-fx/sidecar-big-tis620.srt").expect("the fixture");
        let content = decode(bytes, "srt").expect("a usable file");
        let ass::Content::Script { bytes: script, .. } = &content.source.content else { panic!() };
        let hold = content.text.as_ref().unwrap();
        let header = text_ass::header(&other_picks(TextStyle::current()));
        let t0 = Instant::now();
        let rebuilt = joined(header.as_bytes(), &script[hold.header_len..]);
        println!("restyle of {} bytes, {} cues: {:?}", rebuilt.len(), hold.cues, t0.elapsed());
        let t0 = Instant::now();
        let bytewise: Arc<[u8]> = header.as_bytes().iter().chain(&script[hold.header_len..]).copied().collect();
        println!("the byte-wise join it replaced: {:?} ({} bytes)", t0.elapsed(), bytewise.len());
    }

    /// The failure notice is all that is left of the old caption path: shown for six seconds in
    /// the caption's own place (the caller silences it where the server burns the selection).
    #[test]
    fn the_failure_notice_is_shown_for_six_seconds() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        assert_eq!((notice(), notice_shown()), (None, false), "nothing selected");
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 9, "/library/streams/9".into(), "srt".into(),
            |_, _, _| Some(b"<html>not a subtitle</html>".to_vec()));
        finish_download();
        assert_eq!(activation(plx_plex::plex::ServerId::from_raw(0), 9), crate::route::SubtitleActivation::Failed);
        let message = plx_platform::i18n::msg::widgets_sidecar_empty();
        assert_eq!(notice(), Some(message));
        assert!(notice_shown());
        assert!(ass_source(false).is_none(), "and no script");
        // six seconds on
        {
            let mut st = state();
            let (_, _, at) = st.failed.as_mut().unwrap();
            *at = Instant::now().checked_sub(FAILURE_SHOWN + Duration::from_millis(1)).unwrap();
        }
        assert_eq!((notice(), notice_shown()), (None, false));
        assert_eq!(activation(plx_plex::plex::ServerId::from_raw(0), 9), crate::route::SubtitleActivation::Failed, "failure outlives the notice");
        // a fetch failure says its own thing
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 10, "/library/streams/10".into(), "srt".into(),
            |_, _, _| None);
        finish_download();
        assert_eq!(activation(plx_plex::plex::ServerId::from_raw(0), 10), crate::route::SubtitleActivation::Failed);
        assert_eq!(notice(), Some(plx_platform::i18n::msg::widgets_sidecar_fetch_failed()));
        deselect();
        assert_eq!(notice(), None, "off clears it");
        reset();
    }

    /// Issue #266 I6: the route withholds the audio enhancement exactly when this restore would
    /// put a sidecar on screen, so the two must read ONE predicate
    /// (`metadata::server_selected_sidecar`). Graded against every shape the predicate splits on:
    /// a drawable selected sidecar is restored and is the shared answer; an unselected one, a
    /// keyless one, a bitmap one and a selected EMBEDDED track are all nothing to both.
    #[test]
    fn restore_uses_shared_server_selected_sidecar() {
        let _guard = plx_base::testlock::serial();
        let sidecar = |id: i64, codec: &str, key: &str, selected: bool, external: bool| plx_data::metadata::Stream {
            id,
            codec: codec.into(),
            key: key.into(),
            selected,
            external,
            ..Default::default()
        };
        let cases: [(Vec<plx_data::metadata::Stream>, Option<i64>); 5] = [
            (vec![sidecar(1, "srt", "/library/streams/1", false, true), sidecar(2, "srt", "/library/streams/2", true, true)], Some(2)),
            (vec![sidecar(3, "srt", "/library/streams/3", false, true)], None),
            (vec![sidecar(4, "srt", "", true, true)], None),
            (vec![sidecar(5, "pgs", "/library/streams/5", true, true)], None),
            (vec![sidecar(6, "srt", "", true, false)], None),
        ];
        for (subs, want) in cases {
            let ids: Vec<i64> = subs.iter().map(|s| s.id).collect();
            let item = plx_data::metadata::PlayingItem::with_subs(subs);
            assert_eq!(plx_data::metadata::server_selected_sidecar(&item).map(|s| s.id), want, "{ids:?}");
            let mut store = plx_data::stores::metadata::MetadataStore::default();
            assert!(store.run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
            assert_eq!(restore_selection(plx_plex::plex::ServerId::UNSET, store.view(), None), want, "{ids:?}");
            finish_download();
            deselect();
        }
    }

    fn render_selected_ready() {
        let source = ass_source(false).unwrap();
        let deadline = Instant::now() + WORKER_HUNG;
        loop {
            ass::request(source.clone(), 1_500, 320, 180, 320, 180);
            if ass::source_revision_readiness(source.id, source.revision) == ass::SourceReadiness::Ready { return; }
            assert!(Instant::now() < deadline, "renderer did not publish the selected source");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn alignment_prepare_preserves_authored_bounds_with_offsets() {
        use super::super::alignment::{self, Identity, PlaybackSnapshot};
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,009 --> 00:00:01,800\nfirst\n\n2\n00:00:02,000 --> 00:00:20,000\nlong\n\n3\n00:00:05,000 --> 00:00:06,000\nlast\n".to_vec()));
        finish_download();
        render_selected_ready();
        let parsed = alignment_snapshot(0, 30_000, 32).unwrap();
        assert_eq!((parsed.cues[0].authored_start_ms, parsed.cues[0].start_ms), (1009, 1000));
        assert_eq!((parsed.cues[1].authored_end_ms, parsed.cues[1].end_ms), (20_000, 5000));
        let mut snapshot = PlaybackSnapshot { endpoint: "http://localhost".into(), playback_id: "playback".into(),
            identity: Identity { server_machine_id: "machine".into(), rating_key: "1".into(), media_id: "2".into(),
                part_id: "3".into(), part_key: "/part".into(), audio_stream_id: "4".into(),
                subtitle_stream_id: "42".into(), subtitle_codec: "srt".into() },
            subtitle_source_kind: "sidecar".into(), language: "en".into(), subtitle_offset_ms: 0,
            position_ms: 15_000, track: 0, style_generation: 1 };
        for offset in [-300, 0, 300] {
            snapshot.subtitle_offset_ms = offset;
            let request = alignment::prepare_request_for_test(&snapshot).unwrap();
            assert_eq!(request.subtitle_offset_ms, offset);
            assert_eq!(request.cues.len(), 2, "long authored cue must not poison the window");
            assert_eq!(request.cues.iter().map(|cue| cue.ordinal).collect::<Vec<_>>(), [0, 2]);
            let first = &request.cues[0];
            assert_eq!((first.authored_start_ms, first.authored_end_ms), (1009, 1800));
            assert_eq!((first.start_ms - offset, first.end_ms - offset), (1009, 1800));
        }
        snapshot.subtitle_offset_ms = 0;
        for (position, start) in [(-1, 0), (0, 0), (14_999, 0), (15_000, 0), (16_000, 1_000)] {
            snapshot.position_ms = position;
            let request = alignment::prepare_request_for_test(&snapshot).expect("early cue is eligible");
            assert_eq!((request.window_start_ms, request.window_end_ms), (start, start + 30_000));
            assert!(request.cues.iter().any(|cue| cue.ordinal == 0));
        }
        reset();
        ass::clear();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn regression_sidecar_fences_estimates_and_source_revisions() {
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:04,000\nhello\n\n2\n00:00:06,000 --> 00:00:09,000\nworld\n".to_vec()));
        finish_download();
        let word = super::super::reveal::AlignedWord { start_byte: 0, end_byte: 5, start_ms: 1100, end_ms: 1500 };
        let send = |updates, revision| {
            render_selected_ready();
            let snapshot = alignment_snapshot(0, 10000, 32).unwrap();
            assert!(apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision,
                super::super::alignment::epoch(), updates, revision));
            finish_restyle();
        };
        send(vec![AlignmentUpdate { ordinal: 0, words: vec![word] }], Some("a"));
        send(vec![AlignmentUpdate { ordinal: 1, words: vec![word] }], Some("a"));
        assert_eq!(state().loaded.as_ref().unwrap().1.text.as_ref().unwrap().aligned.len(), 2);
        send(vec![AlignmentUpdate { ordinal: 1, words: vec![] }], Some("a"));
        {
            let st = state();
            let aligned = &st.loaded.as_ref().unwrap().1.text.as_ref().unwrap().aligned;
            assert!(aligned.contains_key(&0));
            assert!(!aligned.contains_key(&1));
        }
        send(vec![AlignmentUpdate { ordinal: 1, words: vec![word] }], Some("b"));
        {
            let st = state();
            let aligned = &st.loaded.as_ref().unwrap().1.text.as_ref().unwrap().aligned;
            assert!(!aligned.contains_key(&0));
            assert!(aligned.contains_key(&1));
        }
        send(vec![AlignmentUpdate { ordinal: 0, words: vec![] }], None);
        assert!(state().loaded.as_ref().unwrap().1.text.as_ref().unwrap().aligned.is_empty());
        reset();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn alignment_snapshot_pins_fetched_bytes_and_authored_and_visible_cues() {
        let _guard = plx_base::testlock::serial();
        reset();
        let body = b"1\r\n00:00:01,000 --> 00:00:05,000\r\n<i>Hello</i> world\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nNext\r\n";
        let expected: String = plx_base::sha256::sha256(body).iter().map(|b| format!("{b:02x}")).collect();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/stream".into(), "srt".into(),
            move |_, _, _| Some(body.to_vec()));
        finish_download();
        render_selected_ready();
        let snapshot = alignment_snapshot(0, 6_000, 1).unwrap();
        assert_eq!(snapshot.stream_id, 42);
        assert_eq!(snapshot.source_sha256, expected);
        assert_eq!(snapshot.cues.len(), 1);
        let cue = &snapshot.cues[0];
        assert_eq!((cue.ordinal, cue.authored_start_ms, cue.authored_end_ms), (0, 1_000, 5_000));
        assert_eq!((cue.start_ms, cue.end_ms), (1_000, 3_000));
        assert_eq!(cue.text, "<i>Hello</i> world");
        invalidate_alignment();
        assert!(!apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), Vec::new(), None));
        finish_restyle();
        reset();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn alignment_rejects_an_old_selection_and_excludes_authored_ass() {
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:02,000\nHello\n".to_vec()));
        finish_download();
        render_selected_ready();
        let snapshot = alignment_snapshot(0, 3_000, 32).unwrap();
        deselect();
        assert!(!apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), Vec::new(), None));
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 43, "/ass".into(), "ass".into(),
            |_, _, _| Some(STYLED_SCRIPT.to_vec()));
        finish_download();
        assert!(alignment_snapshot(0, 3_000, 32).is_none());
        reset();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn accepted_alignment_is_retained_and_invalidated_off_thread() {
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:04,000\nHello world\n".to_vec()));
        finish_download();
        assert!(alignment_snapshot(0, 5_000, 32).is_none(), "loading is not ready");
        render_selected_ready();
        let snapshot = alignment_snapshot(0, 5_000, 32).unwrap();
        assert!(alignment_ready(snapshot.generation, snapshot.source_id, snapshot.source_revision));
        ass::clear();
        assert!(!alignment_ready(snapshot.generation, snapshot.source_id, snapshot.source_revision), "readiness must be rechecked before send");
        assert!(!apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), Vec::new(), None), "loading must not accept results");
        render_selected_ready();
        let word = super::super::reveal::AlignedWord { start_byte: 0, end_byte: 5, start_ms: 1_100, end_ms: 1_500 };
        assert!(apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), vec![AlignmentUpdate { ordinal: 0, words: vec![word] }], Some("test-revision")));
        finish_restyle();
        {
            let st = state();
            let hold = st.loaded.as_ref().unwrap().1.text.as_ref().unwrap();
            assert_eq!(hold.aligned.get(&0).unwrap()[0], word);
            assert_eq!(hold.alignment_revision, hold.published_alignment_revision);
        }
        invalidate_alignment();
        finish_restyle();
        assert!(state().loaded.as_ref().unwrap().1.text.as_ref().unwrap().aligned.is_empty());
        reset();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn alignment_requires_the_exact_selected_published_revision() {
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:04,000\nHello world\n".to_vec()));
        finish_download();
        render_selected_ready();
        let snapshot = alignment_snapshot(0, 5_000, 32).unwrap();
        {
            let mut st = state();
            let content = &mut st.loaded.as_mut().unwrap().1;
            Arc::make_mut(&mut content.source).revision += 1;
        }
        assert!(alignment_snapshot(0, 5_000, 32).is_none(), "an older published revision is not ready");
        assert!(!alignment_ready(snapshot.generation, snapshot.source_id, snapshot.source_revision));
        assert!(!alignment_ready(snapshot.generation, snapshot.source_id.wrapping_add(1), snapshot.source_revision));
        assert!(!apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), Vec::new(), None));
        render_selected_ready();
        let newer = alignment_snapshot(0, 5_000, 32).unwrap();
        assert_eq!(newer.source_id, snapshot.source_id);
        assert_ne!(newer.source_revision, snapshot.source_revision);
        assert!(!alignment_ready(snapshot.generation, snapshot.source_id, snapshot.source_revision));
        assert!(!apply_alignment(snapshot.generation, snapshot.source_id, snapshot.source_revision, super::super::alignment::epoch(), Vec::new(), None));
        reset();
    }

    #[test]
    #[ignore = "requires the pinned native host libass artifact and packaged fonts"]
    fn invalidating_empty_alignment_does_not_restyle() {
        let _guard = plx_base::testlock::serial();
        reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42, "/text".into(), "srt".into(),
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:04,000\nHello\n".to_vec()));
        finish_download();
        render_selected_ready();
        let source = ass_source(false).unwrap();
        invalidate_alignment();
        invalidate_alignment();
        let st = state();
        let content = &st.loaded.as_ref().unwrap().1;
        let hold = content.text.as_ref().unwrap();
        assert!(!hold.restyling);
        assert_eq!(hold.alignment_revision, hold.published_alignment_revision);
        assert!(Arc::ptr_eq(&source, selected_source(content)));
        drop(st);
        reset();
    }

    /// The worker asks again every few seconds of dialogue, mostly for words already held: an answer
    /// that changes no cue's words must not bump the revision that rebuilds the whole script.
    #[test]
    fn an_unchanged_realignment_does_not_bump_the_revision() {
        let mut content = decode(b"1\n00:00:01,000 --> 00:00:04,000\nHello world\n".to_vec(), "srt").unwrap();
        let hold = content.text.as_mut().unwrap();
        let word = super::super::reveal::AlignedWord { start_byte: 0, end_byte: 5, start_ms: 1_100, end_ms: 1_500 };
        let words = || vec![AlignmentUpdate { ordinal: 0, words: vec![word] }];
        assert!(hold.realign(7, words(), Some("a")));
        let revision = hold.alignment_revision;
        assert!(!hold.realign(7, words(), Some("a")), "the same words again");
        assert_eq!(hold.alignment_revision, revision);
        assert!(hold.realign(8, words(), Some("a")), "a new epoch restamps the held words");
        assert_eq!(hold.alignment_revision, revision.wrapping_add(1));
    }
}
