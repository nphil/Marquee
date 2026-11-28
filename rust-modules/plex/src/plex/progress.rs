//! **How far one request has got**, from the three places that know: Seerr (the request and what it
//! holds of the title), Radarr/Sonarr (the title's record and its download queue) and, behind the
//! queue, the download client (NZBGet), which the queue reports on: size, size left, time left and
//! the stage the app tracks.
//!
//! Nothing here is estimated. A percentage is `(size - left) / size` of a download that reported
//! both; an ETA is the app's own time-left (never the wall clock, which is skewed on this TV) and
//! only while bytes are moving; an episode count is the app's own file count. What no source
//! reports is `None`, and the screen says nothing about it.
//!
//! **Exact ids only.** Radarr is asked for `tmdbId`, Sonarr for the `tvdbId` Seerr gives the show;
//! each answer is checked against the id asked for. The app's own id then selects its queue
//! (`/queue/details?movieId=` / `?seriesId=`), so a title is never matched by name. Radarr's and
//! Sonarr's contract is their own source (v3 API, read 2026-11-27): a queue record carries `status`,
//! `trackedDownloadState`, `trackedDownloadStatus`, `size`, `sizeleft`, `timeleft` (a .NET
//! `[d.]hh:mm:ss`), `downloadId` (shared by the episodes of one season pack, so a pack is one download,
//! not one per episode) and `downloadClient`. Seerr's `mediaInfo.downloadStatus` is the same queue as
//! Seerr last synced it (about once a minute) and is used when no app is linked or the app is
//! unreachable.
//!
//! The module is pure but for [`run_arr`] / [`run_list`], which take the apps' [`ArrTransport`]; the
//! [`Tracker`] is the page's state machine and is driven by the frame clock, so its tests need no
//! sleeps. At most one call is outstanding per tracker and nothing is asked while the page is hidden.

use super::arr::{Arr, ArrError, Transport as ArrTransport};
use super::bazarr::{int, text};
use super::seerr::{Have, Kind};
use super::session::{ArrKind, ArrLink};
use serde_json::Value;

// ---- what is known -------------------------------------------------------------------------------

/// A title the Requests area names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: Kind,
    pub tmdb: u32,
}

/// The app that keeps a title's downloads: Radarr for a film, Sonarr for a show.
pub fn arr_of(kind: Kind) -> ArrKind {
    match kind {
        Kind::Movie => ArrKind::Radarr,
        Kind::Tv => ArrKind::Sonarr,
    }
}

/// Where a request stands, from asked to watchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Asked for; waiting for someone to approve it.
    Requested,
    /// Approved; no download has started that anything reports.
    Approved,
    /// In the download client's list, not started.
    Queued,
    Downloading,
    Paused,
    /// Every byte has arrived and the download client is still working on it (checking, unpacking).
    Finishing,
    /// Downloaded; the app is moving it into the library.
    Importing,
    /// In the app's library; Seerr has not yet seen it in Plex.
    Imported,
    /// Some episodes are there, others are not.
    Partial,
    /// The app reports a problem the viewer has to look at.
    Attention,
    Failed,
    Declined,
    /// Seerr says it is all there.
    Available,
}

impl Stage {
    /// What a headline over several parts shows: work in motion first, then what is stuck, then
    /// what waits, then what is done.
    fn rank(self) -> u8 {
        match self {
            Stage::Downloading => 0,
            Stage::Finishing => 1,
            Stage::Importing => 2,
            Stage::Queued => 3,
            Stage::Paused => 4,
            Stage::Attention => 5,
            Stage::Failed => 6,
            Stage::Approved => 7,
            Stage::Requested => 8,
            Stage::Declined => 9,
            Stage::Partial => 10,
            Stage::Imported => 11,
            Stage::Available => 12,
        }
    }

    /// Bytes are moving or about to: ask often.
    fn moving(self) -> bool {
        matches!(self, Stage::Downloading | Stage::Finishing | Stage::Importing | Stage::Queued)
    }

    /// Nothing more will happen unless something outside changes.
    fn settled(self) -> bool {
        matches!(self, Stage::Available | Stage::Declined | Stage::Failed)
    }
}

/// One download as an app's queue reports it. A season pack is one download however many episodes
/// it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub stage: Stage,
    /// Bytes; `0` when the app did not say.
    pub size: u64,
    pub left: Option<u64>,
    /// The app's own time-left, only while the download is moving.
    pub eta_s: Option<u32>,
    /// The seasons its episodes belong to (a show), sorted; empty when the record names none.
    pub seasons: Vec<u32>,
    /// The download client as the app names it ("NZBGet").
    pub client: String,
    identified: bool,
}

