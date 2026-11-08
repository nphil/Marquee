//! Local optimistic edits: watched/unwatched flips and Continue Watching deck removal.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;
use super::test_support::land;

/// Every appearance of the item flips, deck and shelves alike — a home screen that marked one
/// of them and not the other would be two answers about one film on one screen. And the resume
/// point goes with the flag: `poster_mark` reads progress AHEAD of watched, so a row keeping its
/// old `viewOffset` wears its old bar and no tick, which reads as the press having done nothing.
#[test]
fn marking_an_item_watched_flips_every_row_that_names_it_and_retires_its_resume_bar() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let mut b = SourceBuild {
        lane: Default::default(),
        cw: vec![CwItem {
            last_viewed_at: 9,
            m: started(0, "7"),
            position: 0,
        }],
        shelves: vec![shelf(
            0,
            "Recently Added",
            "home.movies.recent",
            &["7", "8"],
        )],
    };

    assert!(apply_edit(&mut b, &ViewAt::leaf(sid(0), "7"), ViewEdit::Watched(true)));

    assert!(b.cw[0].m.watched && !b.cw[0].m.unwatched, "the deck card");
    assert_eq!(b.cw[0].m.resume_ms, 0, "…and its bar retires with the flag");
    assert!(
        b.shelves[0].items[0].watched,
        "the same film on another shelf"
    );
    assert!(!b.shelves[0].items[1].watched, "and nothing else on it");

    // …and the reverse toggle is the exact inverse, on a container as much as on a leaf
    assert!(apply_edit(&mut b, &ViewAt::leaf(sid(0), "7"), ViewEdit::Watched(false)));
    assert!(!b.cw[0].m.watched && b.cw[0].m.unwatched);
    reset(&mut o.state, &o.adapter);
}

/// An item on another server that happens to share the ratingKey is a DIFFERENT item — the rule
/// `plex::same_item` exists for, applied to the one edit that writes to a row rather than
/// reading one. A bare-key match here would tick a friend's film because you finished yours.
#[test]
fn an_edit_never_reaches_the_same_rating_key_on_another_server() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let mut b = SourceBuild {
        lane: Default::default(),
        cw: Vec::new(),
        shelves: vec![shelf(1, "Theirs", "h", &["7"])],
    };
    assert!(
        !apply_edit(&mut b, &ViewAt::leaf(sid(0), "7"), ViewEdit::Watched(true)),
        "nothing matched"
    );
    assert!(!b.shelves[0].items[0].watched);
    reset(&mut o.state, &o.adapter);
}

/// Remove from Continue Watching is a HIDE and nothing else: the card leaves the deck, keeps its
/// resume point, and its appearances on every OTHER shelf are untouched — which is exactly what
/// the server does (`plex::Client::remove_from_continue_watching`). Marking it watched instead
/// would throw the position away, which is the mistake that endpoint exists to avoid.
#[test]
fn a_deck_removal_leaves_the_deck_only_and_keeps_the_resume_point() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let mut b = SourceBuild {
        lane: Default::default(),
        cw: vec![
            CwItem {
                last_viewed_at: 9,
                m: started(0, "7"),
                position: 0,
            },
            CwItem {
                last_viewed_at: 8,
                m: started(0, "8"),
                position: 0,
            },
        ],
        shelves: vec![Shelf { epoch: 0, positions: Vec::new(),
            title: "Recently Added".into(),
            hub_id: "home.movies.recent".into(),
            key: String::new(),
            items: vec![started(0, "7")],
            total: 0, offset: 0, end: 0, more: false, shown: 0, row: Default::default(),
        }],
    };

    assert!(apply_edit(&mut b, &ViewAt::leaf(sid(0), "7"), ViewEdit::LeftDeck));

    assert_eq!(b.cw.len(), 1, "only the one asked for leaves");
    assert_eq!(b.cw[0].m.rk, "8");
    assert_eq!(b.shelves[0].items[0].rk, "7", "its other shelf keeps it");
    assert_eq!(
        b.shelves[0].items[0].resume_ms,
        30 * 60_000,
        "…with the position it had"
    );
    assert!(
        !b.shelves[0].items[0].watched,
        "…and its watch state untouched: this is a HIDE"
    );
    reset(&mut o.state, &o.adapter);
}

