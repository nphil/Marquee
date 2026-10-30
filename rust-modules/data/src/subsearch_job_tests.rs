//! A subtitle-search request is a typed job the store emits, not a thread it starts
//! (`docs/sans-io.md`), and which client a server holds is an input to the step, not a read of the
//! registry: what a pump emits and when, what the job says, how its answer or refusal comes back,
//! and what a refusal does to the pump that sees it.

use super::*;
use crate::stores::subsearch::SubSearchStore;
use crate::stores::StoreJob;
use plx_machine::landgate::Gate;

const KEY: ClientKey = ClientKey { instance: 7, token: 3 };

fn sid() -> ServerId { ServerId::from_raw(1) }

/// The registry's answer when the server holds the client `KEY` names.
fn holding() -> ClientKeys { ClientKeys::from_keys([(sid(), KEY)]) }

fn hit(key: &str) -> SubHit {
    SubHit { key: key.into(), title: "rel".into(), provider: "OpenSubtitles".into(),
        language: "Nederlands".into(), language_code: "nld".into(), codec: "srt".into(),
        score: 5, hearing_impaired: false, forced: false }
}

/// A model opened on item `506548` of the server, and the mailbox its requests answer into.
struct Rig {
    state: SubSearchState,
    adapter: Arc<SubSearchAdapter>,
}

impl Rig {
    fn opened(lang: &str) -> Self {
        let adapter = Arc::new(SubSearchAdapter::default());
        let mut state = SubSearchState::default();
        assert!(state.run(&adapter, SubSearchCmd::Open { sid: sid(), rk: "506548".into(), lang: lang.into() }));
        assert!(state.jobs.is_empty(), "a command emits no job: the request is the pump's");
        Rig { state, adapter }
    }

    /// One pump with the registry's answer `keys`, and the requests it emitted.
    fn pump(&mut self, keys: &ClientKeys) -> Vec<StoreJob> {
        self.state.pump_with_gate(&self.adapter, &Gate::default(), keys);
        std::mem::take(&mut self.state.jobs)
    }

    fn run(&mut self, cmd: SubSearchCmd) -> bool { self.state.run(&self.adapter, cmd) }

    fn port(&self) -> SubSearchPort { SubSearchPort::of(&self.adapter) }

    /// The job the pump emits for `work` at `generation`, as the step builds it.
    fn job(&self, generation: u32, work: Work) -> StoreJob {
        StoreJob::SubtitleSearch(
            Job { addr: subsearch_addr(generation), req: SubSearchReq { sid: sid(), key: KEY, rk: "506548".into(), work, link: None } },
            self.port())
    }

    /// The one request the pump emitted, taken apart.
    fn one(&mut self, keys: &ClientKeys) -> (Job<SubSearchReq>, SubSearchPort) {
        let mut jobs = self.pump(keys);
        assert_eq!(jobs.len(), 1, "one request is out at a time");
        let StoreJob::SubtitleSearch(job, port) = jobs.remove(0) else { panic!("not a subtitle-search job") };
        (job, port)
    }

    fn claim_held(&self) -> bool { self.adapter.busy_for_test() }
}

fn search(lang: &str) -> Work { Work::Search { lang: lang.into() } }

/// A pump over an opened search emits the request and says which client it decided on; a command
/// emits nothing; and one request is out at a time, so a second pump emits nothing until the first
/// is answered.
#[test]
fn an_open_search_emits_one_request_naming_the_client_it_decided_on() {
    let mut rig = Rig::opened("nld");
    let generation = rig.state.generation();

    plx_machine::assert_fx!(rig.pump(&holding()), [rig.job(generation, search("nl"))]);
    assert!(rig.claim_held(), "the request holds the claim while it is out");

    plx_machine::assert_fx!(rig.pump(&holding()), []);
}

