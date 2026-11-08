//! A confirmed deletion ([`ViewEdit::Deleted`]) leaves the Library listing and the section shelves
//! at once: the deleted rows close up, the show or season a deleted episode counted in gives the
//! episode up, and a read that was out before the deletion cannot bring a row back. The listing's
//! reads are answered through the real request site against a registered but unreachable server,
//! as in `browse_view_ledger_tests.rs`.

use super::test_support::*;
use super::*;
use crate::stores::paging::{Ledger, LedgerKey, RowMode, RowState};
use crate::viewstate::{Scope, ViewAt, ViewEdit};
use plx_machine::job::Port;

const SID: ServerId = ServerId::from_raw(1);
const OTHER: ServerId = ServerId::from_raw(2);

fn row(sid: ServerId, rk: &str) -> PmsMovie {
    PmsMovie { sid, rk: rk.into(), ..Default::default() }
}

fn show(sid: ServerId, rk: &str, leaves: u32, viewed: u32) -> PmsMovie {
    PmsMovie { kind: 1, leaf_count: Some(leaves), viewed_leaf_count: Some(viewed), ..row(sid, rk) }
}

fn season(sid: ServerId, rk: &str, of: &str, index: i32, leaves: u32, viewed: u32) -> PmsMovie {
    PmsMovie {
        kind: 2,
        show_rk: of.into(),
        season_index: index,
        leaf_count: Some(leaves),
        viewed_leaf_count: Some(viewed),
        ..row(sid, rk)
    }
}

fn episode(sid: ServerId, rk: &str, of: &str, index: i32) -> PmsMovie {
    PmsMovie { kind: 3, show_rk: of.into(), season_index: index, ..row(sid, rk) }
}

fn episode_at(sid: ServerId, rk: &str, of: &str, index: i64) -> ViewAt {
    ViewAt { sid, rk: rk.into(), scope: Scope::Episode { show_rk: of.into(), index } }
}

fn rows(sid: ServerId, rks: &[&str]) -> SecItems {
    SecItems::from_vec(rks.iter().map(|rk| Some(row(sid, rk))).collect())
}

/// `n` fully loaded plain rows keyed "0", "1", …
fn numbered(n: usize) -> SecItems {
    SecItems::from_vec((0..n).map(|i| Some(row(SID, &i.to_string()))).collect())
}

fn names(range: impl Iterator<Item = usize>) -> Vec<String> {
    range.map(|i| i.to_string()).collect()
}

fn some(rks: &[&str]) -> Vec<Option<String>> {
    rks.iter().map(|rk| Some(rk.to_string())).collect()
}

/// The key at every index, `None` for a hole.
fn rks_of(items: &SecItems) -> Vec<Option<String>> {
    (0..items.len()).map(|i| items.get(i).map(|m| m.rk.clone())).collect()
}

/// The loaded rows' identities in order, holes skipped.
fn held(items: &SecItems) -> Vec<(ServerId, &str)> {
    (0..items.len()).filter_map(|i| items.get(i)).map(|m| (m.sid, m.rk.as_str())).collect()
}

fn find<'a>(items: &'a SecItems, rk: &str) -> &'a PmsMovie {
    (0..items.len()).filter_map(|i| items.get(i)).find(|m| m.rk == rk).expect("the row is still listed")
}

/// `(leaf_count, viewed_leaf_count, watched, unwatched)` of a show or season row.
fn tally(m: &PmsMovie) -> (Option<u32>, Option<u32>, bool, bool) {
    (m.leaf_count, m.viewed_leaf_count, m.watched, m.unwatched)
}

/// A TV listing: show S with seasons 1 and 2 and their episodes, another show T, the same keys S
/// and E1 on ANOTHER server, and a movie.
fn tv_listing() -> SecItems {
    SecItems::from_vec(vec![
        Some(show(SID, "S", 3, 0)),
        Some(season(SID, "S1", "S", 1, 2, 0)),
        Some(episode(SID, "E1", "S", 1)),
        Some(episode(SID, "E2", "S", 1)),
        Some(season(SID, "S2", "S", 2, 1, 0)),
        Some(episode(SID, "E3", "S", 2)),
        Some(show(SID, "T", 4, 0)),
        Some(episode(SID, "E7", "T", 1)),
        Some(show(OTHER, "S", 3, 0)),
        Some(episode(OTHER, "E1", "S", 1)),
        Some(row(SID, "M")),
    ])
}