/// The reason the edit goes through the projection and the pure `merge` rather than splicing
/// `CATALOG`: a `HubRow` is a `start`/`len` WINDOW into one flat vec and the hero pool holds
/// indices into the same, so a row removed by hand means fixing every window behind it. Here the
/// deck loses a card and the shelf behind it must still draw exactly its own items.
#[test]
fn a_removed_deck_card_leaves_the_shelves_behind_it_correctly_addressed() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let build = SourceBuild {
        lane: Default::default(),
        cw: vec![
            CwItem {
                last_viewed_at: 9,
                m: started(0, "7"),
                position: 0,
            },
            CwItem {
                last_viewed_at: 8,
                m: started(0, "8"),
                position: 0,
            },
        ],
        shelves: vec![shelf(
            0,
            "Recently Added",
            "home.movies.recent",
            &["a", "b"],
        )],
    };
    seed(&mut o.state, vec![src(0, "", HubState::Ready, Some(build))]);
    assert_eq!(rks(&o.state, 0), vec!["7", "8"], "the deck as it stands");
    assert_eq!(rks(&o.state, 1), vec!["a", "b"]);

    assert!(edit_item(&mut o.state, &ViewAt::leaf(sid(0), "7"), ViewEdit::LeftDeck));

    assert_eq!(rks(&o.state, 0), vec!["8"], "the card is gone from the deck");
    assert_eq!(
        rks(&o.state, 1),
        vec!["a", "b"],
        "and the shelf behind it still names its own items"
    );
    assert_eq!(hub_count(&o.state), 2, "no shelf appeared or vanished");
    reset(&mut o.state, &o.adapter);
}

/// An item on no shelf at all — a Library-grid or Related page press — must not re-commit Home
/// for nothing: the return value is what tells `viewstate` whether anything on screen moved, and
/// a commit here would free the catalog strings out from under a live `hub_title` borrow for no
/// reason at all.
#[test]
fn an_item_on_no_shelf_reports_no_edit_and_recommits_nothing() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    seed(&mut o.state, vec![src(0, "", HubState::Ready, Some(build_test(2)))]);
    assert!(!edit_item(&mut o.state, &ViewAt::leaf(sid(0), "not-on-home"), ViewEdit::Watched(true)));
    assert_eq!(hub_len(&o.state, 0), 2, "Home is exactly as it was");
    reset(&mut o.state, &o.adapter);
}

#[test]
fn scoped_optimistic_edit_cannot_restore_an_unpinned_sibling_library() {
    let _guard = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let sid = sid(0);
    let directory = two_library_directory(sid, true);
    seed_two_library_home_for_test(&mut o.state, sid, directory.view());
    assert_eq!(rks(&o.state, 0), ["alpha"]);

    let outcome = run_with_directory(
        &mut o.state,
        &o.adapter,
        crate::stores::hubs::HubsCmd::ApplyView {
            at: ViewAt::leaf(sid, "alpha"),
            edit: ViewEdit::Watched(true),
        },
        directory.view(),
        &registry_keys_for_test(),
    );

    assert!(outcome.changed);
    assert_eq!(rks(&o.state, 0), ["alpha"],
        "ApplyView must re-merge through the retained one-pinned directory");
    assert!(hub_item(&o.state, 0, 0).unwrap().watched);
    reset(&mut o.state, &o.adapter);
}

/// A publication hands the catalog the source's own cards: cloning a pointer per card, not the
/// card, is what keeps a Home publication cheap enough on the main thread to hold thousands.
#[test]
fn a_publication_shares_cards_with_its_source() {
    let _g = plx_base::testlock::serial();
    let mut source = Src::new(sid(0), String::new(), &registry_keys_for_test());
    source.state = HubState::Ready;
    source.last = Some(built(
        0,
        &[(9, "7")],
        vec![shelf(0, "Recently Added", "home.movies.recent", &["7", "8"])],
    ));
    let srcs = vec![source];
    let (cat, hubs, _pool) = merge(&srcs);
    let last = srcs[0].last.as_ref().unwrap();

    assert_eq!(hubs.len(), 2, "the deck and one shelf");
    assert!(Arc::ptr_eq(&cat[0], &last.cw[0].m), "the deck card is the source's card");
    assert!(Arc::ptr_eq(&cat[1], &last.shelves[0].items[0]), "a shelf card is the source's card");
    assert!(Arc::ptr_eq(&cat[2], &last.shelves[0].items[1]));
}

