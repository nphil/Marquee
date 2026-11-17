//! The Requests page: what lands on its rows, the search, Back, the strip and opening a title.
use super::cards_harness::{title, Harness, ReqHost};
use super::*;
use plx_machine::machine::{Edge, InputEvent, Source, Stamped, Tick};
use plx_plex::plex::seerr::{Have, MediaState, Page, SeerrError};

fn key(key: Key) -> ScreenEvent<ReqHost> {
    ScreenEvent::Input(InputEvent {
        at: Tick::default(),
        source: Source::Sdl,
        kind: InputKind::Key { key, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
    })
}

fn with_fx(h: &mut Harness, f: impl FnOnce(&mut RequestsScreen, &mut Effects<'_, ReqHost>)) -> Vec<Stamped<ReqHost>> {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let mut fx = Effects::<ReqHost>::new(&mut out, MachineId::Instance(cards_harness::INSTANCE), &mut present);
    f(&mut h.screen, &mut fx);
    out
}

fn app(out: &[Stamped<ReqHost>]) -> Vec<&AppFx> {
    out.iter().filter_map(|e| if let Fx::App(a) = &e.fx { Some(a) } else { None }).collect()
}

fn shelf(shelf: Discover, page: u32, pages: u32, ids: &[u32]) -> RequestsReply {
    RequestsReply::Shelf {
        shelf,
        page,
        result: Ok(Page { items: ids.iter().map(|id| title(*id)).collect(), page, pages }),
    }
}

fn names(h: &Harness) -> Vec<(Lane, Vec<u32>)> {
    h.screen.shown().map(|r| (r.lane, r.cards.iter().map(|c| c.tmdb).collect())).collect()
}

#[test]
fn opening_the_page_asks_the_driver_once_and_seats_the_field() {
    let mut h = Harness::new(vec![]);
    let (out, _) = h.step_fx(ScreenEvent::Mount);
    let asked: Vec<_> = app(&out).into_iter().filter(|a| matches!(a, AppFx::Requests(RequestsCmd::Open(_)))).collect();
    assert_eq!(asked.len(), 1);
    assert!(out.iter().any(|e| matches!(&e.fx, Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(_))))), "focus lands on the field");
}

#[test]
fn shelves_land_in_order_and_only_rows_with_titles_show() {
    let mut h = Harness::new(vec![]);
    h.screen.receive(shelf(Discover::Shows, 1, 1, &[3]));
    h.screen.receive(shelf(Discover::Trending, 1, 1, &[1, 2]));
    h.screen.receive(shelf(Discover::Upcoming, 1, 1, &[]));
    assert_eq!(names(&h), [(Lane::Trending, vec![1, 2]), (Lane::Shows, vec![3])]);
}

#[test]
fn a_later_page_follows_the_row_and_repeats_nothing() {
    let mut h = Harness::new(vec![]);
    h.screen.receive(shelf(Discover::Movies, 1, 3, &[1, 2]));
    h.screen.receive(shelf(Discover::Movies, 2, 3, &[2, 3]));
    assert_eq!(names(&h), [(Lane::Movies, vec![1, 2, 3])]);
    let row = h.screen.rows.iter().find(|r| r.lane == Lane::Movies).unwrap();
    assert!(row.more && row.pages == 2);
    // a first page again replaces the row
    h.screen.receive(shelf(Discover::Movies, 1, 1, &[9]));
    assert_eq!(names(&h), [(Lane::Movies, vec![9])]);
}

#[test]
fn a_title_keeps_its_element_when_its_row_is_read_again() {
    let mut h = Harness::new(vec![]);
    h.screen.receive(shelf(Discover::Movies, 1, 1, &[5, 6]));
    let before: Vec<u32> = h.screen.rows[2].cards.iter().map(|c| c.elem).collect();
    h.screen.receive(shelf(Discover::Movies, 1, 1, &[6, 5]));
    let after: Vec<u32> = h.screen.rows[2].cards.iter().map(|c| c.elem).collect();
    assert_eq!(before, [after[1], after[0]]);
}

#[test]
fn a_poster_shows_the_state_seerr_gives_it_or_the_one_of_a_request() {
    let mut h = Harness::new(vec![]);
    let mut asked = title(1);
    asked.state = MediaState { have: Have::Pending, downloading: false };
    let plain = title(2);
    h.screen.receive(RequestsReply::Shelf {
        shelf: Discover::Trending,
        page: 1,
        result: Ok(Page { items: vec![asked, plain], page: 1, pages: 1 }),
    });
    let cards = &h.screen.rows[1].cards;
    assert_eq!(cards[0].badge, Some(Badge::Requested));
    assert_eq!(cards[1].badge, None);
    h.screen.receive(RequestsReply::Mine {
        skip: 0,
        result: Ok((vec![crate::registry::AskedTitle { title: title(7), badge: Some(Badge::Downloading) }], false)),
    });
    assert_eq!(h.screen.rows[0].cards[0].badge, Some(Badge::Downloading));
}