impl Download {
    pub fn percent(&self) -> Option<u8> {
        percent(u128::from(self.size), u128::from(self.left?))
    }
}

fn percent(size: u128, left: u128) -> Option<u8> {
    (size > 0 && left <= size).then(|| ((size - left) * 100 / size) as u8)
}

fn bytes(rec: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| {
        let value = rec.get(*k)?;
        value.as_u64().or_else(|| {
            let n = value.as_f64()?;
            (n.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&n)).then_some(n as u64)
        })
    })
}

fn number(v: &Value, key: &str) -> Option<u32> {
    u32::try_from(v.get(key)?.as_i64()?).ok()
}

/// A .NET time span (`[d.]hh:mm:ss[.fffffff]`) in seconds.
fn span(text: &str) -> Option<u32> {
    let (head, rest) = text.trim().split_once(':')?;
    let (days, hours) = match head.split_once('.') {
        Some((d, h)) => (d.parse::<u32>().ok()?, h.parse::<u32>().ok()?),
        None => (0, head.parse::<u32>().ok()?),
    };
    let (minutes, seconds) = rest.split_once(':')?;
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds = seconds.split('.').next()?.parse::<u32>().ok()?;
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    u32::try_from(((u64::from(days) * 24 + u64::from(hours)) * 60 + u64::from(minutes)) * 60 + u64::from(seconds)).ok()
}

impl Download {
    /// One queue record of Radarr or Sonarr, or of Seerr's copy of one (`sizeLeft`, `timeLeft`).
    fn of(rec: &Value) -> Download {
        let size = bytes(rec, &["size"]).unwrap_or(0);
        let left = bytes(rec, &["sizeleft", "sizeLeft"]);
        let status = text(rec, "status").to_ascii_lowercase();
        let state = text(rec, "trackedDownloadState");
        let tracked = text(rec, "trackedDownloadStatus");
        let stage = match (state.as_str(), status.as_str()) {
            ("importBlocked", _) | ("ignored", _) => Stage::Attention,
            ("failedPending" | "failed", _) | (_, "failed") => Stage::Failed,
            ("imported", _) => Stage::Imported,
            _ if tracked == "error" => Stage::Attention,
            ("importPending" | "importing", _) | (_, "completed") => Stage::Importing,
            (_, "paused") => Stage::Paused,
            (_, "downloadclientunavailable" | "warning") => Stage::Attention,
            (_, "downloading") if size > 0 && left == Some(0) => Stage::Finishing,
            (_, "downloading") => Stage::Downloading,
            _ => Stage::Queued,
        };
        let eta_s = (stage == Stage::Downloading)
            .then(|| text_of(rec, &["timeleft", "timeLeft"]).and_then(span))
            .flatten()
            .filter(|s| *s > 0);
        let seasons: Vec<u32> = rec
            .get("episode")
            .and_then(|e| number(e, "seasonNumber"))
            .or_else(|| number(rec, "seasonNumber"))
            .into_iter()
            .collect();
        let identified = rec.get("downloadId").and_then(Value::as_str).is_some_and(|id| !id.is_empty());
        Download { stage, size, left, eta_s, seasons, client: text(rec, "downloadClient"), identified }
    }
}

fn text_of<'a>(rec: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| rec.get(*k)?.as_str())
}

/// Pack records share an exact download id. Without it, keep each record separate and do not
/// aggregate the bytes of unidentifiable records: they might be copies of one pack.
fn group<'a>(records: impl IntoIterator<Item = &'a Value>) -> Vec<Download> {
    let mut keys = Vec::new();
    let mut out: Vec<Download> = Vec::new();
    for rec in records.into_iter().filter(|r| r.is_object()) {
        let one = Download::of(rec);
        let key = rec.get("downloadId").and_then(Value::as_str).filter(|id| !id.is_empty())
            .map(|id| (id, rec.get("downloadClient").and_then(Value::as_str)));
        if let Some(at) = key.and_then(|k| keys.iter().position(|known| *known == Some(k))) {
            let held = &mut out[at];
            for season in one.seasons {
                if !held.seasons.contains(&season) {
                    held.seasons.push(season);
                    held.seasons.sort_unstable();
                }
            }
            // Inconsistent copies do not support a single percentage or estimate.
            if (held.size, held.left) != (one.size, one.left) {
                held.left = None;
            }
            if held.eta_s != one.eta_s {
                held.eta_s = None;
            }
        } else {
            keys.push(key);
            out.push(one);
        }
    }
    out
}

