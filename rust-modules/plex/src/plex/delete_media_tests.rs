use super::*;
use serde_json::json;
use std::collections::VecDeque;

const ID: &str = "0123456789abcdef01234567";

struct Fake {
    reads: VecDeque<(&'static str, Option<(i32, Option<Value>)>)>,
    delete_reply: Option<i32>,
    deleted: Vec<String>,
    watch_reply: Option<bool>,
    watch_ids: Vec<String>,
    current: bool,
}
impl Fake {
    fn new(reads: Vec<(&'static str, Option<(i32, Option<Value>)>)>) -> Self {
        Self { reads: reads.into(), delete_reply: Some(200), deleted: vec![], watch_reply: Some(true), watch_ids: vec![], current: true }
    }
}
impl Backend for Fake {
    fn still_current(&self) -> bool { self.current }
    fn get(&mut self, path: &str) -> Option<(i32, Option<Value>)> {
        let (expected, reply) = self.reads.pop_front().expect("unexpected read");
        assert_eq!(path, expected);
        reply
    }
    fn delete(&mut self, path: &str) -> Option<i32> {
        self.deleted.push(path.to_owned());
        self.delete_reply
    }
    fn remove_watchlist(&mut self, hex: &str) -> Option<bool> {
        self.watch_ids.push(hex.to_owned());
        self.watch_reply
    }
}
fn ok(value: Value) -> Option<(i32, Option<Value>)> { Some((200, Some(value))) }
fn permission(value: Value) -> (&'static str, Option<(i32, Option<Value>)>) {
    ("/", ok(json!({"MediaContainer": {"allowMediaDeletion": value}})))
}
fn movie(kind: &str, guid: &str) -> (&'static str, Option<(i32, Option<Value>)>) {
    ("/library/metadata/42", ok(json!({"MediaContainer":{"Metadata":[{"ratingKey":"42", "type":kind, "guid":guid}]}})))
}
fn episode() -> (&'static str, Option<(i32, Option<Value>)>) {
    ("/library/metadata/42", ok(json!({"MediaContainer":{"Metadata":[{"ratingKey":"42", "type":"episode", "grandparentRatingKey":"7"}]}})))
}
fn show() -> (&'static str, Option<(i32, Option<Value>)>) {
    ("/library/metadata/7", ok(json!({"MediaContainer":{"Metadata":[{"type":"show", "guid":format!("plex://show/{ID}")}]}})))
}
fn leaves(n: u32) -> (&'static str, Option<(i32, Option<Value>)>) {
    ("/library/metadata/7/allLeaves?X-Plex-Container-Start=0&X-Plex-Container-Size=1", ok(json!({"MediaContainer":{"size":n}})))
}

#[test]
fn permission_fail_closed_and_never_delete() {
    for (value, error) in [(json!(false), DeleteError::Denied), (json!(null), DeleteError::Unavailable)] {
        let mut fake = Fake::new(vec![permission(value)]);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(error));
        assert!(fake.deleted.is_empty());
    }
    let mut fake = Fake::new(vec![("/", None)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::Unavailable));
    assert!(fake.deleted.is_empty());
}

#[test]
fn movie_and_show_use_discover_guid_not_local_key() {
    for (kind, wire, guid_kind) in [(MediaKind::Movie, "movie", "movie"), (MediaKind::Show, "show", "show")] {
        let mut fake = Fake::new(vec![permission(json!(1)), movie(wire, &format!("plex://{guid_kind}/{ID}"))]);
        assert_eq!(execute(&mut fake, "42", kind), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Removed }));
        assert_eq!(fake.deleted, ["/library/metadata/42"]);
        assert_eq!(fake.watch_ids, [ID]);
    }
}

#[test]
fn episode_preserves_show_until_the_last_episode_is_gone() {
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(2), leaves(1)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Preserved }));
    assert!(fake.watch_ids.is_empty());
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(1), leaves(0)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Removed }));
    assert_eq!(fake.watch_ids, [ID]);
}