fn listed(rks: &[&str]) -> SecState {
    SecState { fetch: SecFetch::Ready, total: rks.len() as i64, items: rows(SID, rks), ..SecState::default() }
}

/// Publish the deletion to the store the way `Stores::media_deleted` does, minus the ledger.
fn publish(browse: &mut TestBrowse, at: ViewAt, watched: bool) -> bool {
    let cmd = crate::stores::browse::BrowseCmd::ApplyView { at, edit: ViewEdit::Deleted { watched } };
    browse.state.run_owned(&browse.adapter, cmd, &ClientKeys::default(), &plx_plex::plex::session::peek(), &mut Vec::new())
}

/// Section 0 of a registered source, listed and fully loaded with `n` rows keyed "0", "1", …
fn list(browse: &mut TestBrowse, sid: ServerId, n: usize) {
    let state = browse.state.state_mut(0).unwrap();
    state.fetch = SecFetch::Ready;
    state.total = n as i64;
    state.items = SecItems::from_vec((0..n).map(|i| Some(row(sid, &i.to_string()))).collect());
}

/// A page read's answer, as the server described the listing, taken in by the store's pump.
fn land_page(browse: &mut TestBrowse, client: &'static plx_plex::plex::Client, sid: ServerId, gen: u32, start: usize,
    rks: Vec<String>, total: i64) {
    let page = PageResult {
        client,
        token_gen: client.token_gen(),
        gen,
        sec: 0,
        start,
        items: rks.iter().map(|rk| row(sid, rk)).collect(),
        total,
        sorts: None, restored: None, genres: None, genre: None,
        resolved: Default::default(),
    };
    browse.adapter.page.post(page, |_| true);
    let _ = browse.state.pump_owned(&browse.adapter);
}

fn shelf(id: &str, sid: ServerId, rks: &[&str]) -> section_hubs::Shelf {
    section_hubs::Shelf {
        id: id.into(),
        key: format!("/hubs/{id}"),
        title: id.into(),
        total: rks.len(),
        items: rks.iter().map(|rk| row(sid, rk)).collect(),
        positions: (0..rks.len()).collect(),
        end: rks.len(),
        ..Default::default()
    }
}

fn cards(shelf: &section_hubs::Shelf) -> Vec<&str> {
    shelf.items.iter().map(|m| m.rk.as_str()).collect()
}

/// A store whose only section has these shelves published.
fn on_screen(shelves: Vec<section_hubs::Shelf>) -> TestBrowse {
    let mut browse = TestBrowse::default();
    browse.state.states = vec![SecState::default()];
    browse.state.states[0].hubs.committed = Arc::new(shelves);
    browse
}

/// Rows after a removed one close up across the page boundary: the row that opened the next page
/// now ends the previous one, and the listing is one row shorter.
#[test]
fn a_removed_row_is_closed_over_across_the_page_boundary() {
    let mut items = numbered(2 * PAGE + 10);
    let before = items.clone();
    assert_eq!(items.deleted(&ViewAt::leaf(SID, "5"), false), (1, false));
    assert_eq!(items.len(), 2 * PAGE + 9);
    assert_eq!(items.pages.len(), 3);
    let at = |i: usize| items.get(i).map(|m| m.rk.clone());
    assert_eq!(at(4), Some("4".to_string()));
    assert_eq!(at(5), Some("6".to_string()), "the row after the deleted one takes its place");
    assert_eq!(at(PAGE - 1), Some(PAGE.to_string()), "the first row of the next page ends the previous page");
    assert_eq!(at(2 * PAGE + 8), Some((2 * PAGE + 9).to_string()));
    assert!((0..3).all(|p| !items.page_missing(p)), "every page was loaded, so none has a hole");
    assert_eq!(before.len(), 2 * PAGE + 10, "a reader's clone keeps the rows it had");
    assert_eq!(before.get(5).map(|m| m.rk.as_str()), Some("5"));
}