/// A watched flip on a card that is on several shelves must clone exactly the cards it changes:
/// the predicate is tested on the shared reference first, so a card that does not match is never
/// copied (`Arc::make_mut` on every card would deep-clone the whole catalog).
#[test]
fn a_watched_edit_clones_exactly_one_card() {
    let _g = plx_base::testlock::serial();
    let mut b = built(
        0,
        &[(9, "7"), (8, "8")],
        vec![shelf(0, "Recently Added", "home.movies.recent", &["8", "9", "10"])],
    );
    let before = b.clone(); // shares every card with `b`
    assert!(apply_edit(&mut b, &ViewAt::leaf(sid(0), "8"), ViewEdit::Watched(true)));

    // "8" is on the deck and on the shelf: those two cards (and only those) were copied
    let changed = |a: &Arc<PmsMovie>, c: &Arc<PmsMovie>| !Arc::ptr_eq(a, c);
    assert!(changed(&b.cw[1].m, &before.cw[1].m));
    assert!(changed(&b.shelves[0].items[0], &before.shelves[0].items[0]));
    assert!(b.cw[1].m.watched && b.shelves[0].items[0].watched);
    assert!(Arc::ptr_eq(&b.cw[0].m, &before.cw[0].m), "an unrelated deck card is not copied");
    for i in [1, 2] {
        assert!(
            Arc::ptr_eq(&b.shelves[0].items[i], &before.shelves[0].items[i]),
            "shelf card {i} is not copied"
        );
    }
    assert!(!before.cw[1].m.watched, "the source's own card is untouched");
}

/// A show-scoped change reaches the episode rows of that show wherever the hubs hold one (a
/// Continue Watching card, a Recently Aired shelf) and nothing else; a deck removal stays on the
/// one row pressed whatever the scope.
#[test]
fn a_show_scoped_edit_reaches_its_episode_rows_and_a_deck_removal_stays_on_one_row() {
    let _g = plx_base::testlock::serial();
    let episode = |rk: &str, show: &str, season: i32| {
        let mut m = (*started(0, rk)).clone();
        m.kind = 3;
        m.show_rk = show.into();
        m.season_index = season;
        Arc::new(m)
    };
    let mut b = SourceBuild {
        lane: Default::default(),
        cw: vec![
            CwItem { last_viewed_at: 9, m: episode("e11", "S", 1), position: 0 },
            CwItem { last_viewed_at: 8, m: episode("t11", "T", 1), position: 1 },
        ],
        shelves: vec![{
            let mut sh = shelf(0, "Recently Aired", "home.tv.recent", &[]);
            sh.items = vec![episode("e21", "S", 2), episode("e12", "S", 1)];
            sh.positions = vec![0, 1];
            sh
        }],
    };
    let show = ViewAt { sid: sid(0), rk: "S".into(), scope: crate::viewstate::Scope::Show };
    assert!(apply_edit(&mut b, &show, ViewEdit::Watched(true)));
    assert!(b.cw[0].m.watched && b.cw[0].m.resume_ms == 0, "the show's card on the deck");
    assert!(!b.cw[1].m.watched, "another show's card");
    assert!(b.shelves[0].items.iter().all(|m| m.watched), "every episode on the shelf");

    let season = ViewAt {
        sid: sid(0),
        rk: "S2".into(),
        scope: crate::viewstate::Scope::Season { show_rk: "S".into(), index: 2 },
    };
    assert!(apply_edit(&mut b, &season, ViewEdit::Watched(false)));
    let by_rk = |rk: &str| b.shelves[0].items.iter().find(|m| m.rk == rk).unwrap().watched;
    assert!(!by_rk("e21"), "season 2 is unmarked");
    assert!(by_rk("e12"), "season 1 stays marked");

    assert!(!apply_edit(&mut b, &show, ViewEdit::LeftDeck),
        "a deck removal names the one row pressed, never the episodes under a show");
    assert_eq!(b.cw.len(), 2);
}

