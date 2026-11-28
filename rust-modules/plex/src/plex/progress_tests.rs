use super::super::arr::{Reply, Request, Verb};
use super::super::bazarr::TransportError;
use super::*;
use serde_json::json;
use std::cell::RefCell;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../../tests/fixtures/seerr/", $name))
    };
}

fn link() -> ArrLink {
    ArrLink { url: "https://arr.example.net".into(), api_key: "k".into() }
}

fn dl(stage: Stage, size: u64, left: u64, eta_s: Option<u32>, seasons: &[u32]) -> Download {
    Download { stage, size, left: Some(left), eta_s, seasons: seasons.to_vec(), client: "NZBGet".into(), identified: true }
}

const FILM: Key = Key { kind: Kind::Movie, tmdb: 603 };
const SHOW: Key = Key { kind: Kind::Tv, tmdb: 171802 };

// ---- parsing -------------------------------------------------------------------------------------

#[test]
fn a_dotnet_time_span_is_read_in_seconds() {
    assert_eq!(span("00:12:34"), Some(754));
    assert_eq!(span("1.02:03:04.5000000"), Some(93_784));
    assert_eq!(span("00:00:00"), Some(0));
    assert_eq!(span(""), None);
    assert_eq!(span("soon"), None);
}

#[test]
fn a_queue_record_gives_a_stage_a_floored_percent_and_the_apps_own_time_left() {
    let one = |v: Value| Download::of(&v);
    let moving = one(json!({"status": "downloading", "size": 3000, "sizeleft": 1000, "timeleft": "00:01:00", "downloadClient": "NZBGet"}));
    assert_eq!((moving.stage, moving.percent(), moving.eta_s, moving.client.as_str()), (Stage::Downloading, Some(66), Some(60), "NZBGet"));

    let done = one(json!({"status": "downloading", "size": 100, "sizeleft": 0, "timeleft": "00:00:00"}));
    assert_eq!((done.stage, done.eta_s), (Stage::Finishing, None), "every byte is in; the client is still working");

    let waiting = one(json!({"status": "completed", "trackedDownloadState": "importPending", "size": 100, "sizeleft": 0}));
    assert_eq!(waiting.stage, Stage::Importing);
    assert_eq!(one(json!({"status": "completed", "trackedDownloadState": "importBlocked"})).stage, Stage::Attention);
    assert_eq!(one(json!({"status": "completed", "trackedDownloadState": "imported"})).stage, Stage::Imported);
    assert_eq!(one(json!({"status": "failed"})).stage, Stage::Failed);
    assert_eq!(one(json!({"status": "downloading", "size": 100, "sizeleft": 50, "trackedDownloadStatus": "error"})).stage, Stage::Attention);

    let paused = one(json!({"status": "paused", "size": 100, "sizeleft": 90, "timeleft": "00:09:00"}));
    assert_eq!((paused.stage, paused.percent(), paused.eta_s), (Stage::Paused, Some(10), None), "no time is left while paused");

    let queued = one(json!({"status": "queued"}));
    assert_eq!((queued.stage, queued.percent()), (Stage::Queued, None), "no size, no percentage");
    assert_eq!(one(json!({"status": "downloading", "size": 100, "sizeleft": 200})).percent(), None);
    let missing = one(json!({"status":"downloading", "size":100}));
    assert_eq!((missing.stage, missing.percent()), (Stage::Downloading, None), "missing bytes left is not zero");
    assert_eq!(one(json!({"status": "downloading", "size": 100, "sizeleft": 50, "timeleft": "00:00:00"})).eta_s, None, "a zero time left is not known");
}

#[test]
fn the_episodes_of_a_pack_are_one_download() {
    let pack: Vec<Value> = [1, 2, 3]
        .iter()
        .map(|e| json!({"id": e, "downloadId": "ABC", "status": "downloading", "size": 900, "sizeleft": 300, "episode": {"seasonNumber": 1, "episodeNumber": e}}))
        .collect();
    let got = group(&pack);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].seasons.clone(), got[0].percent()), (vec![1], Some(66)));

    let other = json!({"id": 9, "downloadId": "XYZ", "status": "queued", "size": 100, "sizeleft": 100, "episode": {"seasonNumber": 2}});
    let mut both = pack;
    both.push(other);
    assert_eq!(group(&both).len(), 2, "another download id is another download");
}

