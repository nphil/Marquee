//! What follows a Plex deletion so the title stays gone: the Watchlist, Radarr or Sonarr, and Seerr.
//!
//! Each step reports alone ([`Cleanup`]), a failed step never stops the next, and only a failed step
//! is ever tried again ([`retry_pending`]): the Plex delete is never repeated. The identity the
//! steps need (TMDB and TVDB ids, title, year) is read from Plex BEFORE the delete, because the
//! server forgets the title afterwards ([`Captured`]). All calls block; use a worker.

use super::{MediaKind, WatchlistOutcome};
use crate::plex::account::AccountClient;
use crate::plex::arr::{Arr, ArrError, CurlTransport as ArrCurl};
use crate::plex::seerr::{CurlTransport as SeerrCurl, Kind as SeerrKind, Seerr, SeerrError};
use crate::plex::session::{ArrKind, ArrLink};
use serde_json::Value;
use std::sync::Mutex;

/// What one cleanup step came to. Only [`Step::Failed`] is worth trying again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    /// Not set up in Settings, so nothing was asked.
    #[default]
    NotConnected,
    /// Nothing to do: the deletion left episodes behind, or the app held nothing of the title.
    NotApplicable,
    /// Plex could not say which title this was (no TMDB or TVDB id), so nothing was asked.
    Unmatched,
    Removed,
    /// The app refused the key or the account's permission; asking again changes nothing.
    Refused,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleanup {
    /// Radarr for a movie, Sonarr for a show.
    pub arr: Step,
    pub seerr: Step,
}

impl Cleanup {
    /// An episode or season deleted while others remain: removing the series would take their files.
    pub const PARTIAL: Self = Self { arr: Step::NotApplicable, seerr: Step::NotApplicable };

    pub fn failed(self) -> bool {
        self.arr == Step::Failed || self.seerr == Step::Failed
    }
}

/// The title's identity from Plex's own record of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub title: String,
    pub year: i64,
}

impl Ids {
    /// `item` is a metadata record read with `includeGuids=1`: its `Guid` array holds `tmdb://603`.
    pub(super) fn of(item: &Value) -> Self {
        let guid = |scheme: &str| {
            item.get("Guid")?.as_array()?.iter().find_map(|g| g.get("id")?.as_str()?.strip_prefix(scheme)?.parse().ok())
        };
        Self {
            tmdb: guid("tmdb://"),
            tvdb: guid("tvdb://"),
            title: item.get("title").and_then(Value::as_str).unwrap_or_default().to_owned(),
            year: super::number(item, "year").unwrap_or(0),
        }
    }
}

/// What the deletion read before the delete and the cleanup needs after it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captured {
    pub ids: Ids,
    /// The Watchlist's catalog id.
    pub watch_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Connected {
    pub radarr: bool,
    pub sonarr: bool,
    pub seerr: bool,
}

impl Connected {
    pub(super) fn any(self) -> bool {
        self.radarr || self.sonarr || self.seerr
    }

