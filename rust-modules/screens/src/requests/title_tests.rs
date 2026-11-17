//! A title's page: what it offers for each state Seerr can report, what a press sends, and the fit
//! of every string in every shipped language. No test sends a request anywhere: the page only
//! queues a command for the driver.
use super::cards_harness::{title, ReqHost};
use super::*;
use plx_machine::machine::{InputOwner, Stamped, Tick};
use plx_plex::plex::seerr::{Detail, InPlex, MediaState, Rules, Season};
use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
use plx_ui::fixture::FixtureMeasure;

const ENTRY: EntryId = EntryId(1);
const WHOLE: Rules = Rules { partial: false, specials: false };
const PARTIAL: Rules = Rules { partial: true, specials: false };

fn season(number: u32, have: Have) -> Season {
    Season { number, episodes: 8, have, requested: false }
}

fn page(kind: Kind, have: Have, requested: bool, seasons: Vec<Season>, rules: Rules) -> TitlePage {
    let mut t = title(603);
    t.kind = kind;
    t.state = MediaState { have, downloading: false };
    TitlePage {
        detail: Detail { title: t, minutes: 100, genres: vec!["Drama".into()], seasons, requested, plex: None },
        plex: None,
        rules,
    }
}

fn movie(have: Have, requested: bool) -> TitlePage {
    page(Kind::Movie, have, requested, Vec::new(), WHOLE)
}

fn show(seasons: Vec<Season>, rules: Rules) -> TitlePage {
    page(Kind::Tv, Have::Nothing, false, seasons, rules)
}

fn step(screen: &mut TitleScreen, ev: ScreenEvent<ReqHost>) -> Vec<Stamped<ReqHost>> {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let cx = Cx::<ReqHost> {
        views: (),
        tick: Tick { ms: 0, dt_us: 16_667 },
        measure: &FixtureMeasure,
        press: Default::default(),
        focus: Default::default(),
        owner: InputOwner::Entry(ENTRY),
    };
    let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(1)), &mut present);
    Machine::<ReqHost>::step(screen, &ev, &cx, &mut fx);
    out
}

fn commands(out: &[Stamped<ReqHost>]) -> Vec<&RequestsCmd> {
    out.iter().filter_map(|e| if let Fx::App(AppFx::Requests(c)) = &e.fx { Some(c) } else { None }).collect()
}

/// A page that has read `title` from the driver.
fn opened(title: TitlePage) -> TitleScreen {
    let mut screen = TitleScreen::new(ENTRY, title.detail.title.kind, title.detail.title.tmdb);
    let out = step(&mut screen, ScreenEvent::Mount);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Detail { tmdb: 603, .. }]));
    answer(&mut screen, title);
    screen
}

fn answer(screen: &mut TitleScreen, title: TitlePage) {
    let (kind, tmdb) = (title.detail.title.kind, title.detail.title.tmdb);
    screen.tx.send(RequestsReply::Detail { kind, tmdb, result: Ok(title) }).unwrap();
    step(screen, ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
}

fn press(screen: &mut TitleScreen, id: TId) -> Vec<Stamped<ReqHost>> {
    step(screen, ScreenEvent::Activate(id.key().0))
}

fn labels(screen: &TitleScreen) -> Vec<String> {
    screen.form.table.sections.iter().flat_map(|s| s.rows.iter()).map(|r| r.label.clone()).collect()
}

fn has(screen: &TitleScreen, id: TId) -> bool {
    screen.form.index_of_key(id.key()).is_some()
}

#[test]
fn a_movie_that_can_be_asked_for_sends_one_request_and_only_one() {
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut screen = opened(movie(Have::Nothing, false));
    assert_eq!(labels(&screen)[0], "Request");
    let out = press(&mut screen, TId::Request);
    assert!(matches!(
        commands(&out).as_slice(),
        [RequestsCmd::Request { kind: Kind::Movie, tmdb: 603, seasons, .. }] if seasons.is_empty()
    ));
    assert_eq!(screen.sent, Sent::Sending);
    assert!(commands(&press(&mut screen, TId::Request)).is_empty(), "a second press while sending sends nothing");
}

#[test]
fn a_title_seerr_already_has_offers_nothing_to_press() {
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    // asked for by the viewer, by someone else, or by Pulsarr (a request without a state yet)
    for page in [movie(Have::Pending, false), movie(Have::Processing, false), movie(Have::Nothing, true)] {
        let mut screen = opened(page);
        assert_eq!(labels(&screen)[0], "Already requested");
        assert!(commands(&press(&mut screen, TId::Request)).is_empty());
        assert_eq!(screen.sent, Sent::Idle);
    }
    let mut have = opened(movie(Have::All, false));
    assert_eq!(labels(&have)[0], "Already available");
    assert!(commands(&press(&mut have, TId::Request)).is_empty());
    let blocked = opened(movie(Have::Blocklisted, false));
    assert!(!has(&blocked, TId::Season(1)) && labels(&blocked)[0] != "Request");
}

#[test]
fn a_title_in_plex_offers_play_which_opens_it() {
    let mut with_plex = movie(Have::All, false);
    let sid = plx_plex::plex::ServerId::from_raw(0);
    with_plex.detail.plex = Some(InPlex { machine: "m".into(), rating_key: "42".into() });
    with_plex.plex = Some((sid, "42".into()));
    let mut screen = opened(with_plex);
    assert!(has(&screen, TId::Play));
    let out = press(&mut screen, TId::Play);
    let pushed = out.iter().any(|e| {
        matches!(&e.fx, Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { rk, .. }))) if rk == "42")
    });
    assert!(pushed);
    assert!(!has(&opened(movie(Have::Nothing, false)), TId::Play));
}

