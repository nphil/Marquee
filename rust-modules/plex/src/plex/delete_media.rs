//! Destructive PMS deletion and what follows it: the account Watchlist, Radarr or Sonarr, and Seerr
//! ([`cleanup`]). All calls block; use a worker.
use super::{account::AccountClient, catalog_hex_of_guid, Client, ClientKey};
use super::paging::{paged_path, PageReq};
use serde_json::Value;

mod cleanup;
use cleanup::{unlist, Captured, Cleared, Connected, Fail, Ids, Steps};
pub use cleanup::{pending_title, retry_pending, Cleanup, Retried, Step};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind { Movie, Show, Episode }

/// Why nothing was deleted, reduced to what a person can act on. Trying again is always safe: a
/// deletion that did take effect reads on the next try as "already deleted", which is a success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteError {
    /// The server refused this account (401/403) or does not say that it allows deletion: the
    /// server's owner has to turn "Allow media deletion" on, and only the owner's account may delete.
    NotAllowed,
    /// No answer, a server error, or a delete that could not be confirmed either way.
    ServerProblem,
    /// The request does not name a movie, show or episode on the server.
    InvalidItem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchlistOutcome {
    Removed,
    NotListed,
    Preserved,
    Failed,
    /// An episode was deleted but whether anything of its show is left could not be read, so the show
    /// was neither kept nor taken off the Watchlist.
    Unknown,
    /// The item was already gone and the screen could not say which Watchlist entry it had.
    Skipped,
}

/// Where a deleted episode sat, from the server's own record of it just before the delete: what the
/// season and show counts on screen need in order to follow the deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpisodeParent { pub show_rk: String, pub season_index: i64 }

/// The Watchlist identity the screen already holds for the item being deleted, for when the server
/// no longer has a record to read it from: the catalog guid of the movie or show (an episode names
/// its show's) and, for an episode, the show's rating key, to tell whether anything of the show is left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed { pub guid: String, pub show_rk: Option<String> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub watchlist: WatchlistOutcome,
    /// Set for an episode whose record named its show and season.
    pub parent: Option<EpisodeParent>,
    /// The item had been played, which decides whether the watched counts above it move too.
    pub watched: bool,
    /// The server no longer had the item when the delete started, so this try deleted nothing.
    pub already_gone: bool,
    /// What Radarr or Sonarr and Seerr came to.
    pub cleanup: Cleanup,
}

trait Backend: Steps {
    fn still_current(&self) -> bool;
    fn get(&mut self, path: &str) -> Option<(i32, Option<Value>)>;
    fn delete(&mut self, path: &str) -> Option<i32>;
    /// What this attempt learned first-hand about the server's "Allow media deletion" setting.
    fn note_deletion(&mut self, on: bool);
}

struct Live<'a> { server: Client, original: &'a Client, key: ClientKey, reach: cleanup::Reach<'a> }
impl Backend for Live<'_> {
    fn still_current(&self) -> bool {
        !self.original.data_io_disabled() && ClientKey::of(self.original) == self.key
            && super::client_for(self.original.id()).is_some_and(|current| std::ptr::eq(current, self.original))
            && super::server_facts(self.original.id()).is_some_and(|facts| facts.owned)
    }
    fn get(&mut self, path: &str) -> Option<(i32, Option<Value>)> {
        self.server.get_value_status(path)
    }
    fn delete(&mut self, path: &str) -> Option<i32> {
        let status = self.server.delete_slow(path);
        (status >= 0).then_some(status)
    }
    fn note_deletion(&mut self, on: bool) {
        super::serverinfo::note_deletion(self.original.id(), on);
    }
}

impl Steps for Live<'_> {
    fn pause(&mut self, ms: u64) { self.reach.pause(ms) }
    fn remove_watchlist(&mut self, hex: &str) -> Option<bool> { self.reach.remove_watchlist(hex) }
    fn connected(&self) -> Connected { self.reach.connected() }
    fn remove_from_arr(&mut self, kind: MediaKind, ids: &Ids) -> Result<(), Fail> { self.reach.remove_from_arr(kind, ids) }
    fn clear_seerr(&mut self, kind: MediaKind, tmdb: i64) -> Result<Cleared, Fail> { self.reach.clear_seerr(kind, tmdb) }
}