/// Whatever a source's read brings — a hub shelf, not only the deck lane — describes the server
/// from when it was issued, so what was pressed since is made again on it: a marked card stays
/// marked and a card taken off Continue Watching stays off.
#[test]
fn a_source_read_in_flight_across_edits_lands_with_them_made_again() {
    let _g = plx_base::testlock::serial();
    let mut s = Src::new(sid(0), String::new(), &registry_keys_for_test());
    s.stamp = s.ledger.stamp(); // the read goes out…
    let p7 = s.ledger.record(&ViewAt::leaf(sid(0), "7"), ViewEdit::Watched(true)); // …then two presses
    let p8 = s.ledger.record(&ViewAt::leaf(sid(0), "8"), ViewEdit::LeftDeck);
    let mut brought = SourceBuild { lane: Default::default(),
        cw: vec![CwItem { last_viewed_at: 9, m: started(0, "7"), position: 0 },
            CwItem { last_viewed_at: 8, m: started(0, "8"), position: 1 }],
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["7", "8"])] };

    replay_since_request(&s, &mut brought);

    assert!(brought.shelves[0].items[0].watched, "the pressed card is marked on a shelf too");
    assert!(!brought.shelves[0].items[1].watched, "and nothing else");
    assert!(brought.cw[0].m.watched, "…and in the deck");
    assert_eq!(brought.cw.iter().map(|c| c.m.rk.as_str()).collect::<Vec<_>>(), ["7"],
        "a card taken off the deck while the read was out stays off");

    s.ledger.landed(p7); // both writes reach the server…
    s.ledger.landed(p8);
    s.stamp = s.ledger.stamp(); // …and a read issued after that carries them already
    let mut later = SourceBuild { lane: Default::default(), cw: Vec::new(),
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["7"])] };
    replay_since_request(&s, &mut later);
    assert!(!later.shelves[0].items[0].watched, "its answer stands");
}

/// The stamp is taken by the request itself (`Src::begin_request`), not assigned: an edit published
/// before the request is on the server by then and is not made again, one published while the read
/// is out is.
#[test]
fn a_source_request_takes_its_ledger_stamp_at_the_request() {
    let _g = plx_base::testlock::serial();
    plx_plex::plex::reset_servers_for_test();
    let registered = plx_plex::plex::register_for_test("pms-ledger", "127.0.0.1", 9, "t", "cid");
    assert_eq!(registered, sid(0));
    let client = plx_plex::plex::client_for(registered).unwrap();
    let mut s = Src::new(sid(0), String::new(), &registry_keys_for_test());
    s.ledger.record(&ViewAt::leaf(sid(0), "7"), ViewEdit::Watched(true)); // before the request
    assert!(s.begin_request(ClientKey::of(client), 1, || 1).is_some());
    s.ledger.record(&ViewAt::leaf(sid(0), "8"), ViewEdit::Watched(true)); // while it is out
    let mut brought = SourceBuild { lane: Default::default(), cw: Vec::new(),
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["7", "8"])] };
    replay_since_request(&s, &mut brought);
    assert!(!brought.shelves[0].items[0].watched, "the server already had the first press");
    assert!(brought.shelves[0].items[1].watched, "the second was made while the read was out");
    plx_plex::plex::reset_servers_for_test();
}