#[test]
fn only_an_exact_id_can_merge_seerrs_queue_records() {
    let copy = |season: i64, id: Option<&str>| json!({"status": "downloading", "downloadId": id, "size": 1000,
        "sizeLeft": 400, "timeLeft": "00:10:00", "episode": {"seasonNumber": season}});
    let got = group(&[copy(2, Some("pack")), copy(3, Some("pack"))]);
    assert_eq!(got.len(), 1, "exact download id names the same pack");
    assert_eq!((got[0].seasons.as_slice(), got[0].percent(), got[0].eta_s), (&[2, 3][..], Some(60), Some(600)));
    let unknown = group(&[copy(2, None), copy(3, None)]);
    assert_eq!(unknown.len(), 2, "identical sizes and durations do not identify a download");
    assert_eq!(sum(&unknown).unwrap().percent, None, "cannot tell whether two records are a pack or two downloads");
}

#[test]
fn a_sum_needs_every_part_to_report_and_cannot_combine_individual_times() {
    let a = dl(Stage::Downloading, 1000, 500, Some(100), &[]);
    let b = dl(Stage::Downloading, 1000, 0, Some(50), &[]);
    let both = sum([&a, &b]).unwrap();
    assert_eq!((both.stage, both.percent, both.eta_s), (Stage::Downloading, Some(75), None));

    let unsized_ = dl(Stage::Queued, 0, 0, None, &[]);
    let with_unknown = sum([&a, &unsized_]).unwrap();
    assert_eq!((with_unknown.percent, with_unknown.eta_s), (None, None), "a part with no size or time makes it a guess");

    assert_eq!(sum(std::iter::empty::<&Download>()), None);
}

#[test]
fn seerrs_title_answer_gives_the_requests_the_tvdb_id_and_the_downloads() {
    let show = SeerrView::of(&serde_json::from_str(fixture!("tv_approved.json")).unwrap());
    assert_eq!(show.media, Have::Processing);
    assert_eq!(show.tvdb, Some(424855));
    assert_eq!(show.requests, vec![Req { status: Asked::Approved, seasons: vec![1] }]);
    assert!(show.downloads.is_empty());

    let film = SeerrView::of(&serde_json::from_str(fixture!("movie_available.json")).unwrap());
    assert_eq!(film.media, Have::All);
    let new = SeerrView::of(&serde_json::from_str(fixture!("movie_new.json")).unwrap());
    assert_eq!((new.media, new.requests.len()), (Have::Nothing, 0));

    let running = SeerrView::of(&json!({"mediaInfo": {"status": 3, "requests": [{"status": 2, "is4k": true}], "downloadStatus": [
        {"status": "downloading", "size": 100, "sizeLeft": 25, "timeLeft": "00:00:30"}]}}));
    assert!(running.requests.is_empty(), "a 4K request is not this title's");
    assert_eq!((running.downloads[0].percent(), running.downloads[0].eta_s), (Some(75), Some(30)));
}

// ---- asking the apps -----------------------------------------------------------------------------

struct Fake {
    routes: Vec<(&'static str, u16, &'static str)>,
    seen: RefCell<Vec<String>>,
}

impl ArrTransport for Fake {
    fn send(&self, _link: &ArrLink, request: &Request) -> Result<Reply, TransportError> {
        assert_eq!(request.verb, Verb::Get);
        self.seen.borrow_mut().push(request.path.clone());
        let (_, status, body) = self.routes.iter().find(|(p, ..)| *p == request.path).ok_or(TransportError::Unreachable)?;
        Ok(Reply { status: *status, body: body.as_bytes().to_vec() })
    }
}

fn fake(routes: Vec<(&'static str, u16, &'static str)>) -> Fake {
    Fake { routes, seen: RefCell::new(Vec::new()) }
}

fn order(key: Key, step: Step, arr_id: Option<i64>, tvdb: Option<u32>) -> Order {
    Order { gen: 1, key, step, arr_id, tvdb }
}