/// `allowMediaDeletion` as PMS spells it (a bool on 1.43; the lenient forms elsewhere). Anything
/// else is not an answer.
fn flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(v) => Some(*v),
        Value::Number(n) if n.as_i64() == Some(1) => Some(true),
        Value::Number(n) if n.as_i64() == Some(0) => Some(false),
        Value::String(s) if s == "1" || s.eq_ignore_ascii_case("true") => Some(true),
        Value::String(s) if s == "0" || s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// Ask the server, just before deleting, whether it allows deletion. The answer is kept for the
/// item menu too. An absent or unreadable answer is not permission: nothing is deleted on a guess.
fn permitted<B: Backend>(backend: &mut B) -> Result<(), DeleteError> {
    let (status, body) = backend.get("/").ok_or(DeleteError::ServerProblem)?;
    if matches!(status, 401 | 403) {
        backend.note_deletion(false);
        return Err(DeleteError::NotAllowed);
    }
    if !(200..300).contains(&status) { return Err(DeleteError::ServerProblem); }
    let body = body.ok_or(DeleteError::ServerProblem)?;
    let on = body.get("MediaContainer").and_then(|c| c.get("allowMediaDeletion")).and_then(flag) == Some(true);
    backend.note_deletion(on);
    if on { Ok(()) } else { Err(DeleteError::NotAllowed) }
}

fn key_ok(key: &str) -> bool { !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) }
fn item_path(key: &str) -> String { format!("/library/metadata/{key}") }
fn first_metadata(body: &Value) -> Option<Value> {
    body.get("MediaContainer")?.get("Metadata")?.as_array()?.first().cloned()
}
fn metadata<B: Backend>(backend: &mut B, key: &str) -> Option<Value> {
    let (status, body) = backend.get(&item_path(key))?;
    if !(200..300).contains(&status) { return None; }
    first_metadata(&body?)
}

/// The server's record of the item to delete, or `None` when the server says it no longer exists.
fn read_item<B: Backend>(backend: &mut B, key: &str) -> Result<Option<Value>, DeleteError> {
    let (status, body) = backend.get(&item_path(key)).ok_or(DeleteError::ServerProblem)?;
    match status {
        404 => Ok(None),
        401 | 403 => Err(DeleteError::NotAllowed),
        200..=299 => body.as_ref().and_then(first_metadata).map(Some).ok_or(DeleteError::ServerProblem),
        _ => Err(DeleteError::ServerProblem),
    }
}

fn catalog_id(guid: &str, kind: MediaKind) -> Option<String> {
    let prefix = match kind { MediaKind::Movie => "plex://movie/", _ => "plex://show/" };
    let hex = guid.strip_prefix(prefix)?;
    catalog_hex_of_guid(guid)?;
    Some(hex.to_owned())
}

fn number(item: &Value, field: &str) -> Option<i64> {
    let n = item.get(field)?;
    n.as_i64().or_else(|| n.as_str()?.parse().ok())
}

fn episode_parent(item: &Value) -> Option<EpisodeParent> {
    let show_rk = item.get("grandparentRatingKey").and_then(Value::as_str).filter(|s| key_ok(s))?;
    Some(EpisodeParent { show_rk: show_rk.to_owned(), season_index: number(item, "parentIndex")? })
}

// Returns Some(true) only with positive evidence that episodes remain. A missing count is unknown.
fn episodes_remain<B: Backend>(backend: &mut B, show: &str, missing_is_empty: bool) -> Option<bool> {
    let path = paged_path(&format!("/library/metadata/{show}/allLeaves"), PageReq { start: 0, size: 1 });
    let (status, body) = backend.get(&path)?;
    if status == 404 && missing_is_empty { return Some(false); }
    if !(200..300).contains(&status) { return None; }
    let body = body?;
    let c = body.get("MediaContainer")?;
    if c.get("Metadata").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) { return Some(true); }
    let n = c.get("totalSize").or_else(|| c.get("size"))?;
    let n = n.as_u64().or_else(|| n.as_str()?.parse().ok())?;
    Some(n != 0)
}

const SETTLE_POLLS: u32 = 3;
const SETTLE_PAUSE_MS: u64 = 2_000;

/// A DELETE that got no clear answer (a server error, or a connection that dropped) may still have
/// taken effect, because PMS keeps working after a client gives up. Look at the item itself for a
/// few seconds: gone means deleted.
fn settled<B: Backend>(backend: &mut B, path: &str) -> Result<(), DeleteError> {
    for _ in 0..SETTLE_POLLS {
        backend.pause(SETTLE_PAUSE_MS);
        match backend.get(path) {
            Some((404, _)) => return Ok(()),
            Some((401 | 403, _)) => return Err(DeleteError::NotAllowed),
            _ => {}
        }
    }
    Err(DeleteError::ServerProblem)
}