/// A page that was never loaded cannot say which row closes up into the slot before it: that slot
/// is a hole, as one never fetched, and the fetch scan asks for it again. A short last page that
/// loses its only row is dropped with it.
#[test]
fn an_unloaded_page_leaves_holes_where_rows_close_up() {
    let mut items = SecItems::from_vec(
        (0..3 * PAGE).map(|i| (!(PAGE..2 * PAGE).contains(&i)).then(|| row(SID, &i.to_string()))).collect());
    assert_eq!(items.deleted(&ViewAt::leaf(SID, "5"), false), (1, false));
    assert_eq!(items.len(), 3 * PAGE - 1);
    assert!(items.get(PAGE - 1).is_none(), "the slot the unloaded page would have filled");
    assert!(items.page_missing(0) && items.page_missing(1), "both are fetched again");
    assert_eq!(items.get(2 * PAGE - 1).map(|m| m.rk.clone()), Some((2 * PAGE).to_string()));
    assert_eq!(items.get(2 * PAGE).map(|m| m.rk.clone()), Some((2 * PAGE + 1).to_string()));
    assert!(!items.page_missing(2), "the tail page, one row short, is complete");

    let mut tail = numbered(PAGE + 1);
    assert_eq!(tail.deleted(&ViewAt::leaf(SID, &PAGE.to_string()), false), (1, false));
    assert_eq!((tail.len(), tail.pages.len()), (PAGE, 1));
    assert!(tail.get(PAGE).is_none() && !tail.page_missing(0));
}

/// A deletion that names no loaded row copies no page, whatever its scope.
#[test]
fn a_deletion_that_reaches_no_loaded_row_copies_nothing() {
    let mut items = numbered(2 * PAGE);
    let before = items.clone();
    for at in [ViewAt::leaf(SID, "999"), ViewAt::leaf(OTHER, "5"), episode_at(SID, "E9", "X", 1)] {
        assert_eq!(items.deleted(&at, true), (0, false), "{at:?}");
        assert!(Arc::ptr_eq(&items.pages, &before.pages), "nothing was copied for {at:?}");
    }
}

/// Only the pages from the first removal on are rebuilt: the pages before it stay shared with a
/// reader's clone.
#[test]
fn pages_before_the_first_removal_keep_their_handle() {
    let mut items = numbered(2 * PAGE);
    let before = items.clone();
    assert_eq!(items.deleted(&ViewAt::leaf(SID, &(PAGE + 5).to_string()), false), (1, false));
    assert_eq!(items.len(), 2 * PAGE - 1);
    let page = |s: &SecItems, p: usize| s.pages[p].clone().unwrap();
    assert!(Arc::ptr_eq(&page(&items, 0), &page(&before, 0)), "page 0 is untouched");
    assert!(!Arc::ptr_eq(&page(&items, 1), &page(&before, 1)), "page 1 lost a row");
}

/// A deleted show takes its seasons and episodes with it, a deleted season its episodes; the same
/// keys on another server and the neighbouring shows stay.
#[test]
fn a_deleted_show_or_season_takes_its_children_with_it() {
    let mut items = tv_listing();
    let whole_show = ViewAt { sid: SID, rk: "S".into(), scope: Scope::Show };
    assert_eq!(items.deleted(&whole_show, false), (6, false));
    assert_eq!(held(&items), [(SID, "T"), (SID, "E7"), (OTHER, "S"), (OTHER, "E1"), (SID, "M")]);
    assert_eq!(items.len(), 5);

    let mut items = tv_listing();
    let one_season = ViewAt { sid: SID, rk: "S1".into(), scope: Scope::Season { show_rk: "S".into(), index: 1 } };
    assert_eq!(items.deleted(&one_season, false), (3, false));
    assert_eq!(held(&items), [
        (SID, "S"), (SID, "S2"), (SID, "E3"), (SID, "T"), (SID, "E7"), (OTHER, "S"), (OTHER, "E1"), (SID, "M"),
    ]);
}

