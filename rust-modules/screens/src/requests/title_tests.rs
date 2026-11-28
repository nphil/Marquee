//! Commands, focus and words of the Requests title page. No command reaches a server.
use super::super::cards_harness::{title, ReqHost};
use super::*;
use plx_machine::machine::{InputOwner, InstanceId, MachineId, Measure, Stamped};
use plx_plex::plex::seerr::{Detail, Have, InPlex, MediaState, Rules, Season, Still, Video, VideoKind};
use plx_plex::plex::ServerId;
use plx_platform::i18n::{language_on_this_thread_for_test, Preference, SHIPPED};
use plx_ui::fixture::FixtureMeasure;
use plx_ui::theme::{self, size};

const ENTRY: EntryId = EntryId(1);
const WHOLE: Rules = Rules { partial: false, specials: false };
const PARTIAL: Rules = Rules { partial: true, specials: false };
const NOW: Tick = Tick { ms: 0, dt_us: 16_667 };

fn season(number: u32, have: Have) -> Season {
    Season { number, episodes: 8, have, requested: false }
}

fn page(kind: Kind, have: Have, requested: bool, seasons: Vec<Season>, rules: Rules) -> TitlePage {
    let mut t = title(603);
    t.kind = kind;
    t.state = MediaState { have, downloading: false };
    TitlePage {
        detail: Detail { title: t, minutes: 100, genres: vec!["Drama".into()], seasons, requested, plex: None, info: Default::default() },
        plex: None,
        rules,
        art: None,
    }
}

fn movie(have: Have, requested: bool) -> TitlePage { page(Kind::Movie, have, requested, Vec::new(), WHOLE) }

fn show(seasons: Vec<Season>, rules: Rules) -> TitlePage { page(Kind::Tv, Have::Nothing, false, seasons, rules) }

fn step(screen: &mut TitleScreen, ev: ScreenEvent<ReqHost>) -> Vec<Stamped<ReqHost>> {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let cx = Cx::<ReqHost> {
        views: (), tick: NOW, measure: &FixtureMeasure, press: Default::default(), focus: Default::default(), owner: InputOwner::Entry(ENTRY),
    };
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(1)), &mut present);
    Machine::<ReqHost>::step(screen, &ev, &cx, &mut fx);
    out
}

fn commands(out: &[Stamped<ReqHost>]) -> Vec<&RequestsCmd> {
    out.iter().filter_map(|e| match &e.fx {
        Fx::App(AppFx::Requests(RequestsCmd::Progress { .. })) => None,
        Fx::App(AppFx::Requests(c)) => Some(c),
        _ => None,
    }).collect()
}

fn opened(title: TitlePage) -> TitleScreen {
    let mut screen = TitleScreen::new(ENTRY, title.detail.title.kind, title.detail.title.tmdb);
    let out = step(&mut screen, ScreenEvent::Mount);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Detail { tmdb: 603, .. }]));
    answer(&mut screen, title);
    screen
}

fn answer(screen: &mut TitleScreen, title: TitlePage) -> Vec<Stamped<ReqHost>> {
    let (kind, tmdb) = (title.detail.title.kind, title.detail.title.tmdb);
    screen.tx.send(RequestsReply::Detail { kind, tmdb, result: Ok(title) }).unwrap();
    step(screen, ScreenEvent::Tick(NOW))
}

fn press(screen: &mut TitleScreen, elem: u32) -> Vec<Stamped<ReqHost>> { step(screen, ScreenEvent::Activate(elem)) }
fn label(screen: &TitleScreen) -> &str { screen.actions()[0].word() }
fn has(screen: &TitleScreen, elem: u32) -> bool { screen.actions().iter().any(|a| a.act.elem() == Some(elem)) }

#[test]
fn a_movie_that_can_be_asked_for_sends_one_request_and_only_one() {
    let _en = language_on_this_thread_for_test(Preference::En);
    let mut screen = opened(movie(Have::Nothing, false));
    assert_eq!(label(&screen), "Request");
    let out = press(&mut screen, REQUEST);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Request { kind: Kind::Movie, tmdb: 603, seasons, .. }] if seasons.is_empty()));
    assert_eq!(screen.sent, Sent::Sending);
    assert!(commands(&press(&mut screen, REQUEST)).is_empty());
}