/// The DELETE itself. A 404 is "already gone" (an earlier attempt that timed out, another client),
/// which is what the person asked for.
fn remove<B: Backend>(backend: &mut B, key: &str) -> Result<(), DeleteError> {
    let path = item_path(key);
    match backend.delete(&path) {
        Some(status) if (200..300).contains(&status) || status == 404 => Ok(()),
        Some(401 | 403) => {
            backend.note_deletion(false);
            Err(DeleteError::NotAllowed)
        }
        _ => settled(backend, &path),
    }
}

/// The item was already gone when the delete started: an earlier try that reported a server problem
/// but finished later, or another client. Its Watchlist entry can still be there, so it is cleaned
/// from the identity the screen holds, and Radarr, Sonarr and Seerr from what the earlier try read
/// (`read`), if there was one. An episode only counts when its show is gone or empty too.
fn gone<B: Backend>(backend: &mut B, kind: MediaKind, listed: Option<&Listed>, read: &mut Captured) -> DeleteOutcome {
    let Some(id) = listed.and_then(|l| catalog_id(&l.guid, kind)) else {
        return finish_gone(backend, kind, read, WatchlistOutcome::Skipped);
    };
    read.watch_id = Some(id.clone());
    if kind == MediaKind::Episode {
        let Some(show) = listed.and_then(|l| l.show_rk.as_deref()).filter(|s| key_ok(s)) else {
            return finish_gone(backend, kind, read, WatchlistOutcome::Skipped);
        };
        let remaining = match read_item(backend, show) {
            Ok(None) => Some(false),
            Ok(Some(_)) => episodes_remain(backend, show, false),
            Err(_) => None,
        };
        match remaining {
            Some(true) => return finish_gone(backend, kind, read, WatchlistOutcome::Preserved),
            None => return finish_gone(backend, kind, read, WatchlistOutcome::Unknown),
            Some(false) => {}
        }
    }
    let watchlist = unlist(backend, Some(id.as_str()));
    finish_gone(backend, kind, read, watchlist)
}

fn finish_gone<B: Backend>(backend: &mut B, kind: MediaKind, read: &Captured, watchlist: WatchlistOutcome) -> DeleteOutcome {
    // An episode is the last of its show only when the show was read as gone or empty.
    let whole = kind != MediaKind::Episode
        || !matches!(watchlist, WatchlistOutcome::Preserved | WatchlistOutcome::Unknown | WatchlistOutcome::Skipped);
    let cleanup = if !whole {
        Cleanup::PARTIAL
    } else if read.ids.tmdb.is_some() || read.ids.tvdb.is_some() {
        cleanup::run(backend, kind, &read.ids)
    } else {
        cleanup::unmatched(backend, kind)
    };
    DeleteOutcome { watchlist, parent: None, watched: false, already_gone: true, cleanup }
}

/// Delete one item, then clean up after it: the Watchlist, Radarr or Sonarr and Seerr, each
/// reporting alone. Call only after explicit user confirmation. An `Ok` confirms the item is gone,
/// deleted now or already; a step that failed is in the outcome and waits for [`retry_pending`]. An
/// `Err` is safe to retry. `listed` is the Watchlist identity the screen holds, used when the server
/// no longer has the item. `deleted` is called the moment the item is gone, before any cleanup, with
/// what the lists need to follow it.
pub fn delete(server: &Client, key: ClientKey, account: Option<&AccountClient>, rating_key: &str, kind: MediaKind,
              listed: Option<&Listed>, deleted: &mut dyn FnMut(Option<&EpisodeParent>, bool)) -> Result<DeleteOutcome, DeleteError>
{
    if !super::server_facts(server.id()).is_some_and(|facts| facts.owned) { return Err(DeleteError::NotAllowed); }
    let captured = server.captured_for_key(key).ok_or(DeleteError::ServerProblem)?;
    let mut read = cleanup::unstash(rating_key).unwrap_or_default();
    let result = execute(&mut Live { server: captured, original: server, key, reach: cleanup::Reach::read(account) },
        rating_key, kind, listed, &mut read, deleted);
    match &result {
        Ok(done) => {
            if let Some(ticket) = cleanup::Ticket::after(kind, &read, done.watchlist, done.cleanup) {
                cleanup::set_pending(Some(ticket));
            }
        }
        // The delete may still finish on the server: a later try finds the item gone and needs this.
        Err(DeleteError::ServerProblem) if read != Captured::default() => cleanup::stash(rating_key, read),
        Err(_) => {}
    }
    result
}