/// The request names the item and the work, a pure function of the model and the registry's
/// answer: search, then the download the viewer pressed, then the install poll that follows it,
/// each as the answer to the one before.
#[test]
fn each_answer_makes_the_next_pump_emit_the_next_request() {
    let mut rig = Rig::opened("nl");
    let generation = rig.state.generation();
    plx_machine::assert_fx!(rig.pump(&holding()), [rig.job(generation, search("nl"))]);

    rig.adapter.land_for_test(generation, Landing::Search(Ok(vec![hit("/library/streams/1")])));
    plx_machine::assert_fx!(rig.pump(&holding()), []); // the answer is shown; nothing more is owed
    assert!(!rig.claim_held(), "the answer released the claim");

    assert!(rig.run(SubSearchCmd::Download { gen: generation, hit: 0 }));
    plx_machine::assert_fx!(rig.pump(&holding()),
        [rig.job(generation, Work::Download { key: "/library/streams/1".into() })]);

    // accepted: the install is asynchronous, so the poll waits a beat before it reads
    rig.adapter.land_for_test(generation, Landing::Download(Ok(vec![10])));
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    for _ in 1..POLL_FRAMES { plx_machine::assert_fx!(rig.pump(&holding()), []); }
    plx_machine::assert_fx!(rig.pump(&holding()), [rig.job(generation, Work::Refresh)]);
}

/// A server that holds no client has nothing to ask: no job, no claim, and the next attempt is
/// held off for a stretch of frames, then made at once when the server has a client again.
#[test]
fn a_server_without_a_client_emits_nothing_and_holds_off() {
    let mut rig = Rig::opened("nl");
    let generation = rig.state.generation();
    let none = ClientKeys::default();

    plx_machine::assert_fx!(rig.pump(&none), []);
    assert!(!rig.claim_held(), "no claim was made");
    assert_eq!(rig.state.retry_for_test(), RETRY_FRAMES);

    // The client appears mid-wait: the hold-off still runs its course.
    for _ in 1..RETRY_FRAMES { plx_machine::assert_fx!(rig.pump(&holding()), []); }
    plx_machine::assert_fx!(rig.pump(&holding()), [rig.job(generation, search("nl"))]);
}

/// A refusal hands the request's claim back in the call that refuses it. The pump after it reads
/// the refusal BEFORE it decides to spawn and backs off: with only the release it would find the
/// claim free and emit the same request again, every pump.
#[test]
fn a_refused_request_is_asked_again_only_after_the_backoff() {
    let mut rig = Rig::opened("nl");
    let generation = rig.state.generation();
    let (job, port) = rig.one(&holding());
    assert!(rig.claim_held());

    port.refused(job.addr);
    assert!(!rig.claim_held(), "the refusal released the claim in the same call");
    assert!(!rig.adapter.has_mail_for_test(), "a refusal answers nothing");

    // The refusal is seen one pump after it is made, and holds the full wait.
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    assert_eq!(rig.state.retry_for_test(), RETRY_FRAMES, "the pump that saw it backed off");
    for _ in 1..RETRY_FRAMES { plx_machine::assert_fx!(rig.pump(&holding()), []); }
    plx_machine::assert_fx!(rig.pump(&holding()), [rig.job(generation, search("nl"))]);
    assert!(rig.claim_held(), "...and the request that follows claims again");
}

/// A refusal is observed once: a request refused, asked again and answered is not backed off a
/// second time by the memory of the first.
#[test]
fn a_refusal_backs_off_one_request_and_not_the_one_after() {
    let mut rig = Rig::opened("nl");
    let generation = rig.state.generation();
    let (job, port) = rig.one(&holding());
    port.refused(job.addr);
    for _ in 0..RETRY_FRAMES { let _ = rig.pump(&holding()); }
    let (job, _) = rig.one(&holding()); // the second attempt
    assert_eq!(job.addr.req.0, generation);

    rig.adapter.land_for_test(generation, Landing::Search(Ok(vec![hit("/library/streams/1")])));
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    assert_eq!(rig.state.retry_for_test(), 0, "the answered request left no backoff behind");
}

/// A refusal that arrives after the search has moved on leaves the newer claim alone, and the
/// newer request is not backed off for it: the runner refuses a job on its own schedule, and by
/// then the viewer may have changed the language.
#[test]
fn a_refusal_for_a_superseded_request_leaves_the_newer_one_alone() {
    let mut rig = Rig::opened("nl");
    let (old, old_port) = rig.one(&holding());

    assert!(rig.run(SubSearchCmd::SetLanguage("en".into()))); // supersedes the first search
    let (current, _) = rig.one(&holding());
    assert_ne!(old.addr, current.addr);

    old_port.refused(old.addr);
    assert!(rig.claim_held(), "the old request's refusal did not release the claim the new one holds");
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    assert_eq!(rig.state.retry_for_test(), 0, "...nor did it back the new request off");
}