/// The read-back answers for every item a write touched, mostly agreeing with what the optimistic
/// edit already put on the row. Facts that change nothing are not a hit: a hit forces a full merge,
/// a Home commit and a notice, for rows that look exactly as before.
#[test]
fn facts_that_change_no_row_are_not_a_hit_but_facts_that_differ_are() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let mut b = SourceBuild {
        lane: Default::default(),
        cw: vec![CwItem { last_viewed_at: 9, m: started(0, "7"), position: 0 }],
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["7", "8"])],
    };
    let leaf = ViewAt::leaf(sid(0), "7");
    let eight = b.shelves[0].items[1].clone();
    let equal = ViewEdit::Facts { watched: eight.watched, unwatched: eight.unwatched, resume_ms: eight.resume_ms, sort: None };
    assert!(!apply_edit(&mut b, &ViewAt::leaf(sid(0), "8"), equal), "facts equal to the row: no hit");
    // "7" is a started card on the deck and a plain card on the shelf: facts that differ from either edit it
    let watched = ViewEdit::Facts { watched: true, unwatched: false, resume_ms: 0, sort: None };
    assert!(apply_edit(&mut b, &leaf, watched), "facts that differ from the row edit it");
    assert!(b.cw[0].m.watched && b.shelves[0].items[0].watched);
    assert_eq!(b.cw[0].m.resume_ms, 0);
    assert!(!apply_edit(&mut b, &leaf, watched), "the same facts again change nothing: no hit");
    reset(&mut o.state, &o.adapter);
}

/// The card for an episode of show `show`, season `season`, as the hubs hold it.
fn episode_card(rk: &str, show: &str, season: i32) -> Arc<PmsMovie> {
    Arc::new(PmsMovie { kind: KIND_EPISODE, show_rk: show.into(), season_index: season, ..(*started(0, rk)).clone() })
}

/// A show (`KIND_SHOW`) or season (`KIND_SEASON`) card holding `leaves` episodes, `viewed` of them
/// watched, with the flags `parse_item` derives from those counts.
fn container_card(rk: &str, kind: i32, show: &str, season: i32, leaves: u32, viewed: u32) -> Arc<PmsMovie> {
    Arc::new(PmsMovie {
        kind, show_rk: show.into(), season_index: season,
        leaf_count: Some(leaves), viewed_leaf_count: Some(viewed),
        watched: viewed >= leaves, unwatched: viewed == 0,
        ..(*row(0, rk)).clone()
    })
}

fn deleted(watched: bool) -> ViewEdit { ViewEdit::Deleted { watched } }

fn episode_at(rk: &str, show: &str, index: i64) -> ViewAt {
    ViewAt { sid: sid(0), rk: rk.into(), scope: crate::viewstate::Scope::Episode { show_rk: show.into(), index } }
}

fn deck_of(cw: &[CwItem]) -> Vec<&str> { cw.iter().map(|c| c.m.rk.as_str()).collect() }

fn items_of(s: &Shelf) -> Vec<&str> { s.items.iter().map(|m| m.rk.as_str()).collect() }

/// A lane the deck has been placed on, holding `cw` as its rows.
fn placed_lane(cw: &[CwItem]) -> deck::Lane {
    let mut lanes = [deck::Lane::preview("", "", cw.len(), cw.len(), true)];
    deck::place(&mut lanes, &[cw]);
    let [lane] = lanes;
    lane
}