#[test]
fn unknown_episode_count_and_cleanup_failure_are_partial_success() {
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(1), ("/library/metadata/7/allLeaves?X-Plex-Container-Start=0&X-Plex-Container-Size=1", None)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed }));
    assert!(fake.watch_ids.is_empty());
    let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", &format!("plex://movie/{ID}"))]);
    fake.watch_reply = None;
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed }));
}

#[test]
fn rejected_or_unconfirmed_delete_never_cleans_watchlist() {
    for (reply, reread, expected) in [(Some(403), false, DeleteError::Rejected(403)), (None, true, DeleteError::Uncertain), (Some(500), true, DeleteError::Uncertain)] {
        let mut reads = vec![permission(json!(true)), movie("movie", &format!("plex://movie/{ID}"))];
        if reread { reads.push(movie("movie", &format!("plex://movie/{ID}"))); }
        let mut fake = Fake::new(reads);
        fake.delete_reply = reply;
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(expected));
        assert_eq!(fake.deleted.len(), 1);
        assert!(fake.watch_ids.is_empty());
    }
}

#[test]
fn missing_reply_and_absent_item_confirms_without_a_second_delete() {
    let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", &format!("plex://movie/{ID}")), ("/library/metadata/42", Some((404, None)))]);
    fake.delete_reply = None;
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Removed }));
    assert_eq!(fake.deleted.len(), 1);
}

#[test]
fn mismatched_kind_bad_identity_and_profile_change_do_not_delete() {
    let mut fake = Fake::new(vec![]);
    assert_eq!(execute(&mut fake, "42/children", MediaKind::Movie), Err(DeleteError::InvalidItem));
    let mut fake = Fake::new(vec![permission(json!(true)), movie("episode", "")]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::InvalidItem));
    let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", &format!("plex://movie/{ID}"))]);
    fake.current = false;
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::Unavailable));
    assert!(fake.deleted.is_empty());
}

#[test]
fn no_discover_guid_never_substitutes_local_key() {
    let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", "local://42")]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed }));
    assert!(fake.watch_ids.is_empty());
}

#[test]
fn credential_snapshot_rejects_a_retoken_without_network() {
    use crate::plex::{Origin, ServerId};
    let client = Client::new(ServerId::UNSET, "machine", Origin::http("127.0.0.1", 32400), "old", "test-client");
    let key = ClientKey::of(&client);
    let captured = client.captured_for_key(key).expect("matching credential");
    client.set_token("new");
    assert!(client.captured_for_key(key).is_none());
    assert!(captured.with_token("/x").ends_with("X-Plex-Token=old"));
}

#[test]
fn missing_ownership_never_contacts_server() {
    use crate::plex::{Origin, ServerId};
    let client = Client::new(ServerId::UNSET, "machine", Origin::http("127.0.0.1", 32400), "token", "test-client");
    let key = ClientKey::of(&client);
    assert_eq!(super::permission(&client, key), Ok(false));
    assert_eq!(super::delete(&client, key, None, "42", MediaKind::Movie), Err(DeleteError::Denied));
}

#[test]
fn absent_watchlist_entry_is_a_successful_cleanup() {
    let mut fake = Fake::new(vec![permission(json!(true)), movie("show", &format!("plex://show/{ID}"))]);
    fake.watch_reply = Some(false);
    assert_eq!(execute(&mut fake, "42", MediaKind::Show), Ok(DeleteOutcome { watchlist: WatchlistOutcome::NotListed }));
}

#[test]
fn legacy_agent_identity_reports_unresolved_without_wrong_watchlist_write() {
    for guid in ["com.plexapp.agents.imdb://tt1234567?lang=en", "imdb://tt1234567", "tmdb://1234"] {
        let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", guid)]);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed }));
        assert!(fake.watch_ids.is_empty());
    }
}