/// What a set of downloads add up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sum {
    pub stage: Stage,
    pub percent: Option<u8>,
    pub eta_s: Option<u32>,
}

fn sum<'a>(downloads: impl IntoIterator<Item = &'a Download>) -> Option<Sum> {
    let mut entries = downloads.into_iter();
    let first = entries.next()?;
    let mut stage = first.stage;
    let mut size = u128::from(first.size);
    let mut left = u128::from(first.left.unwrap_or(first.size));
    let mut sized = first.left.is_some_and(|n| first.size > 0 && n <= first.size);
    let mut identified = first.identified;
    let mut eta_s = first.eta_s;
    for d in entries {
        if d.stage.rank() < stage.rank() {
            stage = d.stage;
        }
        size += u128::from(d.size);
        left += u128::from(d.left.unwrap_or(d.size));
        identified &= d.identified;
        sized &= identified && d.left.is_some_and(|n| d.size > 0 && n <= d.size);
        // Individual estimates do not say how several jobs overlap or wait for one another.
        eta_s = None;
    }
    Some(Sum { stage, percent: sized.then(|| percent(size, left)).flatten(), eta_s })
}

/// One title's record in Radarr or Sonarr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The app's own id, which selects its queue.
    pub id: i64,
    /// Files in the library and what there is to have (a film is 0 or 1 of 1).
    pub have: u32,
    pub total: u32,
    pub seasons: Vec<SeasonCount>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeasonCount {
    pub number: u32,
    pub have: u32,
    pub total: u32,
}

impl Held {
    /// The entry of an app's lookup answer that carries exactly the id asked for.
    fn of_lookup(v: &Value, key: Key, tvdb: Option<u32>) -> Option<Held> {
        let entry = v.as_array()?.iter().find(|e| match key.kind {
            Kind::Movie => number(e, "tmdbId") == Some(key.tmdb),
            Kind::Tv => tvdb.is_some() && number(e, "tvdbId") == tvdb,
        })?;
        let id = entry.get("id")?.as_i64().filter(|id| *id > 0)?;
        let count = |s: &Value| -> (u32, u32) {
            let stats = s.get("statistics");
            (
                stats.and_then(|s| number(s, "episodeFileCount")).unwrap_or(0),
                stats.and_then(|s| number(s, "episodeCount")).unwrap_or(0),
            )
        };
        Some(match key.kind {
            Kind::Movie => Held {
                id,
                have: u32::from(entry.get("hasFile").and_then(Value::as_bool).unwrap_or(false)),
                total: 1,
                seasons: Vec::new(),
            },
            Kind::Tv => {
                let (have, total) = count(entry);
                let seasons = entry
                    .get("seasons")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| {
                        let (have, total) = count(s);
                        Some(SeasonCount { number: number(s, "seasonNumber")?, have, total })
                    })
                    .collect();
                Held { id, have, total, seasons }
            }
        })
    }
}

/// A Seerr request's standing (`MediaRequestStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    Pending,
    Approved,
    Declined,
    Failed,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    pub status: Asked,
    pub seasons: Vec<u32>,
}

/// What Seerr says of the title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeerrView {
    pub media: Have,
    /// The requests that are not 4K, in Seerr's order.
    pub requests: Vec<Req>,
    pub seasons: Vec<(u32, Have)>,
    /// Seerr's copy of the app's queue.
    pub downloads: Vec<Download>,
    /// The show's TheTVDB id, which is Sonarr's exact key.
    pub tvdb: Option<u32>,
}