    fn arr(self, kind: MediaKind) -> bool {
        if kind == MediaKind::Movie { self.radarr } else { self.sonarr }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fail {
    Refused,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cleared {
    Removed,
    /// Seerr holds no media record of the title.
    Nothing,
}

/// The calls a cleanup makes beside Plex's own.
pub(super) trait Steps {
    fn pause(&mut self, ms: u64);
    fn remove_watchlist(&mut self, hex: &str) -> Option<bool>;
    fn connected(&self) -> Connected {
        Connected::default()
    }
    /// One attempt at taking the title out of Radarr (movie) or Sonarr (show or episode).
    fn remove_from_arr(&mut self, _kind: MediaKind, _ids: &Ids) -> Result<(), Fail> {
        Err(Fail::Failed)
    }
    /// One attempt at clearing the title's Seerr record.
    fn clear_seerr(&mut self, _kind: MediaKind, _tmdb: i64) -> Result<Cleared, Fail> {
        Err(Fail::Failed)
    }
}

pub(super) const WATCHLIST_TRIES: u32 = 3;
pub(super) const WATCHLIST_PAUSE_MS: u64 = 1_000;
const STEP_TRIES: u32 = 2;
const STEP_PAUSE_MS: u64 = 1_000;

/// Take the item off the account Watchlist. The account service can fail a call for a moment, so an
/// unanswered try is repeated before the cleanup is reported as failed.
pub(super) fn unlist<S: Steps>(steps: &mut S, id: Option<&str>) -> WatchlistOutcome {
    let Some(id) = id else { return WatchlistOutcome::Failed; };
    for attempt in 0..WATCHLIST_TRIES {
        if attempt > 0 { steps.pause(WATCHLIST_PAUSE_MS); }
        match steps.remove_watchlist(id) {
            Some(true) => return WatchlistOutcome::Removed,
            Some(false) => return WatchlistOutcome::NotListed,
            None => {}
        }
    }
    WatchlistOutcome::Failed
}

/// Two tries a second apart; a refusal is final.
fn twice<S: Steps, T>(steps: &mut S, mut call: impl FnMut(&mut S) -> Result<T, Fail>) -> Result<T, Fail> {
    let mut last = Fail::Failed;
    for attempt in 0..STEP_TRIES {
        if attempt > 0 { steps.pause(STEP_PAUSE_MS); }
        match call(steps) {
            Ok(done) => return Ok(done),
            Err(Fail::Refused) => return Err(Fail::Refused),
            Err(failed) => last = failed,
        }
    }
    Err(last)
}

fn word(failure: Fail) -> Step {
    match failure { Fail::Refused => Step::Refused, Fail::Failed => Step::Failed }
}

fn arr_step<S: Steps>(steps: &mut S, kind: MediaKind, ids: &Ids) -> Step {
    if !steps.connected().arr(kind) { return Step::NotConnected; }
    if (if kind == MediaKind::Movie { ids.tmdb } else { ids.tvdb }).is_none() { return Step::Unmatched; }
    match twice(steps, |s| s.remove_from_arr(kind, ids)) {
        Ok(()) => Step::Removed,
        Err(failure) => word(failure),
    }
}

fn seerr_step<S: Steps>(steps: &mut S, kind: MediaKind, ids: &Ids) -> Step {
    if !steps.connected().seerr { return Step::NotConnected; }
    let Some(tmdb) = ids.tmdb else { return Step::Unmatched; };
    match twice(steps, |s| s.clear_seerr(kind, tmdb)) {
        Ok(Cleared::Removed) => Step::Removed,
        Ok(Cleared::Nothing) => Step::NotApplicable,
        Err(failure) => word(failure),
    }
}

/// Run both app steps for a title that is wholly gone.
pub(super) fn run<S: Steps>(steps: &mut S, kind: MediaKind, ids: &Ids) -> Cleanup {
    Cleanup { arr: arr_step(steps, kind, ids), seerr: seerr_step(steps, kind, ids) }
}

/// A whole title whose identity could not be read: what is set up is reported as unmatched, not asked.
pub(super) fn unmatched<S: Steps>(steps: &S, kind: MediaKind) -> Cleanup {
    let on = steps.connected();
    Cleanup {
        arr: if on.arr(kind) { Step::Unmatched } else { Step::NotConnected },
        seerr: if on.seerr { Step::Unmatched } else { Step::NotConnected },
    }
}

/// What a person can still ask for after a deletion: the steps that failed, with what they need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    kind: MediaKind,
    captured: Captured,
    watchlist: WatchlistOutcome,
    cleanup: Cleanup,
}

impl Ticket {
    /// `None` when no step failed: nothing waits for a retry.
    pub(super) fn after(kind: MediaKind, captured: &Captured, watchlist: WatchlistOutcome, cleanup: Cleanup) -> Option<Self> {
        (watchlist == WatchlistOutcome::Failed || cleanup.failed())
            .then(|| Self { kind, captured: captured.clone(), watchlist, cleanup })
    }
}

/// What one retry came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retried {
    pub kind: MediaKind,
    pub watchlist: WatchlistOutcome,
    pub cleanup: Cleanup,
}

/// Rerun only what failed; every other step keeps its answer. The Plex delete is not here.
pub(super) fn retry<S: Steps>(steps: &mut S, ticket: &Ticket) -> (Retried, Option<Ticket>) {
    let Ticket { kind, captured, watchlist, cleanup } = ticket;
    let watchlist = if *watchlist == WatchlistOutcome::Failed { unlist(steps, captured.watch_id.as_deref()) } else { *watchlist };
    let cleanup = Cleanup {
        arr: if cleanup.arr == Step::Failed { arr_step(steps, *kind, &captured.ids) } else { cleanup.arr },
        seerr: if cleanup.seerr == Step::Failed { seerr_step(steps, *kind, &captured.ids) } else { cleanup.seerr },
    };
    let next = Ticket::after(*kind, captured, watchlist, cleanup);
    (Retried { kind: *kind, watchlist, cleanup }, next)
}

// ---- what waits, in memory ---------------------------------------------------------------------

/// The deletion whose steps wait for a retry: the newest one. Settings offers it.
static PENDING: Mutex<Option<Ticket>> = Mutex::new(None);

pub(super) fn set_pending(ticket: Option<Ticket>) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = ticket;
}