/// The title's record with its external ids (`includeGuids`). Two tries a second apart; a title that
/// cannot be read is cleaned up as unmatched and never blocks the delete.
fn read_guids<B: Backend>(backend: &mut B, key: &str) -> Option<Value> {
    for attempt in 0..2 {
        if attempt > 0 { backend.pause(1_000); }
        match backend.get(&format!("{}?includeGuids=1", item_path(key))) {
            Some((404, _)) => return None,
            Some((200..=299, Some(body))) => return first_metadata(&body),
            _ => {}
        }
    }
    None
}

fn execute<B: Backend>(backend: &mut B, key: &str, kind: MediaKind, listed: Option<&Listed>, read: &mut Captured,
                       deleted: &mut dyn FnMut(Option<&EpisodeParent>, bool)) -> Result<DeleteOutcome, DeleteError>
{
    if !key_ok(key) { return Err(DeleteError::InvalidItem); }
    permitted(backend)?;
    let Some(item) = read_item(backend, key)? else {
        deleted(None, false);
        return Ok(gone(backend, kind, listed, read));
    };
    let expected = match kind { MediaKind::Movie => "movie", MediaKind::Show => "show", MediaKind::Episode => "episode" };
    if item.get("type").and_then(Value::as_str) != Some(expected) { return Err(DeleteError::InvalidItem); }
    let parent = if kind == MediaKind::Episode { episode_parent(&item) } else { None };
    let watched = number(&item, "viewCount").is_some_and(|n| n > 0);
    let show = if kind == MediaKind::Episode {
        item.get("grandparentRatingKey").and_then(Value::as_str).filter(|s| key_ok(s)).map(str::to_owned)
    } else { None };
    let id = if kind == MediaKind::Episode {
        let from_episode = item.get("grandparentGuid").and_then(Value::as_str).and_then(|g| catalog_id(g, kind));
        from_episode.or_else(|| show.as_deref().and_then(|s| metadata(backend, s))
            .and_then(|v| v.get("guid").and_then(Value::as_str).and_then(|g| catalog_id(g, kind))))
    } else {
        item.get("guid").and_then(Value::as_str).and_then(|g| catalog_id(g, kind))
    };
    // The server forgets the title once deleted, so what Radarr, Sonarr and Seerr are asked about is
    // read now (an episode is cleaned up as its show).
    let mut ids = Ids::of(&item);
    if kind == MediaKind::Episode {
        ids.title = item.get("grandparentTitle").and_then(Value::as_str).unwrap_or_default().to_owned();
        ids.year = 0;
    }
    if backend.connected().any() {
        let target = if kind == MediaKind::Episode { show.as_deref() } else { Some(key) };
        if let Some(record) = target.and_then(|t| read_guids(backend, t)) {
            let known = Ids::of(&record);
            ids = Ids { title: if known.title.is_empty() { ids.title } else { known.title }, ..known };
        }
    }
    *read = Captured { ids, watch_id: id.clone() };
    // Confirm the endpoint exists before interpreting a later 404 as an emptied/removed show.
    let leaves_known = show.as_deref().and_then(|s| episodes_remain(backend, s, false)).is_some();
    if !backend.still_current() { return Err(DeleteError::ServerProblem); }
    remove(backend, key)?;
    deleted(parent.as_ref(), watched);
    let outcome = |watchlist, cleanup| DeleteOutcome { watchlist, parent: parent.clone(), watched, already_gone: false, cleanup };
    if kind == MediaKind::Episode {
        let remaining = if leaves_known { show.as_deref().and_then(|s| episodes_remain(backend, s, true)) } else { None };
        match remaining {
            Some(true) => return Ok(outcome(WatchlistOutcome::Preserved, Cleanup::PARTIAL)),
            None => return Ok(outcome(WatchlistOutcome::Unknown, Cleanup::PARTIAL)),
            Some(false) => {},
        }
    }
    let watchlist = unlist(backend, id.as_deref());
    let cleanup = cleanup::run(backend, kind, &read.ids);
    Ok(outcome(watchlist, cleanup))
}

#[cfg(test)]
#[path = "delete_media_tests.rs"]
mod tests;
