use super::*;
use serde_json::json;
use std::collections::VecDeque;

const ID: &str = "0123456789abcdef01234567";

type Reply = Option<(i32, Option<Value>)>;
type Read = (&'static str, Reply);

struct Fake {
    reads: VecDeque<Read>,
    delete_reply: Option<i32>,
    deleted: Vec<String>,
    watch_reply: Option<bool>,
    watch_ids: Vec<String>,
    current: bool,
    noted: Vec<bool>,
    paused: Vec<u64>,
}
impl Fake {
    fn new(reads: Vec<Read>) -> Self {
        Self {
            reads: reads.into(), delete_reply: Some(200), deleted: vec![], watch_reply: Some(true),
            watch_ids: vec![], current: true, noted: vec![], paused: vec![],
        }
    }
}
impl Backend for Fake {
    fn still_current(&self) -> bool { self.current }
    fn get(&mut self, path: &str) -> Reply {
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
    fn note_deletion(&mut self, on: bool) { self.noted.push(on); }
    fn pause(&mut self, ms: u64) { self.paused.push(ms); }
}
fn ok(value: Value) -> Reply { Some((200, Some(value))) }
fn permission(value: Value) -> Read {
    ("/", ok(json!({"MediaContainer": {"allowMediaDeletion": value}})))
}
fn root_status(status: Option<i32>) -> Read { ("/", status.map(|s| (s, None))) }
fn item_status(status: Option<i32>) -> Read { ("/library/metadata/42", status.map(|s| (s, None))) }
fn movie(kind: &str, guid: &str) -> Read {
    ("/library/metadata/42", ok(json!({"MediaContainer":{"Metadata":[{"ratingKey":"42", "type":kind, "guid":guid}]}})))
}
fn the_movie() -> Read { movie("movie", &format!("plex://movie/{ID}")) }
fn episode_with(extra: Value) -> Read {
    let mut item = json!({"ratingKey":"42", "type":"episode", "grandparentRatingKey":"7"});
    item.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    ("/library/metadata/42", ok(json!({"MediaContainer":{"Metadata":[item]}})))
}
fn episode() -> Read { episode_with(json!({"parentIndex": 2, "viewCount": 1})) }
fn show() -> Read {
    ("/library/metadata/7", ok(json!({"MediaContainer":{"Metadata":[{"type":"show", "guid":format!("plex://show/{ID}")}]}})))
}
fn leaves(n: u32) -> Read {
    ("/library/metadata/7/allLeaves?X-Plex-Container-Start=0&X-Plex-Container-Size=1", ok(json!({"MediaContainer":{"size":n}})))
}
/// What a movie or show deletion reports: nothing about episodes.
fn done(watchlist: WatchlistOutcome) -> DeleteOutcome {
    DeleteOutcome { watchlist, parent: None, watched: false }
}
/// What `episode()`'s deletion reports.
fn episode_done(watchlist: WatchlistOutcome) -> DeleteOutcome {
    DeleteOutcome { watchlist, parent: Some(EpisodeParent { show_rk: "7".into(), season_index: 2 }), watched: true }
}

#[test]
fn a_server_that_does_not_say_deletion_is_allowed_is_never_asked_to_delete() {
    for (read, error, noted) in [
        (permission(json!(false)), DeleteError::NotAllowed, vec![false]),
        (permission(json!("false")), DeleteError::NotAllowed, vec![false]),
        (permission(json!(null)), DeleteError::NotAllowed, vec![false]),
        (root_status(Some(401)), DeleteError::NotAllowed, vec![false]),
        (root_status(Some(403)), DeleteError::NotAllowed, vec![false]),
        (root_status(Some(500)), DeleteError::ServerProblem, vec![]),
        (root_status(Some(200)), DeleteError::ServerProblem, vec![]),
        (root_status(None), DeleteError::ServerProblem, vec![]),
    ] {
        let mut fake = Fake::new(vec![read]);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(error));
        assert!(fake.deleted.is_empty());
        assert_eq!(fake.noted, noted, "what the menu is told about the setting");
    }
}

#[test]
fn movie_and_show_use_discover_guid_not_local_key() {
    for (kind, wire, guid_kind) in [(MediaKind::Movie, "movie", "movie"), (MediaKind::Show, "show", "show")] {
        let mut fake = Fake::new(vec![permission(json!(1)), movie(wire, &format!("plex://{guid_kind}/{ID}"))]);
        assert_eq!(execute(&mut fake, "42", kind), Ok(done(WatchlistOutcome::Removed)));
        assert_eq!(fake.deleted, ["/library/metadata/42"]);
        assert_eq!(fake.watch_ids, [ID]);
        assert_eq!(fake.noted, [true], "the setting the attempt read is kept for the menu");
    }
}

#[test]
fn a_204_or_a_404_from_the_delete_is_a_deletion() {
    for status in [200, 204, 404] {
        let mut fake = Fake::new(vec![permission(json!(true)), the_movie()]);
        fake.delete_reply = Some(status);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Removed)), "{status}");
        assert_eq!(fake.deleted, ["/library/metadata/42"]);
        assert_eq!(fake.watch_ids, [ID], "{status}: the Watchlist cleanup still follows");
        assert!(fake.paused.is_empty(), "{status}: nothing to wait for");
    }
}