#[test]
fn a_row_that_failed_is_settled_and_a_failed_open_stops_every_row() {
    let mut h = Harness::new(vec![]);
    h.screen.receive(RequestsReply::Shelf { shelf: Discover::Shows, page: 1, result: Err(SeerrError::Unreachable) });
    assert!(h.screen.rows[3].loaded && h.screen.shown().count() == 0);
    h.screen.receive(RequestsReply::Failed(SeerrError::SignInRefused));
    assert_eq!(h.screen.phase, Phase::Failed);
    assert!(h.screen.settled());
}

#[test]
fn committing_words_searches_and_empty_words_end_the_search() {
    let mut h = Harness::new(vec![1, 2]);
    h.screen.query = TextBuffer::new("  dune ".into(), 7);
    let out = with_fx(&mut h, |s, fx| s.commit(fx));
    let sent: Vec<_> = app(&out);
    assert!(matches!(sent.as_slice(), [AppFx::Requests(RequestsCmd::Search { query, page: 1, .. })] if query == "dune"));
    assert!(h.screen.searching);
    h.screen.receive(RequestsReply::Search { query: "dune".into(), page: 1, result: Ok(Page { items: vec![title(44)], page: 1, pages: 1 }) });
    assert_eq!(names(&h), [(Lane::Results, vec![44])], "the results stand alone");
    // an answer for words that are no longer the search is dropped
    h.screen.receive(RequestsReply::Search { query: "old".into(), page: 1, result: Ok(Page { items: vec![title(45)], page: 1, pages: 1 }) });
    assert_eq!(names(&h), [(Lane::Results, vec![44])]);
    h.screen.query.clear();
    with_fx(&mut h, |s, fx| s.commit(fx));
    assert!(h.screen.results.is_none());
    assert_eq!(names(&h), [(Lane::Trending, vec![1, 2])], "the browse rows are back");
}

#[test]
fn back_leaves_a_shown_search_first_and_the_page_second() {
    let mut h = Harness::new(vec![1]);
    h.screen.query = TextBuffer::new("dune".into(), 4);
    with_fx(&mut h, |s, fx| s.commit(fx));
    assert!(h.screen.results.is_some());
    let (out, _) = h.step_fx(key(Key::Back));
    assert!(h.screen.results.is_none() && h.screen.query.text().is_empty());
    assert!(!app(&out).iter().any(|a| matches!(a, AppFx::Search(SearchReq::Back))), "the page stays");
    let (out, _) = h.step_fx(key(Key::Back));
    assert!(app(&out).iter().any(|a| matches!(a, AppFx::Search(SearchReq::Back))));
}

#[test]
fn the_strip_goes_to_its_tab_and_the_requests_mark_stays_put() {
    use plx_ui::widgets::{TAB_KEY_HOME, TAB_KEY_REQUESTS, TAB_KEY_SEARCH};
    let mut h = Harness::new(vec![1]);
    let (out, _) = h.step_fx(ScreenEvent::Activate(TAB_KEY_HOME));
    assert!(matches!(app(&out).as_slice(), [AppFx::Search(SearchReq::Tab(HomeTab::Home))]));
    let (out, _) = h.step_fx(ScreenEvent::Activate(TAB_KEY_SEARCH));
    assert!(matches!(app(&out).as_slice(), [AppFx::Search(SearchReq::Tab(HomeTab::Search))]));
    let (out, _) = h.step_fx(ScreenEvent::Activate(TAB_KEY_REQUESTS));
    assert!(app(&out).is_empty());
}

#[test]
fn a_poster_opens_its_title_page() {
    let mut h = Harness::new(vec![11, 12]);
    let elem = h.screen.rows[1].cards[1].elem;
    let (out, _) = h.step_fx(ScreenEvent::Activate(elem));
    assert!(matches!(
        app(&out).as_slice(),
        [AppFx::Content(ContentReq::Push(ContentArg::RequestTitle { kind: Kind::Movie, tmdb: 12 }))]
    ));
}

#[test]
fn the_end_of_a_row_asks_for_its_next_page_once() {
    let mut h = Harness::new(vec![]);
    h.screen.receive(shelf(Discover::Movies, 1, 3, &[1, 2]));
    let out = with_fx(&mut h, |s, fx| {
        s.want(Lane::Movies, fx);
        s.want(Lane::Movies, fx);
    });
    let sent: Vec<_> = app(&out);
    assert!(matches!(sent.as_slice(), [AppFx::Requests(RequestsCmd::Shelf { shelf: Discover::Movies, page: 2, .. })]));
}

#[test]
fn the_page_state_moves_with_what_lands() {
    let mut h = Harness::new(vec![]);
    let before = LogicalState::hash(&h.screen);
    h.screen.receive(shelf(Discover::Movies, 1, 1, &[1]));
    assert_ne!(LogicalState::hash(&h.screen), before);
    let mut probe = String::new();
    LogicalState::probe(&h.screen, &mut probe);
    assert!(probe.starts_with("requests "));
}