/// A deleted episode lowers the episode counts of its show and of its season (the viewed count
/// too when it was watched) and recomputes their flags; the show's other seasons stand.
#[test]
fn a_deleted_episode_is_given_up_by_its_show_and_its_season() {
    let mut items = SecItems::from_vec(vec![
        Some(show(SID, "S", 10, 4)),
        Some(season(SID, "S1", "S", 1, 5, 2)),
        Some(season(SID, "S2", "S", 2, 5, 2)),
        Some(episode(SID, "E1", "S", 1)),
        Some(episode(SID, "E2", "S", 1)),
        Some(show(SID, "T", 7, 7)),
    ]);
    assert_eq!(items.deleted(&episode_at(SID, "E1", "S", 1), true), (1, true));
    assert_eq!(items.len(), 5);
    assert_eq!(tally(find(&items, "S")), (Some(9), Some(3), false, false));
    assert_eq!(tally(find(&items, "S1")), (Some(4), Some(1), false, false));
    assert_eq!(tally(find(&items, "S2")), (Some(5), Some(2), false, false), "another season stands");
    assert_eq!(tally(find(&items, "T")), (Some(7), Some(7), false, false), "another show stands");
    assert!(held(&items).iter().all(|&(_, rk)| rk != "E1") && held(&items).contains(&(SID, "E2")));
}

/// The flags of a season follow its recomputed counts: an unwatched episode that was the only one
/// left to watch makes it watched, a watched one that was the only one watched makes it unwatched.
#[test]
fn the_flags_of_a_season_follow_its_recomputed_counts() {
    let mut items =
        SecItems::from_vec(vec![Some(season(SID, "S1", "S", 1, 4, 3)), Some(season(SID, "S2", "S", 2, 2, 1))]);
    assert_eq!(items.deleted(&episode_at(SID, "E1", "S", 1), false), (0, true));
    assert_eq!(tally(find(&items, "S1")), (Some(3), Some(3), true, false));
    assert_eq!(items.deleted(&episode_at(SID, "E4", "S", 2), true), (0, true));
    assert_eq!(tally(find(&items, "S2")), (Some(1), Some(0), false, true));
}

/// A show or season that held only the deleted episode goes with it; one whose row carries no
/// episode count stands as it was.
#[test]
fn a_show_or_season_that_held_only_the_deleted_episode_goes() {
    let mut items = SecItems::from_vec(vec![
        Some(show(SID, "S", 1, 0)),
        Some(season(SID, "S1", "S", 1, 1, 0)),
        Some(episode(SID, "E1", "S", 1)),
        Some(row(SID, "M")),
    ]);
    assert_eq!(items.deleted(&episode_at(SID, "E1", "S", 1), false), (3, false));
    assert_eq!(held(&items), [(SID, "M")]);

    let mut unknown = SecItems::from_vec(vec![
        Some(PmsMovie { leaf_count: None, ..show(SID, "S", 5, 0) }),
        Some(PmsMovie { leaf_count: None, ..season(SID, "S1", "S", 1, 5, 0) }),
        Some(episode(SID, "E1", "S", 1)),
    ]);
    assert_eq!(unknown.deleted(&episode_at(SID, "E1", "S", 1), false), (1, false));
    assert_eq!(held(&unknown), [(SID, "S"), (SID, "S1")]);
}

/// A count that changes a row in place rewrites only the page the row is on.
#[test]
fn a_counted_row_rewrites_only_its_own_page() {
    let mut items = SecItems::from_vec(
        (0..2 * PAGE).map(|i| Some(if i == 0 { show(SID, "S", 10, 4) } else { row(SID, &i.to_string()) })).collect());
    let before = items.clone();
    assert_eq!(items.deleted(&episode_at(SID, "E1", "S", 1), true), (0, true));
    assert_eq!(items.len(), 2 * PAGE);
    assert_eq!(tally(find(&items, "S")), (Some(9), Some(3), false, false));
    assert_eq!(tally(find(&before, "S")), (Some(10), Some(4), false, false), "a reader's clone keeps the old count");
    let page = |s: &SecItems, p: usize| s.pages[p].clone().unwrap();
    assert!(!Arc::ptr_eq(&page(&items, 0), &page(&before, 0)));
    assert!(Arc::ptr_eq(&page(&items, 1), &page(&before, 1)));
}