/// A confirmed deletion takes the item off the deck, the lane behind it and every shelf, and a
/// shelf that lost a row closes up exactly as the server's listing did: later positions, the
/// window's end and the listing's total all drop by one, so the next page read does not skip a row.
/// A shelf left empty stays a shelf, and a card that stays is not copied.
#[test]
fn a_deleted_item_leaves_the_deck_the_lane_and_every_shelf() {
    let _g = plx_base::testlock::serial();
    let cw = vec![CwItem { last_viewed_at: 9, m: started(0, "7"), position: 0 },
        CwItem { last_viewed_at: 8, m: started(0, "8"), position: 0 }];
    let mut recent = shelf(0, "Recently Added", "home.movies.recent", &["a", "7", "b", "c"]);
    (recent.positions, recent.total, recent.offset, recent.end, recent.more) = (vec![10, 11, 12, 13], 20, 10, 14, true);
    let only = shelf(0, "Only Seven", "home.only", &["7"]);
    let mut b = SourceBuild { lane: placed_lane(&cw), cw, shelves: vec![recent, only] };
    let before = b.clone();

    assert!(apply_edit(&mut b, &ViewAt::leaf(sid(0), "7"), deleted(false)));

    assert_eq!(deck_of(&b.cw), ["8"]);
    assert_eq!(deck_of(&b.lane.rows), ["8"], "the lane drops it too");
    assert_eq!((b.lane.rows[0].position, b.lane.total), (0, 1), "and closes up over it");
    let s = &b.shelves[0];
    assert_eq!(items_of(s), ["a", "b", "c"]);
    assert_eq!(s.positions, [10, 11, 12], "the rows behind the removed one close up");
    assert_eq!((s.total, s.offset, s.end, s.more), (19, 10, 13, true));
    assert_eq!(b.shelves.len(), 2, "a shelf that emptied stays a shelf");
    assert!(b.shelves[1].items.is_empty() && b.shelves[1].positions.is_empty());
    assert!(Arc::ptr_eq(&b.cw[0].m, &before.cw[1].m), "a deck card that stays is not copied");
    assert!(Arc::ptr_eq(&s.items[0], &before.shelves[0].items[0]), "nor a shelf card");

    let mut elsewhere = before.clone();
    assert!(!apply_edit(&mut elsewhere, &ViewAt::leaf(sid(0), "not-on-home"), deleted(false)));
    assert!(!apply_edit(&mut elsewhere, &ViewAt::leaf(sid(1), "7"), deleted(false)), "another server's 7");
    assert_eq!((elsewhere.cw.len(), elsewhere.shelves[0].items.len()), (2, 4), "nothing moved");
}

/// A deleted show takes every card of its episodes and seasons, a deleted season the episodes of
/// that season only; another show's cards stay in both.
#[test]
fn a_deleted_show_or_season_takes_the_cards_under_it() {
    let _g = plx_base::testlock::serial();
    let rows = || {
        let mut recent = shelf(0, "Recently Aired", "home.tv.recent", &[]);
        recent.items = vec![container_card("S", KIND_SHOW, "", 0, 10, 4), episode_card("e21", "S", 2),
            episode_card("e12", "S", 1), container_card("S2", KIND_SEASON, "S", 2, 5, 0), episode_card("t11", "T", 1)];
        recent.positions = (0..5).collect();
        SourceBuild { lane: Default::default(),
            cw: vec![CwItem { last_viewed_at: 9, m: episode_card("e11", "S", 1), position: 0 },
                CwItem { last_viewed_at: 8, m: episode_card("t11", "T", 1), position: 1 }],
            shelves: vec![recent] }
    };

    let mut b = rows();
    let show = ViewAt { sid: sid(0), rk: "S".into(), scope: crate::viewstate::Scope::Show };
    assert!(apply_edit(&mut b, &show, deleted(false)));
    assert_eq!(deck_of(&b.cw), ["t11"]);
    assert_eq!(items_of(&b.shelves[0]), ["t11"], "the show, its episodes and its season are gone");
    assert_eq!(b.shelves[0].positions, [0], "the survivor is addressed from the top");

    let mut b = rows();
    let season = ViewAt { sid: sid(0), rk: "S2".into(), scope: crate::viewstate::Scope::Season { show_rk: "S".into(), index: 2 } };
    assert!(apply_edit(&mut b, &season, deleted(true)));
    assert_eq!(deck_of(&b.cw), ["e11", "t11"], "season 1's card stays on the deck");
    assert_eq!(items_of(&b.shelves[0]), ["S", "e12", "t11"]);
}