/// An answer is stamped with its request's generation, so the pump applies the current search's
/// answer and discards one for a search it has since superseded.
#[test]
fn an_answer_applies_to_its_own_search_and_a_superseded_one_is_dropped() {
    let mut rig = Rig::opened("nl");
    let (stale, stale_port) = rig.one(&holding());
    assert!(rig.run(SubSearchCmd::SetLanguage("en".into())));
    let (current, current_port) = rig.one(&holding());

    stale_port.done(stale.addr, SubSearchAnswer(Landing::Search(Ok(vec![hit("/library/streams/9")]))));
    rig.state.pump_with_gate(&rig.adapter, &Gate::default(), &holding());
    assert_eq!(rig.state.view().status(), SearchStatus::Searching, "a superseded search's answer lands nowhere");
    assert!(rig.claim_held(), "...and does not release the claim of the request still out");

    current_port.done(current.addr, SubSearchAnswer(Landing::Search(Ok(vec![hit("/library/streams/1")]))));
    assert!(rig.state.pump_with_gate(&rig.adapter, &Gate::default(), &holding()));
    assert_eq!(rig.state.view().status(), SearchStatus::Ready);
    assert_eq!(rig.state.view().hits().len(), 1);
}

/// A job's port is captured when the job is emitted. A `Reset` between the request and the runner
/// rotates the store's adapter, and the job still answers into the retired one, where nothing is
/// pumped, never into the replacement's landing.
#[test]
fn a_job_emitted_before_a_reset_answers_into_the_retired_adapter() {
    let mut store = SubSearchStore::default();
    assert!(!store.has_jobs());
    assert!(store.run(SubSearchCmd::Open { sid: sid(), rk: "506548".into(), lang: "nl".into() }));
    assert!(!store.has_jobs(), "a command emits no job");
    store.pump(&Gate::default(), &holding());
    assert!(store.has_jobs());
    let mut jobs = store.take_jobs();
    assert!(!store.has_jobs(), "taking the jobs empties the outbox");
    assert_eq!(jobs.len(), 1);
    let StoreJob::SubtitleSearch(job, port) = jobs.remove(0) else { panic!("not a subtitle-search job") };
    assert_eq!(port, store.subsearch_port());

    store.run(SubSearchCmd::Reset);
    assert_ne!(port, store.subsearch_port(), "the job holds the retired adapter's port");
    assert!(store.run(SubSearchCmd::Open { sid: sid(), rk: "506548".into(), lang: "nl".into() }));

    port.done(job.addr, SubSearchAnswer(Landing::Search(Ok(vec![]))));
    assert!(!store.adapter_for_test().has_mail_for_test(), "the replacement's landing never sees the answer");
}

// ---- Bazarr, and "Wrong subtitles?" ---------------------------------------------------------------

fn bazarr_link() -> BazarrLink {
    BazarrLink { url: "https://bazarr.example.net".into(), api_key: "SECRET-KEY-0123456789abcdef".into() }
}

fn candidate(provider: &str, language: &str, score: i64) -> Candidate {
    Candidate { provider: provider.into(), subtitle: format!("handle-{provider}"), release: format!("rel-{provider}"),
        language: language.into(), hearing_impaired: false, forced: false, score, original_format: false }
}

fn open_bazarr(rig: &mut Rig, lang: &str) -> bool {
    rig.run(SubSearchCmd::OpenBazarr { sid: sid(), rk: "506548".into(), lang: lang.into() })
}

/// A Bazarr search with no Bazarr set up fails at once and says why, and asks nobody anything.
#[test]
fn a_bazarr_search_without_a_link_fails_at_once_and_owes_no_request() {
    let mut rig = Rig::opened("nl");
    assert!(open_bazarr(&mut rig, "nl"));
    assert_eq!(rig.state.view().source(), SubSource::Bazarr);
    assert_eq!(rig.state.view().status(), SearchStatus::Failed);
    assert_eq!(rig.state.view().failure(), Some(SearchFailure::NotSet));
    assert!(rig.state.work_for_test().is_none());
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    assert!(!SearchFailure::NotSet.retryable(), "a retry cannot supply a missing key");
}