/// Every section that holds the deleted row loses it and one from its total, and owes the
/// first-letter rail again; a section the row never reached, and one that has not listed yet, keep
/// everything.
#[test]
fn a_deletion_leaves_every_section_that_held_the_row() {
    let mut browse = TestBrowse::default();
    browse.state.states = vec![listed(&["a", "b", "c"]), listed(&["b", "d"]), listed(&["x", "y"]), SecState::default()];
    let untouched = browse.state.states()[2].items.clone();
    assert!(publish(&mut browse, ViewAt::leaf(SID, "b"), false));
    let states = browse.state.states();
    assert_eq!(rks_of(&states[0].items), some(&["a", "c"]));
    assert_eq!((states[0].total, states[0].letters_gen), (2, 1));
    assert_eq!(rks_of(&states[1].items), some(&["d"]));
    assert_eq!((states[1].total, states[1].letters_gen), (1, 1));
    assert!(Arc::ptr_eq(&states[2].items.pages, &untouched.pages), "a section without the row is not copied");
    assert_eq!((states[2].total, states[2].letters_gen), (2, 0));
    assert_eq!((states[3].total, states[3].letters_gen), (-1, 0), "a section that has not listed keeps its state");
}

/// A deletion no section's loaded rows reach reports nothing and changes nothing.
#[test]
fn a_deletion_no_loaded_row_reaches_reports_nothing() {
    let mut browse = TestBrowse::default();
    browse.state.states = vec![listed(&["a", "b"])];
    let before = browse.state.states()[0].items.clone();
    for at in [ViewAt::leaf(SID, "zzz"), episode_at(SID, "E9", "S", 1)] {
        assert!(!publish(&mut browse, at, true));
    }
    let state = &browse.state.states()[0];
    assert_eq!((state.total, state.letters_gen), (2, 0));
    assert!(Arc::ptr_eq(&state.items.pages, &before.pages));
}

/// The re-list window a section holds back is edited with the rows on screen, so the swap that
/// comes does not bring the row back; a row only that window had is no change on screen.
#[test]
fn a_deletion_also_reaches_the_relist_window_being_held_back() {
    let mut browse = TestBrowse::default();
    browse.state.states = vec![SecState {
        relist: true,
        relist_items: Some(rows(SID, &["a", "b", "c", "d"])),
        ..listed(&["a", "b", "c"])
    }];
    assert!(publish(&mut browse, ViewAt::leaf(SID, "b"), false));
    let state = &browse.state.states()[0];
    assert_eq!(rks_of(&state.items), some(&["a", "c"]));
    assert_eq!(rks_of(state.relist_items.as_ref().unwrap()), some(&["a", "c", "d"]));
    assert_eq!(state.total, 2);
    assert!(!publish(&mut browse, ViewAt::leaf(SID, "d"), false), "only the pending window had it");
    let state = &browse.state.states()[0];
    assert_eq!(rks_of(state.relist_items.as_ref().unwrap()), some(&["a", "c"]));
    assert_eq!(state.total, 2);
}

/// The app re-lists right after a deletion. The window that lands from that re-list is the
/// server's, which no longer has the row: it replaces the listing with exactly the rows that stay.
#[test]
fn a_re_list_after_a_deletion_lands_the_rows_that_stay() {
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, client) = loopback_page_source();
    list(&mut browse, sid, 4);
    browse.state.want(0, PAGE, None);
    assert!(publish(&mut browse, ViewAt::leaf(sid, "1"), false));
    assert_eq!(rks_of(&browse.state.states()[0].items), some(&["0", "2", "3"]));
    assert_eq!(browse.state.states()[0].total, 3);
    assert!(browse.state.listings_changed(sid, true), "the re-list the app asks for right after");
    browse.maybe_spawn();
    assert!(browse.adapter.page.busy(), "the re-list is read");
    let gen = browse.state.query_gen();
    land_page(&mut browse, client, sid, gen, 0, names([0, 2, 3].into_iter()), 3);
    let state = &browse.state.states()[0];
    assert_eq!(rks_of(&state.items), some(&["0", "2", "3"]));
    assert_eq!(state.total, 3);
    assert!(!state.relist, "the re-list is spent");
}