impl SeerrView {
    /// A title answer of `/api/v1/movie/{id}` or `/api/v1/tv/{id}`.
    pub fn of(v: &Value) -> SeerrView {
        let info = v.get("mediaInfo").filter(|i| i.is_object());
        let requests = info
            .and_then(|i| i.get("requests"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|r| !r.get("is4k").and_then(Value::as_bool).unwrap_or(false))
            .filter_map(|r| {
                let status = match int(r, "status") {
                    1 => Asked::Pending,
                    2 => Asked::Approved,
                    3 => Asked::Declined,
                    4 => Asked::Failed,
                    5 => Asked::Completed,
                    _ => return None,
                };
                let seasons = r.get("seasons").and_then(Value::as_array).into_iter().flatten()
                    .filter_map(|s| number(s, "seasonNumber")).collect();
                Some(Req { status, seasons })
            })
            .collect();
        let seasons = info
            .and_then(|i| i.get("seasons"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| Some((number(s, "seasonNumber")?, Have::of(int(s, "status")))))
            .collect();
        let downloads = info
            .and_then(|i| i.get("downloadStatus"))
            .and_then(Value::as_array)
            .map(|d| group(d.iter()))
            .unwrap_or_default();
        let tvdb = info
            .and_then(|i| number(i, "tvdbId"))
            .filter(|id| *id > 0)
            .or_else(|| v.get("externalIds").and_then(|e| number(e, "tvdbId")))
            .filter(|id| *id > 0);
        SeerrView {
            media: info.map_or(Have::Nothing, |i| Have::of(int(i, "status"))),
            requests,
            seasons,
            downloads,
            tvdb,
        }
    }
}

// ---- asking --------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The title as Seerr holds it.
    Seerr,
    /// The title's record in Radarr/Sonarr: its id and its files.
    Lookup,
    /// The title's queue in Radarr/Sonarr.
    Queue,
}

/// One call the page asks the driver to make, with everything it needs: the driver keeps no state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    /// The outstanding call's identifier; a superseded answer is dropped.
    pub gen: u32,
    pub key: Key,
    pub step: Step,
    /// The app's id for the title, once a lookup found it (the queue step).
    pub arr_id: Option<i64>,
    /// The show's TheTVDB id, once Seerr said it (a show's lookup).
    pub tvdb: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Seerr(SeerrView),
    /// `None` when the app does not hold the title.
    Lookup(Option<Held>),
    Queue(Vec<Download>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    /// No answer, or an error: a later try may work.
    Failed,
    /// The key or sign-in was refused: trying again changes nothing.
    Refused,
    /// The driver already has a call out: ask again shortly.
    Busy,
    /// The app is not linked.
    NotSet,
}

impl From<ArrError> for Fail {
    fn from(e: ArrError) -> Self {
        match e {
            ArrError::Refused => Fail::Refused,
            ArrError::Failed => Fail::Failed,
        }
    }
}

pub type Outcome = Result<Answer, Fail>;

fn records(v: &Value) -> Result<&[Value], Fail> {
    v.as_array().or_else(|| v.get("records")?.as_array()).map(Vec::as_slice).ok_or(Fail::Failed)
}

/// A lookup or queue step against the title's own app (never Seerr).
pub fn run_arr<T: ArrTransport>(link: &ArrLink, http: &T, order: &Order) -> Outcome {
    if !link.is_complete() {
        return Err(Fail::NotSet);
    }
    let arr = Arr::new(link, http);
    let kind = arr_of(order.key.kind);
    match order.step {
        Step::Lookup => {
            let path = match kind {
                ArrKind::Radarr => format!("/api/v3/movie?tmdbId={}", order.key.tmdb),
                ArrKind::Sonarr => format!("/api/v3/series?tvdbId={}", order.tvdb.ok_or(Fail::Failed)?),
            };
            let answer = arr.read(path)?;
            if !answer.is_array() {
                return Err(Fail::Failed);
            }
            Ok(Answer::Lookup(Held::of_lookup(&answer, order.key, order.tvdb)))
        }
        Step::Queue => {
            let id = order.arr_id.ok_or(Fail::Failed)?;
            let (path, owner) = match kind {
                ArrKind::Radarr => (format!("/api/v3/queue/details?movieId={id}"), "movieId"),
                ArrKind::Sonarr => (format!("/api/v3/queue/details?seriesId={id}&includeEpisode=true"), "seriesId"),
            };
            let answer = arr.read(path)?;
            Ok(Answer::Queue(group(records(&answer)?.iter().filter(|rec| {
                rec.get(owner).and_then(Value::as_i64).is_none_or(|owner| owner == id)
            }))))
        }
        Step::Seerr => Err(Fail::Failed),
    }
}

/// What one title's downloads add up to on the Requests list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub tmdb: u32,
    pub sum: Sum,
}