/// A deleted episode lowers the episode counts of its show and season where those are on a shelf:
/// the viewed count too when the episode was watched, the flags follow, and a show or season that
/// held only that episode goes. Cards the episode does not belong to are left as they are.
#[test]
fn a_deleted_episode_lowers_the_counts_of_its_show_and_season() {
    let _g = plx_base::testlock::serial();
    let build = |show: Arc<PmsMovie>, season: Arc<PmsMovie>| {
        let mut recent = shelf(0, "Recently Aired", "home.tv.recent", &[]);
        recent.items = vec![show.clone(), season, container_card("S2", KIND_SEASON, "S", 2, 5, 5),
            episode_card("e11", "S", 1), container_card("T", KIND_SHOW, "", 0, 4, 0)];
        recent.positions = (0..5).collect();
        (recent.total, recent.end) = (5, 5);
        SourceBuild { lane: Default::default(),
            cw: vec![CwItem { last_viewed_at: 9, m: show, position: 0 },
                CwItem { last_viewed_at: 8, m: episode_card("e11", "S", 1), position: 1 }],
            shelves: vec![recent] }
    };
    let at = episode_at("e11", "S", 1);

    let mut b = build(container_card("S", KIND_SHOW, "", 0, 10, 4), container_card("S1", KIND_SEASON, "S", 1, 3, 2));
    let before = b.clone();
    assert!(apply_edit(&mut b, &at, deleted(true)));
    let s = &b.shelves[0];
    assert_eq!(items_of(s), ["S", "S1", "S2", "T"], "only the episode itself leaves");
    let counts = |m: &PmsMovie| (m.leaf_count, m.viewed_leaf_count, m.watched, m.unwatched);
    assert_eq!(counts(&s.items[0]), (Some(9), Some(3), false, false), "a watched episode lowers both counts");
    assert_eq!(counts(&s.items[1]), (Some(2), Some(1), false, false));
    assert_eq!(counts(&b.cw[0].m), (Some(9), Some(3), false, false), "the deck's card of the show too");
    assert!(Arc::ptr_eq(&s.items[2], &before.shelves[0].items[2]), "another season is not copied");
    assert!(Arc::ptr_eq(&s.items[3], &before.shelves[0].items[4]), "nor another show");
    assert_eq!((s.total, s.end), (4, 4), "a counted card stays in the listing; the episode does not");
    assert_eq!(deck_of(&b.cw), ["S"]);

    let mut b = build(container_card("S", KIND_SHOW, "", 0, 10, 4), container_card("S1", KIND_SEASON, "S", 1, 3, 2));
    assert!(apply_edit(&mut b, &at, deleted(false)));
    let s = &b.shelves[0];
    assert_eq!(counts(&s.items[0]), (Some(9), Some(4), false, false), "an unwatched episode leaves the viewed count");
    assert_eq!(counts(&s.items[1]), (Some(2), Some(2), true, false), "and a season left all watched reads watched");

    let mut b = build(container_card("S", KIND_SHOW, "", 0, 1, 1), container_card("S1", KIND_SEASON, "S", 1, 1, 1));
    assert!(apply_edit(&mut b, &at, deleted(true)));
    assert_eq!(items_of(&b.shelves[0]), ["S2", "T"], "a show and a season that held only that episode go");
    assert_eq!((b.shelves[0].total, b.shelves[0].positions.clone()), (2, vec![0, 1]));
    assert!(b.cw.is_empty());

    let mut b = SourceBuild { lane: Default::default(), cw: Vec::new(),
        shelves: vec![shelf(0, "Top", "home.top", &["x"])] };
    assert!(!apply_edit(&mut b, &at, deleted(true)), "no card of the episode's show or season is on screen");
}

/// The Home catalog is rebuilt from the edited projection: the card leaves the deck and the shelves
/// behind it still name their own items, the copy facts of the removed card go with it (a newest
/// card leaves its copy outside the span the lane still holds), and what a quiet read staged carries
/// the deletion when it lands. An item on no shelf commits nothing.
#[test]
fn a_deleted_item_is_gone_from_home_and_from_what_a_quiet_read_staged() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    let grouped = |rk: &str, guid: u8| Arc::new(PmsMovie { show_guid: Some([guid; 12]), ..(*episode_card(rk, rk, 1)).clone() });
    let cw = vec![CwItem { last_viewed_at: 9, m: grouped("e1", 1), position: 0 },
        CwItem { last_viewed_at: 8, m: grouped("e2", 2), position: 0 }];
    let build = SourceBuild { lane: placed_lane(&cw), cw,
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["a", "e1", "b"])] };
    seed(&mut o.state, vec![src(0, "", HubState::Ready, Some(build.clone()))]);
    o.state.srcs[0].staged = Some(build);
    assert_eq!(o.state.srcs[0].copies.rks(), ["e1", "e2"], "premise: the facts hold both cards");

    assert!(edit_item(&mut o.state, &ViewAt::leaf(sid(0), "e1"), deleted(false)));

    assert_eq!(rks(&o.state, 0), ["e2"]);
    assert_eq!(rks(&o.state, 1), ["a", "b"], "the shelf behind the deck still names its own items");
    assert_eq!(o.state.srcs[0].copies.rks(), ["e2"], "the removed card's fact is gone with it");
    let staged = o.state.srcs[0].staged.as_ref().unwrap();
    assert_eq!(deck_of(&staged.cw), ["e2"]);
    assert_eq!(items_of(&staged.shelves[0]), ["a", "b"]);

    assert!(!edit_item(&mut o.state, &ViewAt::leaf(sid(0), "not-on-home"), deleted(false)));
    assert_eq!(hub_len(&o.state, 1), 2, "Home is exactly as it was");
    reset(&mut o.state, &o.adapter);
}