/// A re-list that had landed its first page when the deletion was published (the second page
/// still out) has the row removed from the window it holds, so the window that completes it and
/// the swap that follows are consistent with the listing on screen.
#[test]
fn a_re_list_window_half_landed_across_a_deletion_stays_consistent() {
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, client) = loopback_page_source();
    let all = PAGE + 2;
    list(&mut browse, sid, all);
    browse.state.want(0, all, None);
    assert!(browse.state.listings_changed(sid, true));
    browse.maybe_spawn();
    let gen = browse.state.query_gen();
    land_page(&mut browse, client, sid, gen, 0, names(0..PAGE), all as i64);
    assert_eq!(browse.state.states()[0].relist_next((0, all), None), Some(PAGE), "the second page is still owed");

    assert!(publish(&mut browse, ViewAt::leaf(sid, "5"), false));
    let state = &browse.state.states()[0];
    assert_eq!(state.total, (all - 1) as i64);
    assert_eq!(state.items.get(5).map(|m| m.rk.as_str()), Some("6"));
    assert_eq!(state.items.get(PAGE).map(|m| m.rk.as_str()), Some("61"));
    let pending = state.relist_items.as_ref().unwrap();
    assert_eq!(pending.len(), all - 1);
    assert_eq!(pending.get(5).map(|m| m.rk.as_str()), Some("6"));
    assert!(pending.get(PAGE - 1).is_none() && pending.page_missing(0), "a hole where page 1 was not loaded");
    assert!(!browse.state.listings_changed(sid, true), "a re-list is out: one more is owed, not a second read");
    assert_eq!(browse.state.states()[0].relist_next((0, all), None), Some(0));

    browse.maybe_spawn();
    let gen = browse.state.query_gen();
    land_page(&mut browse, client, sid, gen, 0, names((0..=PAGE).filter(|&i| i != 5)), (all - 1) as i64);
    assert_eq!(browse.state.states()[0].relist_next((0, all), None), Some(PAGE), "the tail is read next");
    browse.maybe_spawn();
    let gen = browse.state.query_gen();
    land_page(&mut browse, client, sid, gen, PAGE, names(std::iter::once(PAGE + 1)), (all - 1) as i64);
    let state = &browse.state.states()[0];
    let stays: Vec<Option<String>> = (0..=PAGE + 1).filter(|&i| i != 5).map(|i| Some(i.to_string())).collect();
    assert_eq!(rks_of(&state.items), stays, "the swapped listing is the server's, without the deleted row");
    assert_eq!(state.total, (all - 1) as i64);
    assert!(state.relist, "the re-list owed by the change that came while one was out follows");
}

/// A page read that was out for the old listing when the deletion was published lands after it
/// and is dropped by the re-list's generation fence: it neither brings the row back nor restores
/// the old length.
#[test]
fn a_page_read_out_across_a_deletion_is_dropped_when_it_lands() {
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, client) = loopback_page_source();
    let all = PAGE + 2;
    {
        let state = browse.state.state_mut(0).unwrap();
        state.fetch = SecFetch::Ready;
        state.total = all as i64;
        state.items = SecItems::from_vec((0..all).map(|i| (i < PAGE).then(|| row(sid, &i.to_string()))).collect());
    }
    browse.state.want(0, all, None);
    browse.maybe_spawn();
    assert!(browse.adapter.page.busy(), "the second page is asked for");
    let stale = browse.state.query_gen();
    assert!(publish(&mut browse, ViewAt::leaf(sid, "5"), false));
    assert!(browse.state.listings_changed(sid, true));
    assert_ne!(browse.state.query_gen(), stale, "the re-list fences the read out");
    land_page(&mut browse, client, sid, stale, PAGE, names(PAGE..all), all as i64);
    let state = &browse.state.states()[0];
    assert!(state.items.get(PAGE - 1).is_none() && state.items.get(PAGE).is_none(), "the old page did not land");
    assert_eq!(state.total, (all - 1) as i64);
}

/// The deleted card leaves every shelf it is on and the cards after it close up, so a later page
/// read starts where the listing now holds the next card; the window still begins where it did.
#[test]
fn a_deleted_card_leaves_every_shelf_and_the_positions_close_up() {
    let paged = section_hubs::Shelf {
        positions: vec![10, 11, 12, 13],
        offset: 10,
        end: 14,
        total: 100,
        more: true,
        ..shelf("a", SID, &["a-0", "a-1", "a-2", "a-3"])
    };
    let mut browse = on_screen(vec![paged, shelf("b", SID, &["b-0", "b-1", "b-2", "a-1"])]);
    let revision = browse.state.states()[0].hubs.revision;
    assert!(publish(&mut browse, ViewAt::leaf(SID, "a-1"), false));
    let hubs = &browse.state.states()[0].hubs;
    assert_eq!(hubs.revision, revision + 1, "the screen is told the shelves changed");
    let (a, b) = (&hubs.shelves()[0], &hubs.shelves()[1]);
    assert_eq!(cards(a), ["a-0", "a-2", "a-3"]);
    assert_eq!((a.positions.clone(), a.offset, a.end, a.total, a.more), (vec![10, 11, 12], 10, 13, 99, true));
    assert_eq!(cards(b), ["b-0", "b-1", "b-2"]);
    assert_eq!((b.positions.clone(), b.end, b.total), (vec![0, 1, 2], 3, 3));
}