/// The first page of `kind`'s queue: each title in it by its exact TMDB id (`movie.tmdbId` in Radarr,
/// `series.tmdbId` on Sonarr versions that carry it). A title without an ID is never guessed.
pub fn run_list<T: ArrTransport>(link: &ArrLink, http: &T, kind: Kind) -> Result<Vec<Live>, Fail> {
    if !link.is_complete() {
        return Err(Fail::NotSet);
    }
    let (path, owner) = match kind {
        Kind::Movie => ("/api/v3/queue?page=1&pageSize=100&includeMovie=true", "movie"),
        Kind::Tv => ("/api/v3/queue?page=1&pageSize=100&includeSeries=true", "series"),
    };
    let answer = Arr::new(link, http).read(path.into())?;
    let rows = records(&answer)?;
    let clipped = number(&answer, "totalRecords").is_some_and(|total| total as usize > rows.len());
    let mut by_title = std::collections::BTreeMap::<u32, Vec<&Value>>::new();
    for rec in rows {
        let Some(tmdb) = rec.get(owner).and_then(|o| number(o, "tmdbId")).filter(|id| *id > 0) else { continue };
        by_title.entry(tmdb).or_default().push(rec);
    }
    Ok(by_title
        .into_iter()
        .filter_map(|(tmdb, recs)| {
            let mut sum = sum(group(recs).iter())?;
            if clipped {
                sum.percent = None;
                sum.eta_s = None;
            }
            Some(Live { tmdb, sum })
        })
        .collect())
}

// ---- the page's state ----------------------------------------------------------------------------

const BUSY_RETRY_MS: u32 = 500;
/// A call that has not answered this long is given up on: longer than the transport's own deadline.
const LOST_MS: u32 = 40_000;
const RETRY_MS: [u32; 5] = [5_000, 10_000, 20_000, 40_000, 60_000];
/// A queue answer this old is shown with its age and without a time-left.
const QUEUE_STALE_MS: u32 = 20_000;
/// Seerr syncs the apps' queues about once a minute, so its copy is as old as that plus our read.
const SEERR_STALE_MS: u32 = 150_000;

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    /// When the last good answer landed.
    at: Option<u32>,
    /// When to ask next.
    due: u32,
    fails: u32,
    failed: Option<Fail>,
    /// Never to be asked again on this page (the app is not linked, or refused the key).
    off: bool,
}

/// What a page that shows a request's progress holds. The page feeds it the frame clock; it says
/// which call to make, takes each answer, and describes what is known.
#[derive(Debug)]
pub struct Tracker {
    key: Key,
    gen: u32,
    now: u32,
    visible: bool,
    out: Option<(Step, u32, u32)>,
    seerr: Option<SeerrView>,
    held: Option<Option<Held>>,
    queue: Option<Vec<Download>>,
    slots: [Slot; 3],
}

fn at(step: Step) -> usize {
    match step {
        Step::Seerr => 0,
        Step::Lookup => 1,
        Step::Queue => 2,
    }
}

impl Tracker {
    /// A tracker for `key`. `gen` tells this page's answers from an earlier tracker's; it is shown
    /// from the first frame it is ticked.
    pub fn new(key: Key, gen: u32) -> Self {
        Self { key, gen, now: 0, visible: true, out: None, seerr: None, held: None, queue: None, slots: [Slot::default(); 3] }
    }