/// A hub read issued before the deletion and landing after it describes the server from before:
/// it must not bring the row back, on the deck or on a shelf, nor leave a show's count stale.
#[test]
fn a_hub_read_issued_before_a_deletion_lands_without_the_row() {
    let _g = plx_base::testlock::serial();
    let mut s = Src::new(sid(0), String::new(), &registry_keys_for_test());
    s.stamp = s.ledger.stamp(); // the read goes out…
    let gone = s.ledger.record(&ViewAt::leaf(sid(0), "7"), deleted(false)); // …then the deletion is confirmed
    s.ledger.record(&episode_at("e1", "S", 1), deleted(true));
    let mut brought = SourceBuild { lane: Default::default(),
        cw: vec![CwItem { last_viewed_at: 9, m: started(0, "7"), position: 0 },
            CwItem { last_viewed_at: 8, m: started(0, "8"), position: 1 }],
        shelves: vec![{
            let mut recent = shelf(0, "Recently Added", "home.movies.recent", &[]);
            recent.items = vec![row(0, "7"), row(0, "8"), episode_card("e1", "S", 1), container_card("S", KIND_SHOW, "", 0, 4, 3)];
            recent.positions = (0..4).collect();
            recent
        }] };

    replay_since_request(&s, &mut brought);

    assert_eq!(deck_of(&brought.cw), ["8"]);
    assert_eq!(items_of(&brought.shelves[0]), ["8", "S"]);
    assert_eq!(brought.shelves[0].positions, [0, 1]);
    let show = &brought.shelves[0].items[1];
    assert_eq!((show.leaf_count, show.viewed_leaf_count), (Some(3), Some(2)), "the show's count lost the episode");

    s.ledger.landed(gone); // the server has taken the deletion…
    s.stamp = s.ledger.stamp(); // …and a read issued after that carries it already
    let mut later = SourceBuild { lane: Default::default(), cw: Vec::new(),
        shelves: vec![shelf(0, "Recently Added", "home.movies.recent", &["7"])] };
    replay_since_request(&s, &mut later);
    assert_eq!(items_of(&later.shelves[0]), ["7"], "its answer stands");
}

/// The same through the real landing of a source's read: the row the late answer still held is not
/// on Home afterwards.
#[test]
fn a_landing_issued_before_a_deletion_does_not_publish_the_row() {
    let _g = plx_base::testlock::serial();
    let mut o = Owner::default();
    reset(&mut o.state, &o.adapter);
    seed(&mut o.state, vec![src(0, "", HubState::Loading, None)]);
    o.state.srcs[0].stamp = o.state.ledger.stamp(); // the read is out…
    o.state.ledger.record(&ViewAt::leaf(sid(0), "7"), deleted(false)); // …when the deletion is confirmed

    land(&o.state, &o.adapter, 0, Some(built(0, &[(9, "7"), (8, "8")],
        vec![shelf(0, "Recently Added", "home.movies.recent", &["a", "7", "b"])])));
    pump(&mut o.state, &o.adapter, 0.0);

    assert_eq!(rks(&o.state, 0), ["8"], "the deck");
    assert_eq!(rks(&o.state, 1), ["a", "b"], "and the shelf");
    reset(&mut o.state, &o.adapter);
}