#[test]
fn a_refusal_from_the_delete_is_not_allowed_and_withdraws_the_offer() {
    for status in [401, 403] {
        let mut fake = Fake::new(vec![permission(json!(true)), the_movie()]);
        fake.delete_reply = Some(status);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::NotAllowed), "{status}");
        assert_eq!(fake.deleted.len(), 1);
        assert_eq!(fake.noted, [true, false], "{status}: the menu stops offering Delete");
        assert!(fake.watch_ids.is_empty());
    }
}

#[test]
fn a_server_error_or_a_dropped_connection_is_a_server_problem_unless_the_item_is_found_gone() {
    for reply in [Some(500), Some(502), Some(503), Some(400), Some(409), None] {
        let reads = vec![permission(json!(true)), the_movie(), item_status(Some(200)), item_status(Some(200)), item_status(Some(200))];
        let mut fake = Fake::new(reads);
        fake.delete_reply = reply;
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::ServerProblem), "{reply:?}");
        assert_eq!(fake.deleted.len(), 1, "{reply:?}: never a second delete");
        assert_eq!(fake.paused, [SETTLE_PAUSE_MS; SETTLE_POLLS as usize], "{reply:?}: the item is watched for a few seconds");
        assert!(fake.watch_ids.is_empty());
    }
    // the server finished the job after the client gave up
    for reply in [Some(500), None] {
        let reads = vec![permission(json!(true)), the_movie(), item_status(Some(200)), item_status(Some(404))];
        let mut fake = Fake::new(reads);
        fake.delete_reply = reply;
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Removed)), "{reply:?}");
        assert_eq!(fake.paused, [SETTLE_PAUSE_MS; 2]);
        assert_eq!(fake.watch_ids, [ID]);
    }
}

#[test]
fn an_item_already_gone_is_a_success_without_a_delete() {
    let mut fake = Fake::new(vec![permission(json!(true)), item_status(Some(404))]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Skipped)));
    assert!(fake.deleted.is_empty());
    assert!(fake.watch_ids.is_empty());
}

#[test]
fn the_item_read_maps_each_status_to_what_the_person_is_told() {
    let no_metadata = ("/library/metadata/42", ok(json!({"MediaContainer":{"size":0}})));
    for (read, error) in [
        (item_status(Some(401)), DeleteError::NotAllowed),
        (item_status(Some(403)), DeleteError::NotAllowed),
        (item_status(Some(500)), DeleteError::ServerProblem),
        (item_status(None), DeleteError::ServerProblem),
        (item_status(Some(200)), DeleteError::ServerProblem),
        (no_metadata, DeleteError::ServerProblem),
    ] {
        let mut fake = Fake::new(vec![permission(json!(true)), read]);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(error));
        assert!(fake.deleted.is_empty());
    }
}