    pub fn key(&self) -> Key {
        self.key
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// The page is on screen again: what it holds is old, so every step is due.
    pub fn show(&mut self, now: u32) {
        self.now = now;
        if !self.visible {
            self.visible = true;
            for slot in &mut self.slots {
                slot.due = now;
                slot.off = false;
            }
        }
    }

    /// A covered, suspended or leaving page retires its outstanding call and asks nothing.
    pub fn hide(&mut self) {
        self.visible = false;
        self.out = None;
    }

    /// Whether `step` has what it needs to be asked.
    fn ready(&self, step: Step) -> bool {
        let slot = &self.slots[at(step)];
        if slot.off {
            return false;
        }
        match step {
            Step::Seerr => true,
            Step::Lookup => self.key.kind == Kind::Movie || self.seerr.as_ref().is_some_and(|s| s.tvdb.is_some()),
            Step::Queue => matches!(self.held, Some(Some(_))),
        }
    }

    /// The call to make now, if one is due and none is out.
    pub fn advance(&mut self, now: u32) -> Option<Order> {
        self.now = now;
        if !self.visible {
            return None;
        }
        if let Some((_, _, since)) = self.out {
            if now.wrapping_sub(since) < LOST_MS {
                return None;
            }
            self.out = None;
        }
        let step = [Step::Seerr, Step::Lookup, Step::Queue]
            .into_iter()
            .filter(|s| self.ready(*s) && due(self.slots[at(*s)].due, now))
            .min_by_key(|s| std::cmp::Reverse(now.wrapping_sub(self.slots[at(*s)].due)))?;
        let gen = self.gen;
        self.gen = self.gen.wrapping_add(1);
        self.out = Some((step, gen, now));
        Some(Order {
            gen,
            key: self.key,
            step,
            arr_id: self.held.as_ref().and_then(|h| h.as_ref()).map(|h| h.id),
            tvdb: self.seerr.as_ref().and_then(|s| s.tvdb),
        })
    }

    /// Take an answer. `true` when what the page shows may have changed. An answer for another
    /// tracker, or for a call this one gave up on, changes nothing.
    pub fn land(&mut self, gen: u32, step: Step, outcome: Outcome) -> bool {
        if self.out.is_none_or(|(s, call, _)| s != step || call != gen) {
            return false;
        }
        self.out = None;
        let slot = &mut self.slots[at(step)];
        match outcome {
            Err(Fail::Busy) => {
                slot.due = self.now.wrapping_add(BUSY_RETRY_MS);
                false
            }
            Err(Fail::NotSet) => {
                slot.off = true;
                false
            }
            Err(fail) => {
                let changed = slot.failed != Some(fail) || slot.fails == 0;
                slot.failed = Some(fail);
                slot.fails = slot.fails.saturating_add(1);
                if fail == Fail::Refused {
                    slot.off = true;
                } else {
                    slot.due = self.now.wrapping_add(RETRY_MS[(slot.fails as usize - 1).min(RETRY_MS.len() - 1)]);
                }
                changed
            }
            Ok(answer) => {
                slot.at = Some(self.now);
                slot.fails = 0;
                slot.failed = None;
                match answer {
                    Answer::Seerr(view) => {
                        if self.key.kind == Kind::Tv && self.seerr.as_ref().and_then(|s| s.tvdb) != view.tvdb {
                            self.held = None;
                            self.queue = None;
                            self.slots[at(Step::Lookup)] = Slot { due: self.now, ..Slot::default() };
                            self.slots[at(Step::Queue)] = Slot { due: self.now, ..Slot::default() };
                        }
                        self.seerr = Some(view);
                    }
                    Answer::Lookup(held) => {
                        let id = |h: &Option<Held>| h.as_ref().map(|h| h.id);
                        if self.held.as_ref().and_then(id) != id(&held) {
                            self.queue = None;
                            self.slots[at(Step::Queue)] = Slot { due: self.now, ..Slot::default() };
                        }
                        self.held = Some(held);
                    }
                    Answer::Queue(downloads) => self.queue = Some(downloads),
                }
                self.reschedule();
                true
            }
        }
    }

    /// Set each step's next time from what the page now shows.
    fn reschedule(&mut self) {
        let snap = self.snapshot();
        let stage = snap.stage;
        let arr = !self.slots[at(Step::Queue)].off && !self.slots[at(Step::Lookup)].off;
        let (queue, lookup, seerr) = match stage {
            Some(s) if s.moving() => (3_000, if s == Stage::Importing { 4_000 } else { 15_000 }, 30_000),
            Some(s) if !s.settled() => (6_000, 15_000, 30_000),
            _ => (30_000, 60_000, 60_000),
        };
        let seerr = if arr || stage.is_none_or(Stage::settled) { seerr } else { 15_000 };
        for (step, every) in [(Step::Queue, queue), (Step::Lookup, lookup), (Step::Seerr, seerr)] {
            let slot = &mut self.slots[at(step)];
            if slot.fails == 0 && slot.at == Some(self.now) {
                slot.due = self.now.wrapping_add(every);
            }
        }
        // The app has it but Seerr has not caught up: look at Seerr sooner, never faster than every 10 s.
        if stage == Some(Stage::Imported) {
            let seerr = &mut self.slots[at(Step::Seerr)];
            if let Some(last) = seerr.at {
                let soon = last.wrapping_add(10_000);
                if !due(seerr.due, soon) {
                    seerr.due = soon;
                }
            }
        }
    }

    /// What the page can say now.
    pub fn snapshot(&self) -> Snapshot {
        build(self)
    }
}

/// `deadline` has come, in a clock that wraps.
fn due(deadline: u32, now: u32) -> bool {
    now.wrapping_sub(deadline) < u32::MAX / 2
}

fn age_s(then: Option<u32>, now: u32) -> Option<u32> {
    then.map(|t| now.wrapping_sub(t) / 1000)
}

// ---- what the page says --------------------------------------------------------------------------

/// A service that could not be reached or would not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Seerr,
    Arr(ArrKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    Unreachable(Who),
    Refused(Who),
}

/// Where the downloads shown were read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Arr(ArrKind),
    Seerr,
}