#[test]
fn a_title_seerr_already_has_offers_nothing_to_request() {
    let _en = language_on_this_thread_for_test(Preference::En);
    for page in [movie(Have::Pending, false), movie(Have::Processing, false), movie(Have::Nothing, true)] {
        let mut screen = opened(page);
        assert_eq!(label(&screen), "Already requested");
        assert!(commands(&press(&mut screen, REQUEST)).is_empty());
        assert_eq!(screen.sent, Sent::Idle);
    }
    let mut have = opened(movie(Have::All, false));
    assert_eq!(label(&have), "Already available");
    assert!(commands(&press(&mut have, REQUEST)).is_empty());
    let blocked = opened(movie(Have::Blocklisted, false));
    assert!(!has(&blocked, REQUEST));
}

#[test]
fn a_title_in_plex_offers_play_which_opens_it() {
    let mut with_plex = movie(Have::All, false);
    let sid = ServerId::from_raw(0);
    with_plex.detail.plex = Some(InPlex { machine: "m".into(), rating_key: "42".into() });
    with_plex.plex = Some((sid, "42".into()));
    let mut screen = opened(with_plex);
    assert!(has(&screen, PLAY));
    let out = press(&mut screen, PLAY);
    assert!(out.iter().any(|e| {
        matches!(&e.fx, Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { rk, .. }))) if rk == "42")
    }));
    assert!(!has(&opened(movie(Have::Nothing, false)), PLAY));
}

#[test]
fn a_series_offers_its_missing_seasons_all_chosen_and_requests_the_chosen() {
    let seasons = vec![season(1, Have::All), season(2, Have::Nothing), season(3, Have::Nothing), season(4, Have::Pending)];
    let mut screen = opened(show(seasons, PARTIAL));
    assert_eq!(screen.cards_of(Sec::Seasons).unwrap().cards.len(), 4);
    press(&mut screen, SEASON);
    press(&mut screen, SEASON + 3);
    assert_eq!(screen.picked, [2, 3], "owned and pending seasons do not toggle");
    press(&mut screen, SEASON + 1);
    assert_eq!(screen.picked, [3]);
    assert!(matches!(commands(&press(&mut screen, REQUEST)).as_slice(), [RequestsCmd::Request { kind: Kind::Tv, seasons, .. }] if seasons == &[3]));
}

#[test]
fn with_no_season_chosen_request_does_nothing() {
    let mut screen = opened(show(vec![season(1, Have::Nothing)], PARTIAL));
    press(&mut screen, SEASON);
    assert!(screen.picked.is_empty());
    assert!(!screen.actions()[0].enabled);
    assert!(commands(&press(&mut screen, REQUEST)).is_empty());
    assert_eq!(screen.sent, Sent::Idle);
}

#[test]
fn a_server_that_takes_whole_series_only_shows_no_picker() {
    let mut screen = opened(show(vec![season(1, Have::Nothing), season(2, Have::Nothing)], WHOLE));
    assert!(screen.cards_of(Sec::Seasons).is_none());
    assert!(matches!(commands(&press(&mut screen, REQUEST)).as_slice(), [RequestsCmd::Request { seasons, .. }] if seasons == &[1, 2]));
}

#[test]
fn a_series_with_every_season_asked_for_says_so() {
    let _en = language_on_this_thread_for_test(Preference::En);
    let seasons = vec![season(1, Have::All), season(2, Have::Pending)];
    let mut screen = opened(show(seasons, PARTIAL));
    assert_eq!(label(&screen), "Already requested");
    assert!(commands(&press(&mut screen, REQUEST)).is_empty());
}

#[test]
fn a_request_that_went_through_reads_the_title_again() {
    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, REQUEST);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Ok(()) }).unwrap();
    let out = step(&mut screen, ScreenEvent::Tick(NOW));
    assert_eq!(screen.sent, Sent::Done);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Detail { .. }]));
    answer(&mut screen, movie(Have::Pending, false));
    assert!(!has(&screen, REQUEST));
}

#[test]
fn a_request_taken_meanwhile_reads_the_title_again() {
    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, REQUEST);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::AlreadyRequested) }).unwrap();
    assert!(matches!(commands(&step(&mut screen, ScreenEvent::Tick(NOW))).as_slice(), [RequestsCmd::Detail { .. }]));
}