#[test]
fn episode_preserves_show_until_the_last_episode_is_gone() {
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(2), leaves(1)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(episode_done(WatchlistOutcome::Preserved)));
    assert!(fake.watch_ids.is_empty());
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(1), leaves(0)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(episode_done(WatchlistOutcome::Removed)));
    assert_eq!(fake.watch_ids, [ID]);
}

#[test]
fn a_deleted_episode_reports_its_show_season_and_watched_state() {
    let run = |read: Read| {
        let mut fake = Fake::new(vec![permission(json!(true)), read, show(), leaves(2), leaves(1)]);
        execute(&mut fake, "42", MediaKind::Episode).unwrap()
    };
    let parent = |season_index| Some(EpisodeParent { show_rk: "7".into(), season_index });
    let played = run(episode_with(json!({"parentIndex": 3, "viewCount": 2})));
    assert_eq!((played.parent, played.watched), (parent(3), true));
    let unplayed = run(episode_with(json!({"parentIndex": 0})));
    assert_eq!((unplayed.parent, unplayed.watched), (parent(0), false), "season 0 is the specials");
    let string_numbers = run(episode_with(json!({"parentIndex": "4", "viewCount": "1"})));
    assert_eq!((string_numbers.parent, string_numbers.watched), (parent(4), true));
    let no_season = run(episode_with(json!({"viewCount": 1})));
    assert_eq!(no_season.parent, None, "without a season the counts above it are left to the refresh");
}

#[test]
fn unknown_episode_count_and_cleanup_failure_are_partial_success() {
    let mut fake = Fake::new(vec![permission(json!(true)), episode(), show(), leaves(1), ("/library/metadata/7/allLeaves?X-Plex-Container-Start=0&X-Plex-Container-Size=1", None)]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Episode), Ok(episode_done(WatchlistOutcome::Failed)));
    assert!(fake.watch_ids.is_empty());
    let mut fake = Fake::new(vec![permission(json!(true)), the_movie()]);
    fake.watch_reply = None;
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Failed)));
}

#[test]
fn mismatched_kind_bad_identity_and_profile_change_do_not_delete() {
    let mut fake = Fake::new(vec![]);
    assert_eq!(execute(&mut fake, "42/children", MediaKind::Movie), Err(DeleteError::InvalidItem));
    let mut fake = Fake::new(vec![permission(json!(true)), movie("episode", "")]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::InvalidItem));
    let mut fake = Fake::new(vec![permission(json!(true)), the_movie()]);
    fake.current = false;
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Err(DeleteError::ServerProblem));
    assert!(fake.deleted.is_empty());
}

#[test]
fn no_discover_guid_never_substitutes_local_key() {
    let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", "local://42")]);
    assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Failed)));
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
    assert_eq!(super::delete(&client, key, None, "42", MediaKind::Movie), Err(DeleteError::NotAllowed));
}

#[test]
fn absent_watchlist_entry_is_a_successful_cleanup() {
    let mut fake = Fake::new(vec![permission(json!(true)), movie("show", &format!("plex://show/{ID}"))]);
    fake.watch_reply = Some(false);
    assert_eq!(execute(&mut fake, "42", MediaKind::Show), Ok(done(WatchlistOutcome::NotListed)));
}

#[test]
fn legacy_agent_identity_reports_unresolved_without_wrong_watchlist_write() {
    for guid in ["com.plexapp.agents.imdb://tt1234567?lang=en", "imdb://tt1234567", "tmdb://1234"] {
        let mut fake = Fake::new(vec![permission(json!(true)), movie("movie", guid)]);
        assert_eq!(execute(&mut fake, "42", MediaKind::Movie), Ok(done(WatchlistOutcome::Failed)));
        assert!(fake.watch_ids.is_empty());
    }
}