/// One season of a show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeasonRow {
    pub number: u32,
    pub stage: Stage,
    /// Only when the season's own download reported it.
    pub percent: Option<u8>,
    pub eta_s: Option<u32>,
    /// Files in the library and what there is to have, from the app.
    pub have: Option<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Nothing has answered yet.
    pub loading: bool,
    /// `None` when there is nothing to report: the title has not been asked for.
    pub stage: Option<Stage>,
    pub percent: Option<u8>,
    pub eta_s: Option<u32>,
    /// The download client as the app names it, while a download runs.
    pub client: Option<String>,
    /// Files in the library of a show, from the app.
    pub library: Option<(u32, u32)>,
    pub seasons: Vec<SeasonRow>,
    pub from: Option<Origin>,
    /// Seconds since the answer the downloads were read from.
    pub age_s: Option<u32>,
    /// That answer is old enough that its numbers may have moved on.
    pub stale: bool,
    /// Approved, an app is linked, and the app does not hold the title yet.
    pub not_sent: bool,
    pub trouble: Option<Trouble>,
}

fn open(req: &Req) -> bool {
    matches!(req.status, Asked::Pending | Asked::Approved)
}

/// The stage a request states; a completed one says nothing, Seerr's media status does.
fn request_stage(r: &Req) -> Option<Stage> {
    match r.status {
        Asked::Pending => Some(Stage::Requested),
        Asked::Approved => Some(Stage::Approved),
        Asked::Declined => Some(Stage::Declined),
        Asked::Failed => Some(Stage::Failed),
        Asked::Completed => None,
    }
}

fn have_stage(have: Have) -> Option<Stage> {
    match have {
        Have::Pending => Some(Stage::Requested),
        Have::Processing => Some(Stage::Approved),
        Have::Partly => Some(Stage::Partial),
        Have::All => Some(Stage::Available),
        Have::Nothing | Have::Blocklisted => None,
    }
}