/// A shelf the deletion empties is dropped (a hub that arrives empty is never published), a
/// released row holds no cards and is left alone, and a deletion no card is on copies nothing.
#[test]
fn a_shelf_the_deletion_empties_is_dropped_and_a_released_one_is_left_alone() {
    let released = section_hubs::Shelf { shown: 3, ..shelf("r", SID, &[]) };
    let mut browse = on_screen(vec![shelf("a", SID, &["a-0"]), released, shelf("c", SID, &["c-0", "c-1"])]);
    assert!(publish(&mut browse, ViewAt::leaf(SID, "a-0"), false));
    let ids = |b: &TestBrowse| b.state.states()[0].hubs.shelves().iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&browse), ["r", "c"]);
    let (revision, committed) = (browse.state.states()[0].hubs.revision, browse.state.states()[0].hubs.committed.clone());
    assert!(!publish(&mut browse, ViewAt::leaf(SID, "zzz"), false));
    let hubs = &browse.state.states()[0].hubs;
    assert_eq!(hubs.revision, revision);
    assert!(Arc::ptr_eq(&hubs.committed, &committed), "a deletion that reaches no card copies no shelf");
}

/// A show card on a shelf gives the deleted episode up like a listing row does.
#[test]
fn a_shelf_card_of_the_deleted_episodes_show_gives_the_episode_up() {
    let shows = section_hubs::Shelf {
        items: vec![show(SID, "S", 10, 4), show(SID, "T", 3, 3)],
        ..shelf("shows", SID, &["S", "T"])
    };
    let mut browse = on_screen(vec![shows]);
    assert!(publish(&mut browse, episode_at(SID, "E1", "S", 1), true));
    let on_shelf = &browse.state.states()[0].hubs.shelves()[0].items;
    assert_eq!(on_shelf.len(), 2);
    assert_eq!(tally(&on_shelf[0]), (Some(9), Some(3), false, false));
    assert_eq!(tally(&on_shelf[1]), (Some(3), Some(3), false, false));

    let only = section_hubs::Shelf { items: vec![show(SID, "S", 1, 0)], ..shelf("shows", SID, &["S"]) };
    let mut browse = on_screen(vec![only]);
    assert!(publish(&mut browse, episode_at(SID, "E1", "S", 1), false));
    assert!(browse.state.states()[0].hubs.shelves().is_empty(), "the show held only that episode");
}

/// A row that pages through its ledger loses the deleted key from it, so its positions keep
/// indexing the ledger; a random row's sample preview loses it too, so its listing offsets keep
/// counting from the end of the preview.
#[test]
fn a_ledger_row_and_a_sample_row_forget_the_deleted_key() {
    let ledger = Ledger {
        keys: ["b-0", "b-1", "b-2", "b-3"].into_iter().map(LedgerKey::new).collect(),
        active: true,
        ..Default::default()
    };
    let paged = section_hubs::Shelf {
        positions: vec![1, 2, 3],
        offset: 1,
        end: 4,
        total: 4,
        row: RowState { ledger: Some(ledger), ..Default::default() },
        ..shelf("b", SID, &["b-1", "b-2", "b-3"])
    };
    let sample = section_hubs::Shelf {
        row: RowState { mode: RowMode::Sample(vec!["r-0".into(), "r-1".into()]), ..Default::default() },
        ..shelf("r", SID, &["r-0", "r-1"])
    };
    let mut browse = on_screen(vec![paged, sample]);
    assert!(publish(&mut browse, ViewAt::leaf(SID, "b-2"), false));
    assert!(publish(&mut browse, ViewAt::leaf(SID, "r-1"), false));
    let shelves = browse.state.states()[0].hubs.shelves();
    assert_eq!(cards(&shelves[0]), ["b-1", "b-3"]);
    assert_eq!((shelves[0].positions.clone(), shelves[0].end, shelves[0].total), (vec![1, 2], 3, 3));
    let kept = &shelves[0].row.ledger.as_ref().unwrap().keys;
    assert_eq!(kept.iter().map(LedgerKey::text).collect::<Vec<_>>(), ["b-0", "b-1", "b-3"]);
    assert_eq!(shelves[1].row.mode, RowMode::Sample(vec!["r-0".to_string()]));
}