/// The request for Bazarr carries the link and no other request does; the link never shows in the
/// request's `Debug`, in the snapshot a screen holds, or in the view, which has only the host.
#[test]
fn the_link_rides_only_the_bazarr_request_and_never_shows() {
    let mut rig = Rig::opened("nl");
    assert!(rig.state.set_link(&bazarr_link()));
    assert!(!rig.state.set_link(&bazarr_link()), "the same link is not a change");
    let generation = rig.state.generation();
    let (plain, _) = rig.one(&holding());
    assert_eq!(plain.req.link, None, "an agent search carries no link");
    rig.adapter.land_for_test(generation, Landing::Search(Ok(vec![])));
    rig.pump(&holding());

    assert!(open_bazarr(&mut rig, "nl"));
    let (job, _) = rig.one(&holding());
    assert_eq!(job.req.work, Work::BazarrSearch { lang: "nl".into() });
    assert_eq!(job.req.link, Some(bazarr_link()));
    let printed = format!("{:?}", job.req);
    assert!(!printed.contains("SECRET-KEY") && printed.contains("bazarr.example.net"), "{printed}");

    let view = rig.state.view();
    assert_eq!(view.bazarr_host(), "bazarr.example.net");
    let snapshot = format!("{:?}", view.snapshot_for(sid(), "506548"));
    assert!(!snapshot.contains("SECRET-KEY") && snapshot.contains("bazarr.example.net"), "{snapshot}");
}

/// Candidates become rows in the order Bazarr ranked them, and a download names the WHOLE
/// candidate and where Bazarr holds the item, then polls the item like an agent download does.
#[test]
fn bazarr_candidates_are_rows_and_a_download_names_the_whole_candidate() {
    let mut rig = Rig::opened("nl");
    rig.state.set_link(&bazarr_link());
    assert!(open_bazarr(&mut rig, "nl"));
    let generation = rig.state.generation();
    assert!(rig.state.work_for_test().is_some(), "a Bazarr search is owed");
    let (_, _) = rig.one(&holding());

    let target = Target::Movie { radarr_id: 7 };
    let mut hi = candidate("b", "nl", 80);
    hi.hearing_impaired = true;
    let ranked = vec![candidate("a", "nl", 91), hi, candidate("c", "nl-BE", 40)];
    rig.adapter.land_for_test(generation, Landing::Bazarr(Ok((target, ranked.clone()))));
    rig.pump(&holding());
    let view = rig.state.view();
    assert_eq!(view.status(), SearchStatus::Ready);
    let rows: Vec<(i64, &str, bool)> = view.hits().iter().map(|h| (h.score, h.provider.as_str(), h.hearing_impaired)).collect();
    assert_eq!(rows, [(91, "a", false), (80, "b", true), (40, "c", false)]);
    assert_eq!(view.hits()[0].title, "rel-a");

    assert!(rig.run(SubSearchCmd::Download { gen: generation, hit: 1 }));
    assert_eq!(rig.state.work_for_test(), Some(Work::BazarrDownload { target, candidate: ranked[1].clone() }));
    let (job, _) = rig.one(&holding());
    assert_eq!(job.req.link, Some(bazarr_link()), "the download needs the link too");

    rig.adapter.land_for_test(generation, Landing::Download(Ok(vec![10])));
    rig.pump(&holding());
    for _ in 1..POLL_FRAMES { rig.pump(&holding()); }
    assert_eq!(rig.state.work_for_test(), Some(Work::Refresh));
}

/// Every Bazarr fault keeps its kind, and only a failure to REACH it offers a retry.
#[test]
fn a_bazarr_failure_keeps_its_kind_and_only_unreachable_retries() {
    let mut rig = Rig::opened("nl");
    rig.state.set_link(&bazarr_link());
    assert!(open_bazarr(&mut rig, "nl"));
    let generation = rig.state.generation();
    for (failure, retries) in [(SearchFailure::BadKey, false), (SearchFailure::NotBazarr, false),
        (SearchFailure::NotInBazarr, false), (SearchFailure::Refused, false), (SearchFailure::Unreachable, true)] {
        assert!(rig.state.apply_for_test(Landing::Bazarr(Err(failure))));
        assert_eq!(rig.state.view().failure(), Some(failure));
        assert_eq!(rig.run(SubSearchCmd::Retry), retries, "{failure:?}");
        if retries {
            assert_eq!(rig.state.view().status(), SearchStatus::Searching);
        } else {
            assert_eq!(rig.state.view().status(), SearchStatus::Failed);
        }
        assert_eq!(rig.state.generation(), generation);
        // back to searching, for the next kind
        rig.state.open.as_mut().unwrap().status = SearchStatus::Searching;
    }
}