fn build(t: &Tracker) -> Snapshot {
    let mut snap = Snapshot {
        loading: false,
        stage: None,
        percent: None,
        eta_s: None,
        client: None,
        library: None,
        seasons: Vec::new(),
        from: None,
        age_s: None,
        stale: false,
        not_sent: false,
        trouble: None,
    };
    let kind = arr_of(t.key.kind);
    let [seerr_slot, lookup_slot, queue_slot] = t.slots;
    for (slot, who) in [(queue_slot, Who::Arr(kind)), (lookup_slot, Who::Arr(kind)), (seerr_slot, Who::Seerr)] {
        if snap.trouble.is_none() && slot.fails > 0 {
            snap.trouble = Some(if slot.failed == Some(Fail::Refused) { Trouble::Refused(who) } else { Trouble::Unreachable(who) });
        }
    }
    snap.loading = snap.trouble.is_none() && t.slots.iter().all(|s| s.at.is_none());

    // The downloads come from the app's queue while it answers, else from Seerr's copy of it.
    let arr_age = queue_slot.at.map(|a| t.now.wrapping_sub(a));
    let arr_fresh = arr_age.is_some_and(|a| a <= QUEUE_STALE_MS);
    let seerr_age = seerr_slot.at.map(|a| t.now.wrapping_sub(a));
    let seerr_older = matches!((arr_age, seerr_age), (Some(a), Some(s)) if a > s);
    let (downloads, origin, age, limit): (&[Download], Option<Origin>, Option<u32>, u32) = match (&t.queue, &t.seerr) {
        (Some(q), seerr) if arr_fresh || seerr.is_none() || !seerr_older => (q.as_slice(), Some(Origin::Arr(kind)), arr_age, QUEUE_STALE_MS),
        (_, Some(s)) => (s.downloads.as_slice(), Some(Origin::Seerr), seerr_age, SEERR_STALE_MS),
        (_, None) => (&[], None, None, 0),
    };
    snap.from = origin;
    snap.stale = age.is_some_and(|a| a > limit);
    // The age is only worth saying once the numbers may have moved on.
    snap.age_s = age.filter(|_| snap.stale).map(|a| a / 1000);
    let known_held = t.held.as_ref().and_then(|h| h.as_ref());
    snap.not_sent = t.held == Some(None) && t.seerr.as_ref().is_some_and(|s| {
        s.requests.iter().any(|r| r.status == Asked::Approved) || s.media == Have::Processing
    });

    let total = sum(downloads.iter());
    if let Some(total) = &total {
        snap.percent = total.percent;
        snap.eta_s = total.eta_s.filter(|_| !snap.stale);
        if let Some(first) = downloads.first().map(|d| &d.client).filter(|c| !c.is_empty()) {
            if downloads.iter().all(|d| &d.client == first) {
                snap.client = Some(first.clone());
            }
        }
    }

    let Some(seerr) = &t.seerr else {
        // The app alone: what its queue and files say, nothing about the request.
        snap.stage = total.map(|sum| sum.stage).or_else(|| known_held.filter(|h| h.have > 0 && h.have >= h.total).map(|_| Stage::Imported));
        return snap;
    };

    if t.key.kind == Kind::Movie {
        snap.stage = if seerr.media == Have::All {
            snap.percent = None;
            snap.eta_s = None;
            snap.client = None;
            Some(Stage::Available)
        } else if let Some(total) = total.filter(|_| !downloads.is_empty()) {
            Some(total.stage)
        } else if known_held.is_some_and(|h| h.have > 0) {
            Some(Stage::Imported)
        } else {
            seerr.requests.iter().find(|r| open(r)).or(seerr.requests.last()).and_then(request_stage).or_else(|| have_stage(seerr.media))
        };
        return snap;
    }

    // A show: one row per season that has something to say.
    snap.library = known_held.filter(|h| h.total > 0).map(|h| (h.have, h.total));
    let mut numbers: Vec<u32> = Vec::new();
    let mut note = |n: u32| {
        if !numbers.contains(&n) {
            numbers.push(n);
        }
    };
    for r in &seerr.requests {
        r.seasons.iter().for_each(|n| note(*n));
    }
    for (n, have) in &seerr.seasons {
        if *have != Have::Nothing {
            note(*n);
        }
    }
    for d in downloads {
        d.seasons.iter().for_each(|n| note(*n));
    }
    if let Some(h) = known_held {
        for s in h.seasons.iter().filter(|s| s.have > 0) {
            note(s.number);
        }
    }
    numbers.sort_unstable();
    let unidentified = downloads.len() > 1 && downloads.iter().any(|d| !d.identified);
    for number in numbers {
        let has_pack = downloads.iter().any(|d| d.seasons.len() > 1 && d.seasons.contains(&number));
        let counts = known_held.and_then(|h| h.seasons.iter().find(|s| s.number == number)).map(|s| (s.have, s.total));
        let row_sum = sum(downloads.iter().filter(|d| d.seasons.contains(&number)));
        let asked = seerr.requests.iter().filter(|r| r.seasons.contains(&number));
        let seerr_have = seerr.seasons.iter().find(|(n, _)| *n == number).map_or(Have::Nothing, |(_, h)| *h);
        let stage = row_sum
            .as_ref()
            .map(|s| s.stage)
            .or_else(|| (seerr_have == Have::All).then_some(Stage::Available))
            .or_else(|| match counts {
                Some((have, total)) if total > 0 && have >= total => Some(Stage::Imported),
                _ => None,
            })
            .or_else(|| counts.filter(|(have, _)| *have > 0).map(|_| Stage::Partial))
            .or_else(|| have_stage(seerr_have).filter(|s| *s != Stage::Available))
            .or_else(|| asked.clone().find(|r| open(r)).or(asked.last()).and_then(request_stage));
        let Some(stage) = stage else { continue };
        // A pack shared with other seasons has one progress for all of them: no number per season.
        let alone = !has_pack && !unidentified;
        snap.seasons.push(SeasonRow {
            number,
            stage,
            percent: row_sum.as_ref().filter(|_| alone).and_then(|s| s.percent),
            eta_s: row_sum.as_ref().filter(|_| alone && !snap.stale).and_then(|s| s.eta_s),
            have: counts,
        });
    }
    snap.stage = total
        .map(|sum| sum.stage)
        .into_iter()
        .chain(snap.seasons.iter().map(|s| s.stage))
        .min_by_key(|s| s.rank())
        .or_else(|| seerr.requests.iter().find(|r| open(r)).or(seerr.requests.last()).and_then(request_stage))
        .or_else(|| have_stage(seerr.media));
    if snap.stage == Some(Stage::Available) && seerr.media == Have::Partly {
        snap.stage = Some(Stage::Partial);
    }
    snap
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