#[test]
fn a_series_offers_its_missing_seasons_all_chosen_and_asks_for_the_chosen() {
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let seasons = vec![season(1, Have::All), season(2, Have::Nothing), season(3, Have::Nothing), season(4, Have::Pending)];
    let mut screen = opened(show(seasons, PARTIAL));
    assert!(has(&screen, TId::Season(2)) && has(&screen, TId::Season(3)));
    assert_eq!(screen.picked, [2, 3]);
    // a season that is there or asked for is listed but cannot be picked
    assert_eq!(screen.form.activate(screen.form.index_of_key(TId::Season(1).key()).unwrap()), None);
    assert_eq!(screen.form.activate(screen.form.index_of_key(TId::Season(4).key()).unwrap()), None);
    press(&mut screen, TId::Season(2));
    assert_eq!(screen.picked, [3]);
    let out = press(&mut screen, TId::Request);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Request { kind: Kind::Tv, seasons, .. }] if seasons == &[3]));
}

#[test]
fn with_no_season_chosen_request_does_nothing() {
    let mut screen = opened(show(vec![season(1, Have::Nothing)], PARTIAL));
    press(&mut screen, TId::Season(1));
    assert!(screen.picked.is_empty());
    assert!(commands(&press(&mut screen, TId::Request)).is_empty());
    assert_eq!(screen.sent, Sent::Idle);
}

#[test]
fn a_server_that_takes_whole_series_only_shows_no_picker() {
    let mut screen = opened(show(vec![season(1, Have::Nothing), season(2, Have::Nothing)], WHOLE));
    assert!(!has(&screen, TId::Season(1)));
    let out = press(&mut screen, TId::Request);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Request { seasons, .. }] if seasons == &[1, 2]));
}

#[test]
fn a_series_with_every_season_asked_for_says_so() {
    let _en = language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let seasons = vec![season(1, Have::All), season(2, Have::Pending)];
    let mut screen = opened(show(seasons, PARTIAL));
    assert_eq!(labels(&screen)[0], "Already requested");
    assert!(commands(&press(&mut screen, TId::Request)).is_empty());
}

#[test]
fn a_request_that_went_through_reads_the_title_again() {
    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, TId::Request);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Ok(()) }).unwrap();
    let out = step(&mut screen, ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
    assert_eq!(screen.sent, Sent::Done);
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Detail { .. }]));
    // Seerr now says it is asked for: no Request
    answer(&mut screen, movie(Have::Pending, false));
    assert!(commands(&press(&mut screen, TId::Request)).is_empty());
}

#[test]
fn a_request_taken_meanwhile_is_not_an_error_and_reads_the_title_again() {
    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, TId::Request);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::AlreadyRequested) }).unwrap();
    let out = step(&mut screen, ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
    assert!(matches!(commands(&out).as_slice(), [RequestsCmd::Detail { .. }]));
}

#[test]
fn a_failed_read_says_why_and_a_failed_request_keeps_the_page() {
    let mut screen = TitleScreen::new(ENTRY, Kind::Movie, 603);
    screen.tx.send(RequestsReply::Detail { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::Unreachable) }).unwrap();
    step(&mut screen, ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
    assert!(matches!(screen.load, Load::Failed(SeerrError::Unreachable)) && screen.form.table.n_rows() == 0);

    let mut screen = opened(movie(Have::Nothing, false));
    press(&mut screen, TId::Request);
    screen.tx.send(RequestsReply::Requested { kind: Kind::Movie, tmdb: 603, result: Err(SeerrError::Forbidden) }).unwrap();
    step(&mut screen, ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
    assert_eq!(screen.sent, Sent::Failed(SeerrError::Forbidden));
    assert!(has(&screen, TId::Request), "the viewer can try again");
}

#[test]
fn the_overview_in_the_header_is_cut_at_a_word() {
    assert_eq!(blurb("  short  "), "short");
    let long = "word ".repeat(100);
    let cut = blurb(&long);
    assert!(cut.chars().count() <= 241 && cut.ends_with('…') && !cut.contains("wor…"), "{cut}");
}

/// **Every string fits, in every shipped language**, for every state a title's page can be in.
#[test]
fn every_title_string_fits_in_every_language() {
    let _guard = plx_base::testlock::serial();
    let layout = plx_ui::route_screen::RouteLayout::screen();
    let all = vec![season(0, Have::Nothing), season(1, Have::All), season(2, Have::Partly), season(3, Have::Pending), season(4, Have::Nothing)];
    let mut in_plex = movie(Have::All, false);
    in_plex.plex = Some((plx_plex::plex::ServerId::from_raw(0), "1".into()));
    let pages = [
        movie(Have::Nothing, false),
        movie(Have::Pending, false),
        movie(Have::All, false),
        movie(Have::Blocklisted, false),
        in_plex,
        show(all.clone(), PARTIAL),
        show(all, Rules { partial: true, specials: true }),
        show(Vec::new(), PARTIAL),
    ];
    let sents = [Sent::Idle, Sent::Sending, Sent::Done, Sent::Failed(SeerrError::Unreachable), Sent::Failed(SeerrError::Forbidden), Sent::Failed(SeerrError::RateLimited)];
    let mut out = Vec::new();
    for language in SHIPPED {
        let _lang = language_on_this_thread_for_test(language);
        for (i, page) in pages.iter().enumerate() {
            for sent in sents {
                for picked in [&[][..], &[1, 2, 4][..]] {
                    let mut form = FormTable::<TId, TId, ()>::new(crate::registry::BAND);
                    form.table.compact = false;
                    form.set(title_form(page, picked, sent), None);
                    out.extend(form.table.app_fit_failures(layout.sectioned_table().w, &format!("{} title {i} {sent:?} {picked:?}", language.tag())));
                }
            }
        }
    }
    plx_ui::table::assert_no_fit_failures(&out);
}