/// The other source on the same item searches anew, and the removal is the item's, not the search's.
#[test]
fn another_source_on_the_same_item_searches_anew() {
    let mut rig = Rig::opened("nl");
    rig.state.set_link(&bazarr_link());
    let generation = rig.state.generation();
    rig.state.apply_for_test(Landing::Search(Ok(vec![hit("/library/streams/1")])));
    assert_eq!(rig.state.view().status(), SearchStatus::Ready);
    assert!(open_bazarr(&mut rig, "nl"));
    assert_eq!(rig.state.view().status(), SearchStatus::Searching);
    assert_eq!(rig.state.view().source(), SubSource::Bazarr);
    assert!(rig.state.view().hits().is_empty(), "the agent's rows are not Bazarr's");
    assert_ne!(rig.state.generation(), generation, "a landing for the first search is dropped");
    assert!(!open_bazarr(&mut rig, "nl"), "the same source and language keeps its answer");
}

/// Setting Bazarr up after a search failed for want of it lets the same page search again.
#[test]
fn a_search_that_failed_for_want_of_a_link_searches_once_one_is_set() {
    let mut rig = Rig::opened("nl");
    assert!(open_bazarr(&mut rig, "nl"));
    assert_eq!(rig.state.view().failure(), Some(SearchFailure::NotSet));
    assert!(!open_bazarr(&mut rig, "nl"), "still no link: nothing changes");
    rig.state.set_link(&bazarr_link());
    assert!(open_bazarr(&mut rig, "nl"));
    assert_eq!(rig.state.view().status(), SearchStatus::Searching);
}

/// Fixing an item asks nothing; a removal is one request, its answer is one of three, and while it
/// is out nothing else about the item changes.
#[test]
fn a_removal_is_one_request_with_three_answers_and_holds_the_item() {
    let adapter = Arc::new(SubSearchAdapter::default());
    let mut rig = Rig { state: SubSearchState::default(), adapter };
    assert!(rig.run(SubSearchCmd::Fix { sid: sid(), rk: "506548".into() }), "the item is held");
    assert!(!rig.run(SubSearchCmd::Fix { sid: sid(), rk: "506548".into() }), "the same item keeps it");
    plx_machine::assert_fx!(rig.pump(&holding()), []);
    assert_eq!(rig.state.view().status(), SearchStatus::Idle);

    let generation = rig.state.generation();
    assert!(rig.run(SubSearchCmd::Remove { stream: 5, key: "/library/streams/5".into() }));
    assert!(!rig.run(SubSearchCmd::Remove { stream: 6, key: "/library/streams/6".into() }), "one at a time");
    assert!(!rig.run(SubSearchCmd::OpenBazarr { sid: sid(), rk: "506548".into(), lang: "nl".into() }),
        "a removal in flight holds a search back");
    plx_machine::assert_fx!(rig.pump(&holding()),
        [rig.job(generation, Work::Remove { stream: 5, key: "/library/streams/5".into() })]);

    rig.adapter.land_for_test(generation, Landing::Removed(Ok(())));
    rig.pump(&holding());
    assert_eq!(rig.state.view().removal(), Some(&RemovePhase::Removed { stream: 5 }));
    for (answer, phase) in [(Err(true), RemovePhase::Refused { stream: 7 }), (Err(false), RemovePhase::Failed { stream: 7 })] {
        assert!(rig.run(SubSearchCmd::Remove { stream: 7, key: "/library/streams/7".into() }));
        rig.state.apply_for_test(Landing::Removed(answer));
        assert_eq!(rig.state.view().removal(), Some(&phase));
    }
}

/// A tiny Bazarr that answers by path prefix and writes down what it was asked.
struct Canned {
    routes: Vec<(&'static str, u16, &'static str)>,
    seen: std::cell::RefCell<Vec<bazarr::Request>>,
}

impl Canned {
    fn new(routes: Vec<(&'static str, u16, &'static str)>) -> Self {
        Self { routes, seen: Default::default() }
    }
}

