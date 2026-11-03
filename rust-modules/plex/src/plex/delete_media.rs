//! Destructive PMS deletion and account Watchlist cleanup. All calls block; use a worker.
use super::{account::AccountClient, catalog_hex_of_guid, Client, ClientKey};
use super::paging::{paged_path, PageReq};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind { Movie, Show, Episode }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteError {
    Denied,
    Unavailable,
    InvalidItem,
    Rejected(i32),
    /// The server did not confirm the DELETE. It may still have taken effect; do not retry blindly.
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchlistOutcome { Removed, NotListed, Preserved, Failed }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeleteOutcome { pub watchlist: WatchlistOutcome }

trait Backend {
    fn still_current(&self) -> bool;
    fn get(&mut self, path: &str) -> Option<(i32, Option<Value>)>;
    fn delete(&mut self, path: &str) -> Option<i32>;
    fn remove_watchlist(&mut self, hex: &str) -> Option<bool>;
}

struct Live<'a> { server: Client, original: &'a Client, key: ClientKey, account: Option<&'a AccountClient> }
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
        let status = self.server.delete(path);
        (status >= 0).then_some(status)
    }
    fn remove_watchlist(&mut self, hex: &str) -> Option<bool> {
        let account = self.account?;
        let state = account.user_state(hex, &mut account.watchlist_transport()).ok()?;
        if state.watchlisted_at.is_none() { return Some(false); }
        account.watchlist_remove(hex, &mut account.watchlist_transport()).ok()?;
        Some(true)
    }
}

fn allowed<B: Backend>(backend: &mut B) -> Result<bool, DeleteError> {
    let (status, body) = backend.get("/").ok_or(DeleteError::Unavailable)?;
    if matches!(status, 401 | 403) { return Ok(false); }
    if !(200..300).contains(&status) { return Err(DeleteError::Unavailable); }
    let body = body.ok_or(DeleteError::Unavailable)?;
    let value = body.get("MediaContainer").and_then(|v| v.get("allowMediaDeletion"))
        .ok_or(DeleteError::Unavailable)?;
    match value {
        Value::Bool(v) => Ok(*v),
        Value::Number(n) if n.as_i64() == Some(1) => Ok(true),
        Value::Number(n) if n.as_i64() == Some(0) => Ok(false),
        Value::String(s) if s == "1" || s.eq_ignore_ascii_case("true") => Ok(true),
        Value::String(s) if s == "0" || s.eq_ignore_ascii_case("false") => Ok(false),
        _ => Err(DeleteError::Unavailable),
    }
}

/// Read the permission of this exact server and its current credential. Unknown is not allowed.
pub fn permission(server: &Client, key: ClientKey) -> Result<bool, DeleteError> {
    if !super::server_facts(server.id()).is_some_and(|facts| facts.owned) { return Ok(false); }
    let captured = server.captured_for_key(key).ok_or(DeleteError::Unavailable)?;
    let mut live = Live { server: captured, original: server, key, account: None };
    let answer = allowed(&mut live)?;
    if !live.still_current() { return Err(DeleteError::Unavailable); }
    Ok(answer)
}

fn key_ok(key: &str) -> bool { !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) }
fn item_path(key: &str) -> String { format!("/library/metadata/{key}") }
fn metadata<B: Backend>(backend: &mut B, key: &str) -> Option<Value> {
    let (status, body) = backend.get(&item_path(key))?;
    if !(200..300).contains(&status) { return None; }
    body?.get("MediaContainer")?.get("Metadata")?.as_array()?.first().cloned()
}

fn catalog_id(guid: &str, kind: MediaKind) -> Option<String> {
    let prefix = match kind { MediaKind::Movie => "plex://movie/", _ => "plex://show/" };
    let hex = guid.strip_prefix(prefix)?;
    catalog_hex_of_guid(guid)?;
    Some(hex.to_owned())
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

/// Delete one item and then clean up the Watchlist. Call only after explicit user confirmation.
/// An `Ok` confirms deletion; `Failed` reports that subsequent Watchlist cleanup was not confirmed.
/// `Uncertain` means the DELETE might have succeeded and must not be repeated automatically.
pub fn delete(server: &Client, key: ClientKey, account: Option<&AccountClient>, rating_key: &str, kind: MediaKind)
    -> Result<DeleteOutcome, DeleteError>
{
    if !super::server_facts(server.id()).is_some_and(|facts| facts.owned) { return Err(DeleteError::Denied); }
    let captured = server.captured_for_key(key).ok_or(DeleteError::Unavailable)?;
    execute(&mut Live { server: captured, original: server, key, account }, rating_key, kind)
}

fn execute<B: Backend>(backend: &mut B, key: &str, kind: MediaKind) -> Result<DeleteOutcome, DeleteError> {
    if !key_ok(key) { return Err(DeleteError::InvalidItem); }
    if !allowed(backend)? { return Err(DeleteError::Denied); }
    let item = metadata(backend, key).ok_or(DeleteError::Unavailable)?;
    let expected = match kind { MediaKind::Movie => "movie", MediaKind::Show => "show", MediaKind::Episode => "episode" };
    if item.get("type").and_then(Value::as_str) != Some(expected) { return Err(DeleteError::InvalidItem); }
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
    // Confirm the endpoint exists before interpreting a later 404 as an emptied/removed show.
    let leaves_known = show.as_deref().and_then(|s| episodes_remain(backend, s, false)).is_some();
    if !backend.still_current() { return Err(DeleteError::Unavailable); }
    let path = item_path(key);
    match backend.delete(&path) {
        Some(status) if (200..300).contains(&status) => {},
        Some(status) if status < 500 => return Err(DeleteError::Rejected(status)),
        _ => match backend.get(&path) {
            Some((404, _)) => {},
            _ => return Err(DeleteError::Uncertain),
        },
    }
    if kind == MediaKind::Episode {
        let remaining = if leaves_known { show.as_deref().and_then(|s| episodes_remain(backend, s, true)) } else { None };
        match remaining {
            Some(true) => return Ok(DeleteOutcome { watchlist: WatchlistOutcome::Preserved }),
            None => return Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed }),
            Some(false) => {},
        }
    }
    let watchlist = match id.as_deref().and_then(|id| backend.remove_watchlist(id)) {
        Some(true) => WatchlistOutcome::Removed,
        Some(false) => WatchlistOutcome::NotListed,
        None => WatchlistOutcome::Failed,
    };
    Ok(DeleteOutcome { watchlist })
}

#[cfg(test)]
#[path = "delete_media_tests.rs"]
mod tests;