/// The title a retry would finish, if one waits.
pub fn pending_title() -> Option<String> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|t| t.captured.ids.title.clone())
}

/// What a deletion read before Plex's delete that did not settle: if it did take effect after all,
/// the next attempt finds the item gone and finishes with this.
static STASH: Mutex<Option<(String, Captured)>> = Mutex::new(None);

pub(super) fn stash(rating_key: &str, captured: Captured) {
    *STASH.lock().unwrap_or_else(|e| e.into_inner()) = Some((rating_key.to_owned(), captured));
}

pub(super) fn unstash(rating_key: &str) -> Option<Captured> {
    let mut held = STASH.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_some_and(|(key, _)| key == rating_key) { held.take().map(|(_, c)| c) } else { None }
}

/// Retry the waiting deletion's failed steps. `None` when nothing waits.
pub fn retry_pending(account: Option<&AccountClient>) -> Option<Retried> {
    let ticket = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    let (retried, next) = retry(&mut Reach::read(account), &ticket);
    set_pending(next);
    Some(retried)
}

// ---- the real calls ----------------------------------------------------------------------------

struct SeerrAccess {
    base: String,
    cookie: String,
    plex_token: String,
}

/// The links as they are now, read once when a run starts.
pub(super) struct Reach<'a> {
    account: Option<&'a AccountClient>,
    radarr: ArrLink,
    sonarr: ArrLink,
    seerr: Option<SeerrAccess>,
}

impl<'a> Reach<'a> {
    pub(super) fn read(account: Option<&'a AccountClient>) -> Self {
        let session = crate::plex::session::peek();
        let seerr = session.seerr();
        let seerr = seerr.is_ready().then(|| {
            let user = crate::plex::session::current();
            SeerrAccess {
                plex_token: user.as_ref().and_then(crate::plex::session::plex_tv_credential).unwrap_or_default(),
                cookie: user.as_ref().map(|u| seerr.session_for(&u.uuid).to_owned()).unwrap_or_default(),
                base: seerr.url.clone(),
            }
        });
        Self { account, radarr: session.arr(ArrKind::Radarr), sonarr: session.arr(ArrKind::Sonarr), seerr }
    }
}