impl Transport for Canned {
    fn send(&self, _link: &BazarrLink, request: &bazarr::Request) -> Result<bazarr::Reply, bazarr::TransportError> {
        self.seen.borrow_mut().push(request.clone());
        let route = self.routes.iter().find(|(p, _, _)| request.path.starts_with(p));
        Ok(match route {
            Some((_, status, body)) => bazarr::Reply { status: *status, body: body.as_bytes().to_vec() },
            None => bazarr::Reply { status: 404, body: Vec::new() },
        })
    }
}

const SEARCHES: &str = r#"[{"title":"Tears of Steel","year":"2012","radarrId":7}]"#;
const OFFERS: &str = r#"{"data":[
  {"provider":"a","subtitle":"h1","language":"nl","hearing_impaired":"False","forced":"False","score":70,"release_info":["x"]},
  {"provider":"b","subtitle":"h2","language":"en","hearing_impaired":"True","forced":"False","score":95,"release_info":["y"]},
  {"provider":"c","subtitle":"h3","language":"en","hearing_impaired":"False","forced":"False","score":80,"release_info":["z"]}]}"#;

fn tears() -> ItemFacts {
    ItemFacts { title: "Tears of Steel".into(), year: 2012, ..Default::default() }
}

/// The search half: the language asked for, best score first, and a title Bazarr lacks is its own failure.
#[test]
fn the_bazarr_search_keeps_the_language_asked_for_best_score_first() {
    let link = bazarr_link();
    let http = Canned::new(vec![("/api/system/searches", 200, SEARCHES), ("/api/providers/movies", 200, OFFERS)]);
    let bazarr = Bazarr::new(&link, &http);
    let (target, found) = bazarr_search_with(&bazarr, &tears(), "en").expect("found");
    assert_eq!(target, Target::Movie { radarr_id: 7 });
    let rows: Vec<(&str, i64)> = found.iter().map(|c| (c.provider.as_str(), c.score)).collect();
    assert_eq!(rows, [("b", 95), ("c", 80)]);
    assert_eq!(bazarr_search_with(&bazarr, &tears(), "nl").unwrap().1.len(), 1);

    let absent = Canned::new(vec![("/api/system/searches", 200, "[]")]);
    assert_eq!(bazarr_search_with(&Bazarr::new(&link, &absent), &tears(), "en").unwrap_err(), SearchFailure::NotInBazarr);
    let wrong_key = Canned::new(vec![("/api/system/searches", 401, "")]);
    assert_eq!(bazarr_search_with(&Bazarr::new(&link, &wrong_key), &tears(), "en").unwrap_err(), SearchFailure::BadKey);
}

/// The download half: the BEFORE read comes first and its failure sends nothing; the server is asked
/// to re-read the item only after Bazarr took the pick; a wrong key is `denied`.
#[test]
fn the_bazarr_download_reads_before_it_sends_and_refreshes_after() {
    let link = bazarr_link();
    let target = Target::Movie { radarr_id: 7 };
    let pick = candidate("b", "en", 95);
    let http = Canned::new(vec![("/api/providers/movies", 204, "")]);
    let bazarr = Bazarr::new(&link, &http);
    let order = std::cell::RefCell::new(Vec::new());

    let out = bazarr_download_with(&bazarr, target, &pick,
        || { order.borrow_mut().push("before"); Some(vec![1, 2]) },
        || order.borrow_mut().push("refresh"));
    assert_eq!(out, Ok(vec![1, 2]));
    assert_eq!(*order.borrow(), ["before", "refresh"]);
    let sent = http.seen.borrow();
    assert_eq!(sent.len(), 1);
    let form = sent[0].form.clone().unwrap();
    assert!(form.contains("radarrid=7") && form.contains("provider=b") && form.contains("subtitle=handle-b"), "{form}");

    let unread = Canned::new(vec![]);
    let out = bazarr_download_with(&Bazarr::new(&link, &unread), target, &pick, || None, || panic!("no refresh"));
    assert_eq!(out, Err(false));
    assert!(unread.seen.borrow().is_empty(), "nothing is sent when the item could not be read first");

    let refused = Canned::new(vec![("/api/providers/movies", 401, "")]);
    let out = bazarr_download_with(&Bazarr::new(&link, &refused), target, &pick, || Some(vec![]), || panic!("no refresh"));
    assert_eq!(out, Err(true), "a wrong key is a refusal");
}