#[test]
fn radarr_is_asked_for_the_exact_tmdb_id_and_an_entry_of_another_id_is_not_taken() {
    let http = fake(vec![("/api/v3/movie?tmdbId=603", 200, r#"[{"id": 5, "tmdbId": 603, "hasFile": true}]"#)]);
    let got = run_arr(&link(), &http, &order(FILM, Step::Lookup, None, None));
    assert_eq!(got, Ok(Answer::Lookup(Some(Held { id: 5, have: 1, total: 1, seasons: vec![] }))));

    let wrong = fake(vec![("/api/v3/movie?tmdbId=603", 200, r#"[{"id": 6, "tmdbId": 999, "hasFile": true}]"#)]);
    assert_eq!(run_arr(&link(), &wrong, &order(FILM, Step::Lookup, None, None)), Ok(Answer::Lookup(None)));
    let empty = fake(vec![("/api/v3/movie?tmdbId=603", 200, "[]")]);
    assert_eq!(run_arr(&link(), &empty, &order(FILM, Step::Lookup, None, None)), Ok(Answer::Lookup(None)));
}

#[test]
fn sonarr_is_asked_by_the_tvdb_id_seerr_gave_and_reports_its_file_counts() {
    let body = r#"[{"id": 9, "tvdbId": 424855, "statistics": {"episodeFileCount": 2, "episodeCount": 8},
        "seasons": [{"seasonNumber": 0, "statistics": {"episodeFileCount": 0, "episodeCount": 0}},
                    {"seasonNumber": 1, "statistics": {"episodeFileCount": 2, "episodeCount": 8}}]}]"#;
    let http = fake(vec![("/api/v3/series?tvdbId=424855", 200, body)]);
    let got = run_arr(&link(), &http, &order(SHOW, Step::Lookup, None, Some(424855)));
    let held = Held {
        id: 9,
        have: 2,
        total: 8,
        seasons: vec![SeasonCount { number: 0, have: 0, total: 0 }, SeasonCount { number: 1, have: 2, total: 8 }],
    };
    assert_eq!(got, Ok(Answer::Lookup(Some(held))));

    let none = fake(vec![]);
    assert_eq!(run_arr(&link(), &none, &order(SHOW, Step::Lookup, None, None)), Err(Fail::Failed), "no tvdb id, no lookup");
    assert!(none.seen.borrow().is_empty());
}

#[test]
fn a_titles_queue_is_read_by_the_apps_own_id_and_a_pack_is_one_download() {
    const BODY: &str = r#"[
        {"id": 1, "downloadId": "P", "status": "downloading", "size": 900, "sizeleft": 450, "timeleft": "00:05:00", "downloadClient": "NZBGet", "episode": {"seasonNumber": 1}},
        {"id": 2, "downloadId": "P", "status": "downloading", "size": 900, "sizeleft": 450, "timeleft": "00:05:00", "downloadClient": "NZBGet", "episode": {"seasonNumber": 1}}]"#;
    let body = BODY;
    let http = fake(vec![("/api/v3/queue/details?seriesId=9&includeEpisode=true", 200, body), ("/api/v3/queue/details?movieId=5", 200, "[]")]);
    let got = run_arr(&link(), &http, &order(SHOW, Step::Queue, Some(9), Some(424855)));
    let Ok(Answer::Queue(downloads)) = got else { panic!("{got:?}") };
    assert_eq!((downloads.len(), downloads[0].percent(), downloads[0].eta_s), (1, Some(50), Some(300)));
    assert_eq!(run_arr(&link(), &http, &order(FILM, Step::Queue, Some(5), None)), Ok(Answer::Queue(vec![])));
}

#[test]
fn an_unlinked_refused_or_unreachable_app_is_told_apart() {
    let http = fake(vec![("/api/v3/movie?tmdbId=603", 401, "{}")]);
    let lookup = order(FILM, Step::Lookup, None, None);
    assert_eq!(run_arr(&link(), &http, &lookup), Err(Fail::Refused));
    assert_eq!(run_arr(&ArrLink::default(), &http, &lookup), Err(Fail::NotSet));
    assert_eq!(http.seen.borrow().len(), 1, "an unlinked app is not called");
    assert_eq!(run_arr(&link(), &fake(vec![("/api/v3/movie?tmdbId=603", 500, "{}")]), &lookup), Err(Fail::Failed));
    assert_eq!(run_arr(&link(), &fake(vec![]), &lookup), Err(Fail::Failed));
    assert_eq!(run_arr(&link(), &fake(vec![("/api/v3/movie?tmdbId=603", 200, "<html>")]), &lookup), Err(Fail::Failed));
}

#[test]
fn the_list_matches_exact_tmdb_ids_and_only_gives_numbers_for_an_unclipped_queue() {
    let body = r#"{"page": 1, "records": [
        {"id": 1, "downloadId": "a", "status": "downloading", "size": 100, "sizeleft": 40, "timeleft": "00:01:00", "movie": {"tmdbId": 603}},
        {"id": 2, "downloadId": "b", "status": "downloading", "size": 100, "sizeleft": 100, "timeleft": "00:02:00", "movie": {"tmdbId": 603}},
        {"id": 3, "downloadId": "c", "status": "downloading", "size": 100, "sizeleft": 10, "movie": {"title": "no id"}},
        {"id": 4, "downloadId": "d", "status": "queued", "movie": {"tmdbId": 11}}]}"#;
    let http = fake(vec![("/api/v3/queue?page=1&pageSize=100&includeMovie=true", 200, body)]);
    let got = run_list(&link(), &http, Kind::Movie).unwrap();
    assert_eq!(got.len(), 2, "a record that names no TMDB id is left out");
    let film = got.iter().find(|l| l.tmdb == 603).unwrap();
    assert_eq!((film.sum.stage, film.sum.percent, film.sum.eta_s), (Stage::Downloading, Some(30), None));
    assert_eq!(got.iter().find(|l| l.tmdb == 11).unwrap().sum.stage, Stage::Queued);

    let shows = fake(vec![("/api/v3/queue?page=1&pageSize=100&includeSeries=true", 200, r#"{"records": [{"id": 1, "status": "downloading", "size": 10, "sizeleft": 5, "series": {"tmdbId": 171802}}]}"#)]);
    assert_eq!(run_list(&link(), &shows, Kind::Tv).unwrap()[0].tmdb, 171802);
    assert_eq!(run_list(&ArrLink::default(), &shows, Kind::Tv), Err(Fail::NotSet));

    let clipped = fake(vec![("/api/v3/queue?page=1&pageSize=100&includeMovie=true", 200,
        r#"{"totalRecords": 200, "records": [{"status":"downloading", "size":10, "sizeleft":5, "movie":{"tmdbId":603}}]}"#)]);
    let got = run_list(&link(), &clipped, Kind::Movie).unwrap();
    assert_eq!((got[0].sum.stage, got[0].sum.percent), (Stage::Downloading, None));
}

// ---- the page's state ----------------------------------------------------------------------------

fn movie_view(media: Have) -> SeerrView {
    SeerrView { media, requests: vec![Req { status: Asked::Approved, seasons: vec![] }], ..SeerrView::default() }
}

fn show_view() -> SeerrView {
    SeerrView { media: Have::Processing, requests: vec![Req { status: Asked::Approved, seasons: vec![1, 2, 3] }], tvdb: Some(424855), ..SeerrView::default() }
}

/// Ask, answer: the step asked for and its answer, at `now`.
fn answer(t: &mut Tracker, now: u32, expect: Step, outcome: Outcome) {
    let o = t.advance(now).expect("a call is due");
    assert_eq!(o.step, expect);
    t.land(o.gen, o.step, outcome);
}

fn film_flow(downloads: Vec<Download>, held: Option<Held>, media: Have) -> Tracker {
    let mut t = Tracker::new(FILM, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(movie_view(media))));
    answer(&mut t, 0, Step::Lookup, Ok(Answer::Lookup(held.clone())));
    if held.is_some() {
        answer(&mut t, 0, Step::Queue, Ok(Answer::Queue(downloads)));
    }
    t
}

fn held(have: u32) -> Held {
    Held { id: 7, have, total: 1, seasons: vec![] }
}

#[test]
fn one_call_is_out_at_a_time_and_an_answer_for_another_page_is_dropped() {
    let mut t = Tracker::new(SHOW, 3);
    let first = t.advance(0).expect("Seerr is asked first");
    assert_eq!((first.step, first.gen), (Step::Seerr, 3));
    assert_eq!(t.advance(10), None, "one call at a time");
    assert!(!t.land(2, Step::Seerr, Ok(Answer::Seerr(show_view()))), "another tracker's answer");
    assert!(!t.land(3, Step::Lookup, Err(Fail::Failed)), "not the step asked");
    assert_eq!(t.advance(20), None, "still waiting for the call");
    assert!(t.land(3, Step::Seerr, Ok(Answer::Seerr(show_view()))));
    let next = t.advance(30).unwrap();
    assert_eq!((next.step, next.tvdb), (Step::Lookup, Some(424855)), "the lookup waits for Seerr's tvdb id");
}

#[test]
fn a_call_that_never_answers_is_given_up_on_and_asked_again() {
    let mut t = Tracker::new(SHOW, 1);
    assert!(t.advance(0).is_some());
    assert_eq!(t.advance(LOST_MS - 1), None);
    let again = t.advance(LOST_MS).expect("asked again");
    assert_eq!(again.step, Step::Seerr);
    assert!(!t.land(1, Step::Seerr, Ok(Answer::Seerr(show_view()))), "superseded call");
    assert!(t.land(again.gen, again.step, Ok(Answer::Seerr(show_view()))));
}

#[test]
fn nothing_is_asked_while_the_page_is_hidden_and_everything_is_due_when_it_returns() {
    let mut t = film_flow(vec![], Some(held(0)), Have::Processing);
    t.hide();
    assert_eq!(t.advance(10 * 60_000), None);
    t.show(10 * 60_000 + 5);
    let order = t.advance(10 * 60_000 + 5).expect("due at once");
    assert_eq!(order.step, Step::Seerr);
    let mut t = Tracker::new(SHOW, 1);
    let out = t.advance(0).unwrap();
    t.hide();
    assert!(!t.land(out.gen, out.step, Ok(Answer::Seerr(show_view()))), "a covered page retires pending work");
    t.show(5);
    let fresh = t.advance(5).unwrap();
    assert!(!t.land(out.gen, out.step, Ok(Answer::Seerr(show_view()))), "an old reply cannot land on return");
    assert!(t.land(fresh.gen, fresh.step, Ok(Answer::Seerr(show_view()))));
}

#[test]
fn a_failure_backs_off_and_a_busy_driver_is_asked_again_shortly() {
    let mut t = Tracker::new(SHOW, 1);
    answer(&mut t, 0, Step::Seerr, Err(Fail::Failed));
    assert_eq!(t.advance(4_999), None);
    answer(&mut t, 5_000, Step::Seerr, Err(Fail::Failed));
    assert_eq!(t.advance(14_999), None, "twice as long");
    let o = t.advance(15_000).unwrap();
    assert!(!t.land(o.gen, o.step, Err(Fail::Busy)), "busy is not a failure");
    assert_eq!(t.advance(15_499), None);
    assert_eq!(t.advance(15_500).unwrap().step, Step::Seerr);
}

#[test]
fn an_app_that_is_not_linked_or_refuses_the_key_is_not_asked_again() {
    let mut t = film_flow(vec![], None, Have::Processing);
    let next = t.advance(10 * 60_000).expect("something is due").step;
    assert_ne!(next, Step::Queue, "an app that does not hold the title has no queue to read");

    let mut t = Tracker::new(FILM, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(movie_view(Have::Processing))));
    let o = t.advance(0).unwrap();
    t.land(o.gen, o.step, Err(Fail::NotSet));
    assert_eq!(t.advance(0), None, "Seerr is not due again, the lookup is off, the queue has no id");
    assert_eq!(t.snapshot().trouble, None, "an unlinked app is not trouble");

    let mut t = Tracker::new(FILM, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(movie_view(Have::Processing))));
    answer(&mut t, 0, Step::Lookup, Err(Fail::Refused));
    assert_eq!(t.snapshot().trouble, Some(Trouble::Refused(Who::Arr(ArrKind::Radarr))));
    assert_eq!(t.advance(10 * 60_000).map(|o| o.step), Some(Step::Seerr));
}

#[test]
fn the_queue_is_read_every_three_seconds_while_bytes_move_and_seerr_every_thirty() {
    let mut t = film_flow(vec![dl(Stage::Downloading, 1000, 500, Some(600), &[])], Some(held(0)), Have::Processing);
    assert_eq!(t.advance(2_999), None);
    answer(&mut t, 3_000, Step::Queue, Ok(Answer::Queue(vec![dl(Stage::Downloading, 1000, 400, Some(500), &[])])));
    assert_eq!(t.advance(5_999), None);
    assert_eq!(t.advance(6_000).map(|o| o.step), Some(Step::Queue));
}

#[test]
fn a_film_downloading_shows_the_stage_the_percent_and_the_time_from_the_app() {
    let t = film_flow(vec![dl(Stage::Downloading, 1000, 500, Some(600), &[])], Some(held(0)), Have::Processing);
    let s = t.snapshot();
    assert_eq!((s.stage, s.percent, s.eta_s, s.from), (Some(Stage::Downloading), Some(50), Some(600), Some(Origin::Arr(ArrKind::Radarr))));
    assert_eq!((s.client.as_deref(), s.stale, s.age_s, s.loading, s.not_sent), (Some("NZBGet"), false, None, false, false));
}

#[test]
fn an_old_answer_keeps_its_percent_loses_its_time_and_says_how_old_it_is() {
    let mut t = film_flow(vec![dl(Stage::Downloading, 1000, 500, Some(600), &[])], Some(held(0)), Have::Processing);
    t.advance(QUEUE_STALE_MS + 5_000);
    let s = t.snapshot();
    assert_eq!((s.percent, s.eta_s, s.stale, s.age_s), (Some(50), None, true, Some(25)));
}

#[test]
fn the_apps_queue_wins_over_seerrs_copy_and_seerrs_copy_stands_in_when_the_app_is_down() {
    let mut seerr_copy = movie_view(Have::Processing);
    seerr_copy.downloads = vec![dl(Stage::Downloading, 1000, 800, Some(900), &[])];
    let mut t = Tracker::new(FILM, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(seerr_copy)));
    let s = t.snapshot();
    assert_eq!((s.percent, s.from), (Some(20), Some(Origin::Seerr)));

    answer(&mut t, 0, Step::Lookup, Err(Fail::Failed));
    let s = t.snapshot();
    assert_eq!((s.percent, s.from, s.trouble), (Some(20), Some(Origin::Seerr), Some(Trouble::Unreachable(Who::Arr(ArrKind::Radarr)))));

    answer(&mut t, 5_000, Step::Lookup, Ok(Answer::Lookup(Some(held(0)))));
    answer(&mut t, 5_000, Step::Queue, Ok(Answer::Queue(vec![dl(Stage::Downloading, 1000, 100, Some(30), &[])])));
    let s = t.snapshot();
    assert_eq!((s.percent, s.eta_s, s.from, s.trouble), (Some(90), Some(30), Some(Origin::Arr(ArrKind::Radarr)), None));
}

#[test]
fn a_film_the_app_has_but_seerr_has_not_seen_yet_is_in_the_library_and_seerr_is_asked_sooner() {
    let mut t = film_flow(vec![], Some(held(1)), Have::Processing);
    assert_eq!(t.snapshot().stage, Some(Stage::Imported));
    answer(&mut t, 6_000, Step::Queue, Ok(Answer::Queue(vec![])));
    assert_eq!(t.advance(9_999), None);
    assert_eq!(t.advance(10_000).map(|o| o.step), Some(Step::Seerr), "ten seconds after Seerr was last read, not thirty");
}

#[test]
fn a_film_seerr_calls_available_is_ready_whatever_the_queue_says() {
    let t = film_flow(vec![dl(Stage::Downloading, 1000, 500, Some(600), &[])], Some(held(1)), Have::All);
    let s = t.snapshot();
    assert_eq!((s.stage, s.percent, s.eta_s, s.client), (Some(Stage::Available), None, None, None), "an upgrade is not a download of the title");
}

#[test]
fn an_approved_film_the_app_does_not_hold_says_seerr_has_not_passed_it_on() {
    let t = film_flow(vec![], None, Have::Processing);
    let s = t.snapshot();
    assert_eq!((s.stage, s.not_sent), (Some(Stage::Approved), true));
    let waiting = film_flow(vec![], Some(held(0)), Have::Processing);
    assert_eq!((waiting.snapshot().stage, waiting.snapshot().not_sent), (Some(Stage::Approved), false));
}

#[test]
fn a_completed_request_says_nothing_when_seerr_no_longer_holds_the_film() {
    let mut view = movie_view(Have::Nothing);
    view.requests = vec![Req { status: Asked::Completed, seasons: vec![] }];
    let mut t = Tracker::new(FILM, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(view)));
    assert_eq!(t.snapshot().stage, None);
}

#[test]
fn nothing_has_answered_yet_is_loading_until_something_fails() {
    let mut t = Tracker::new(FILM, 1);
    assert!(t.snapshot().loading);
    answer(&mut t, 0, Step::Seerr, Err(Fail::Failed));
    let s = t.snapshot();
    assert!(!s.loading);
    assert_eq!(s.trouble, Some(Trouble::Unreachable(Who::Seerr)));
}

#[test]
fn a_show_gets_a_row_per_season_and_a_shared_pack_has_a_stage_but_no_number() {
    let mut t = Tracker::new(SHOW, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(show_view())));
    let counts = vec![
        SeasonCount { number: 1, have: 2, total: 10 },
        SeasonCount { number: 2, have: 0, total: 10 },
        SeasonCount { number: 3, have: 0, total: 0 },
    ];
    answer(&mut t, 0, Step::Lookup, Ok(Answer::Lookup(Some(Held { id: 9, have: 2, total: 20, seasons: counts }))));
    let queue = vec![dl(Stage::Downloading, 1000, 250, Some(300), &[1]), dl(Stage::Queued, 2000, 2000, None, &[2, 3])];
    answer(&mut t, 0, Step::Queue, Ok(Answer::Queue(queue)));

    let s = t.snapshot();
    assert_eq!((s.stage, s.percent, s.eta_s, s.library), (Some(Stage::Downloading), Some(25), None, Some((2, 20))));
    let rows: Vec<_> = s.seasons.iter().map(|r| (r.number, r.stage, r.percent, r.eta_s, r.have)).collect();
    assert_eq!(
        rows,
        vec![
            (1, Stage::Downloading, Some(75), Some(300), Some((2, 10))),
            (2, Stage::Queued, None, None, Some((0, 10))),
            (3, Stage::Queued, None, None, Some((0, 0))),
        ]
    );
}

#[test]
fn a_season_is_imported_when_the_app_has_every_episode_and_partial_when_it_has_some() {
    let mut view = show_view();
    view.requests = vec![Req { status: Asked::Approved, seasons: vec![1, 2] }];
    let mut t = Tracker::new(SHOW, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(view)));
    let counts = vec![SeasonCount { number: 1, have: 8, total: 8 }, SeasonCount { number: 2, have: 3, total: 8 }];
    answer(&mut t, 0, Step::Lookup, Ok(Answer::Lookup(Some(Held { id: 9, have: 11, total: 16, seasons: counts }))));
    answer(&mut t, 0, Step::Queue, Ok(Answer::Queue(vec![])));
    let s = t.snapshot();
    let rows: Vec<_> = s.seasons.iter().map(|r| (r.number, r.stage)).collect();
    assert_eq!(rows, vec![(1, Stage::Imported), (2, Stage::Partial)]);
    assert_eq!(s.stage, Some(Stage::Partial));
}

#[test]
fn a_show_seerr_gives_no_tvdb_id_for_is_never_looked_up_by_name() {
    let mut view = show_view();
    view.tvdb = None;
    let mut t = Tracker::new(SHOW, 1);
    answer(&mut t, 0, Step::Seerr, Ok(Answer::Seerr(view)));
    assert_eq!(t.advance(10 * 60_000).map(|o| o.step), Some(Step::Seerr));
    let s = t.snapshot();
    assert_eq!((s.stage, s.trouble), (Some(Stage::Approved), None));
}