/// A section's shelves read that was out when the deletion was published lands carrying the item;
/// the ledger's replay takes it off again, closes the positions up, and a shelf it leaves empty is
/// not published.
#[test]
fn a_section_shelves_read_in_flight_across_a_deletion_lands_without_the_item() {
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, _client) = loopback_page_source();
    let ledger = browse.state.ledger.clone();
    browse.hubs_kick(0);
    ledger.record(&ViewAt::leaf(sid, "2"), ViewEdit::Deleted { watched: false });
    let (job, port) = browse.take_hubs_job();
    let shelves = vec![shelf("a", sid, &["1", "2", "3"]), shelf("b", sid, &["2"]), shelf("c", sid, &["4"])];
    port.done(job.addr, section_hubs::BrowseHubsAnswer::list_for_test(&job, shelves));
    browse.hubs_land();
    assert!(browse.state.state_mut(0).unwrap().hubs.commit_staged(true));
    let landed = browse.state.states()[0].hubs.shelves().to_vec();
    assert_eq!(landed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "c"], "the emptied shelf is not published");
    assert_eq!(cards(&landed[0]), ["1", "3"]);
    assert_eq!((landed[0].positions.clone(), landed[0].total, landed[0].end), (vec![0, 1], 2, 2));
    assert_eq!(cards(&landed[1]), ["4"]);
}

/// A set that has landed but is held back from the screen is edited too, so publishing it later
/// shows the shelf without the item.
#[test]
fn a_staged_shelf_set_loses_the_deleted_item_before_it_is_published() {
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, _client) = loopback_page_source();
    browse.hubs_kick(0);
    let (job, port) = browse.take_hubs_job();
    port.done(job.addr, section_hubs::BrowseHubsAnswer::list_for_test(&job, vec![shelf("a", sid, &["1", "2", "3"])]));
    browse.hubs_land();
    assert!(!publish(&mut browse, ViewAt::leaf(sid, "2"), false), "nothing is on screen yet");
    assert!(browse.state.state_mut(0).unwrap().hubs.commit_staged(true));
    assert_eq!(cards(&browse.state.states()[0].hubs.shelves()[0]), ["1", "3"]);
}

/// A row's page read (the second hubs request site) that was out when the deletion was published
/// lands without the item, with the window closed up and the listing's total one lower.
#[test]
fn a_hub_row_page_in_flight_across_a_deletion_lands_closed_up() {
    const ID: &str = "movie.row0";
    const KEY: &str = "/hubs/sections/1/row0";
    let _g = plx_base::testlock::serial();
    let (_cleanup, mut browse, sid, _client) = loopback_page_source();
    section_hubs::seed_named_shelves_for_owner_test(&mut browse.state, 0, &[(ID, KEY, "Row")], 12);
    {
        let seeded = &mut Arc::make_mut(&mut browse.state.state_mut(0).unwrap().hubs.committed)[0];
        seeded.more = true;
        seeded.total = 500;
    }
    let ledger = browse.state.ledger.clone();
    let seen = browse.state.states()[0].hubs.revision;
    assert!(browse.hubs_page(0, seen, ID, KEY, false), "the page ask was queued");
    ledger.record(&ViewAt::leaf(sid, "20"), ViewEdit::Deleted { watched: false });
    let (job, port) = browse.take_hubs_job();
    let mut next = browse.state.states()[0].hubs.committed[0].clone();
    next.items = vec![row(sid, "20"), row(sid, "21")];
    next.positions = vec![12, 13];
    next.offset = 12;
    next.end = 14;
    port.done(job.addr, section_hubs::BrowseHubsAnswer::row_for_test(&job, ID, KEY, next));
    browse.hubs_land();
    let landed = &browse.state.states()[0].hubs.committed[0];
    assert_eq!(cards(landed), ["21"]);
    assert_eq!(
        (landed.positions.clone(), landed.offset, landed.end, landed.total, landed.more),
        (vec![12], 12, 13, 499, true)
    );
}