#[test]
fn a_failed_read_says_why_and_a_failed_request_keeps_the_page() {
    let mut screen = TitleScreen::new(ENTRY, Kind::Movie, 603);
    screen.tx.send(RequestsReply::Detail { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::Unreachable) }).unwrap();
    step(&mut screen, ScreenEvent::Tick(NOW));
    assert!(matches!(screen.load, Load::Failed(SeerrError::Unreachable)) && screen.actions().is_empty());

    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, REQUEST);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::Forbidden) }).unwrap();
    step(&mut screen, ScreenEvent::Tick(NOW));
    assert_eq!(screen.sent, Sent::Failed(SeerrError::Forbidden));
    assert!(screen.actions()[0].enabled);
}

fn trailer() -> Video {
    Video { key: "dQw4w9WgXcQ".into(), name: "Official Trailer".into(), kind: VideoKind::Trailer }
}

#[test]
fn a_trailer_opens_only_on_an_explicit_press() {
    let mut screen = TitleScreen::new(ENTRY, Kind::Movie, 603);
    let mut with = movie(Have::Nothing, false);
    with.detail.info.videos.push(trailer());
    with.art = Some(ServerId::from_raw(0));
    assert!(matches!(commands(&step(&mut screen, ScreenEvent::Mount)).as_slice(), [RequestsCmd::Detail { .. }]));
    assert!(commands(&answer(&mut screen, with)).is_empty());
    assert!(has(&screen, TRAILER));
    assert!(screen.cards_of(Sec::Videos).is_some());
    assert!(matches!(commands(&press(&mut screen, TRAILER)).as_slice(), [RequestsCmd::Trailer { video, .. }] if video == "dQw4w9WgXcQ"));
    assert!(commands(&press(&mut screen, TRAILER)).is_empty());
    screen.tx.send(RequestsReply::Trailer { opened: false }).unwrap();
    step(&mut screen, ScreenEvent::Tick(NOW));
    assert!(screen.trailer_failed);

    let mut without = movie(Have::Nothing, false);
    without.art = Some(ServerId::from_raw(0));
    let mut screen = opened(without);
    assert!(!has(&screen, TRAILER));
    assert!(screen.cards_of(Sec::Videos).is_none());
    assert!(commands(&press(&mut screen, TRAILER)).is_empty());
}

#[test]
fn a_series_shows_a_still_shelf_only_after_real_art_arrives() {
    let mut page = show(vec![season(0, Have::Nothing), season(1, Have::Nothing), season(4, Have::Nothing)], PARTIAL);
    page.art = Some(ServerId::from_raw(0));
    let mut screen = TitleScreen::new(ENTRY, Kind::Tv, 603);
    let out = answer(&mut screen, page);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Stills { tmdb: 603, seasons, .. }] if seasons == &[1, 4]));
    assert!(screen.cards_of(Sec::Episodes).is_none());
    screen.tx.send(RequestsReply::Stills { tmdb: 603, result: Ok(vec![Still {
        season: 4, episode: 1, name: "Premiere".into(), image: "https://image.tmdb.org/t/p/w780/episode.jpg".into(),
    }]) }).unwrap();
    step(&mut screen, ScreenEvent::Tick(NOW));
    assert_eq!(screen.cards_of(Sec::Episodes).unwrap().cards.len(), 1);
}

#[test]
fn translated_title_controls_and_feedback_fit() {
    let _guard = plx_base::testlock::serial();
    let mut in_plex = movie(Have::All, false);
    in_plex.plex = Some((ServerId::from_raw(0), "1".into()));
    let mut requestable = movie(Have::Nothing, false);
    requestable.detail.info.videos.push(trailer());
    let pages = [movie(Have::Pending, false), movie(Have::All, false), movie(Have::Blocklisted, false), in_plex,
        requestable, show(vec![season(1, Have::Nothing)], PARTIAL), show(Vec::new(), PARTIAL)];
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        for page in &pages {
            let actions = facts::actions(page, &[1], false);
            for (a, r) in paint::pills(&actions, 0.0, &FixtureMeasure) {
                assert!(r.x + r.w <= SCR_W - MARGIN_X, "{}: {} outside the safe area", language.tag(), a.word());
            }
        }
        for word in [msg::requests_trailer_failed(), msg::requests_sending(), msg::requests_sent()] {
            let w = FixtureMeasure.width_str(word, size::CAPTION, false);
            assert!(w <= theme::hero::TEXT_W, "{}: {word} too wide ({w})", language.tag());
        }
    }
}