impl Steps for Reach<'_> {
    fn pause(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn remove_watchlist(&mut self, hex: &str) -> Option<bool> {
        let account = self.account?;
        let state = account.user_state(hex, &mut account.watchlist_transport()).ok()?;
        if state.watchlisted_at.is_none() { return Some(false); }
        account.watchlist_remove(hex, &mut account.watchlist_transport()).ok()?;
        Some(true)
    }

    fn connected(&self) -> Connected {
        Connected { radarr: self.radarr.is_complete(), sonarr: self.sonarr.is_complete(), seerr: self.seerr.is_some() }
    }

    fn remove_from_arr(&mut self, kind: MediaKind, ids: &Ids) -> Result<(), Fail> {
        let (arr, link, id) = match kind {
            MediaKind::Movie => (ArrKind::Radarr, &self.radarr, ids.tmdb),
            _ => (ArrKind::Sonarr, &self.sonarr, ids.tvdb),
        };
        let id = id.ok_or(Fail::Failed)?;
        Arr::new(link, &ArrCurl).remove(arr, id, &ids.title, ids.year).map_err(|e| match e {
            ArrError::Refused => Fail::Refused,
            ArrError::Failed => Fail::Failed,
        })
    }

    fn clear_seerr(&mut self, kind: MediaKind, tmdb: i64) -> Result<Cleared, Fail> {
        let access = self.seerr.as_ref().ok_or(Fail::Failed)?;
        let tmdb = u32::try_from(tmdb).map_err(|_| Fail::Failed)?;
        let seerr = Seerr::new(&access.base, &access.cookie, &access.plex_token, &SeerrCurl);
        let kind = if kind == MediaKind::Movie { SeerrKind::Movie } else { SeerrKind::Tv };
        let failed = |e: SeerrError| match e {
            SeerrError::Forbidden | SeerrError::SignInRefused => Fail::Refused,
            _ => Fail::Failed,
        };
        match seerr.media_id(kind, tmdb).map_err(failed)? {
            None => Ok(Cleared::Nothing),
            Some(id) => seerr.delete_media(id).map(|()| Cleared::Removed).map_err(failed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        connected: Connected,
        arr: VecDeque<Result<(), Fail>>,
        seerr: VecDeque<Result<Cleared, Fail>>,
        watch: VecDeque<Option<bool>>,
        calls: Vec<&'static str>,
        paused: Vec<u64>,
    }

    impl Steps for Fake {
        fn pause(&mut self, ms: u64) { self.paused.push(ms); }
        fn remove_watchlist(&mut self, _: &str) -> Option<bool> {
            self.calls.push("watchlist");
            self.watch.pop_front().unwrap_or(Some(true))
        }
        fn connected(&self) -> Connected { self.connected }
        fn remove_from_arr(&mut self, _: MediaKind, _: &Ids) -> Result<(), Fail> {
            self.calls.push("arr");
            self.arr.pop_front().unwrap_or(Ok(()))
        }
        fn clear_seerr(&mut self, _: MediaKind, _: i64) -> Result<Cleared, Fail> {
            self.calls.push("seerr");
            self.seerr.pop_front().unwrap_or(Ok(Cleared::Removed))
        }
    }

    const ALL: Connected = Connected { radarr: true, sonarr: true, seerr: true };

    fn ids() -> Ids {
        Ids { tmdb: Some(603), tvdb: Some(81189), title: "The Matrix".into(), year: 1999 }
    }

    #[test]
    fn identity_comes_from_the_guid_list_and_a_missing_one_is_unmatched_not_guessed() {
        let item = serde_json::json!({"title":"The Matrix","year":1999,"Guid":[{"id":"imdb://tt0133093"},{"id":"tmdb://603"},{"id":"tvdb://12"}]});
        assert_eq!(Ids::of(&item), Ids { tmdb: Some(603), tvdb: Some(12), title: "The Matrix".into(), year: 1999 });
        let mut fake = Fake { connected: ALL, ..Default::default() };
        let none = Ids { title: "x".into(), ..Default::default() };
        assert_eq!(run(&mut fake, MediaKind::Movie, &none), Cleanup { arr: Step::Unmatched, seerr: Step::Unmatched });
        assert!(fake.calls.is_empty(), "no id, no request");
    }

    #[test]
    fn a_movie_asks_radarr_and_a_show_asks_sonarr_and_what_is_not_set_up_is_not_asked() {
        for (kind, connected) in [(MediaKind::Movie, Connected { radarr: true, ..Connected::default() }), (MediaKind::Show, Connected { sonarr: true, ..Connected::default() })] {
            let mut fake = Fake { connected, ..Default::default() };
            assert_eq!(run(&mut fake, kind, &ids()), Cleanup { arr: Step::Removed, seerr: Step::NotConnected });
            assert_eq!(fake.calls, ["arr"]);
        }
        let mut other = Fake { connected: Connected { sonarr: true, ..Connected::default() }, ..Default::default() };
        assert_eq!(run(&mut other, MediaKind::Movie, &ids()).arr, Step::NotConnected, "a movie never asks Sonarr");
    }

    #[test]
    fn each_step_reports_alone_with_two_tries_and_a_refusal_is_never_repeated() {
        let mut fake = Fake { connected: ALL, ..Default::default() };
        fake.arr = [Err(Fail::Failed), Err(Fail::Failed)].into();
        fake.seerr = [Err(Fail::Refused)].into();
        assert_eq!(run(&mut fake, MediaKind::Show, &ids()), Cleanup { arr: Step::Failed, seerr: Step::Refused });
        assert_eq!(fake.calls, ["arr", "arr", "seerr"]);
        assert_eq!(fake.paused, [1_000]);
        let mut late = Fake { connected: ALL, arr: [Err(Fail::Failed)].into(), seerr: [Ok(Cleared::Nothing)].into(), ..Default::default() };
        assert_eq!(run(&mut late, MediaKind::Movie, &ids()), Cleanup { arr: Step::Removed, seerr: Step::NotApplicable });
    }

    #[test]
    fn a_retry_runs_only_the_failed_steps_and_keeps_the_rest() {
        let captured = Captured { ids: ids(), watch_id: Some("abc".into()) };
        let cleanup = Cleanup { arr: Step::Failed, seerr: Step::Removed };
        let ticket = Ticket::after(MediaKind::Movie, &captured, WatchlistOutcome::Removed, cleanup).expect("a step failed");
        let mut fake = Fake { connected: ALL, ..Default::default() };
        let (done, next) = retry(&mut fake, &ticket);
        assert_eq!(fake.calls, ["arr"], "not the Watchlist, not Seerr, never the Plex delete");
        assert_eq!((done.watchlist, done.cleanup), (WatchlistOutcome::Removed, Cleanup { arr: Step::Removed, seerr: Step::Removed }));
        assert_eq!(next, None, "nothing waits any more");
        let mut again = Fake { connected: ALL, arr: [Err(Fail::Failed), Err(Fail::Failed)].into(), ..Default::default() };
        let (_, next) = retry(&mut again, &ticket);
        assert!(next.is_some(), "still failing: it keeps waiting");
    }

    #[test]
    fn a_failed_watchlist_removal_is_retried_with_the_id_read_before_the_delete() {
        let captured = Captured { ids: Ids::default(), watch_id: Some("abc".into()) };
        let ticket = Ticket::after(MediaKind::Show, &captured, WatchlistOutcome::Failed, Cleanup::default()).unwrap();
        let mut fake = Fake::default();
        let (done, next) = retry(&mut fake, &ticket);
        assert_eq!((done.watchlist, next), (WatchlistOutcome::Removed, None));
        assert!(Ticket::after(MediaKind::Show, &captured, WatchlistOutcome::Removed, Cleanup { arr: Step::Refused, seerr: Step::Unmatched }).is_none(),
            "a refusal is not retried");
    }

    #[test]
    fn what_a_deletion_read_is_handed_back_once_and_only_for_its_own_item() {
        stash("42", Captured { ids: ids(), watch_id: None });
        assert_eq!(unstash("43"), None);
        assert_eq!(unstash("42").map(|c| c.ids.tmdb), Some(Some(603)));
        assert_eq!(unstash("42"), None);
    }
}
