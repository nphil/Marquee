//! The profile's Home layout in the merge: row order, hidden rows, the ring's row mapping, and the
//! rows Settings > Home lists.

use super::*;
use super::test_support::*;

fn rule(sid: Option<ServerId>, hub: &str, section: i64, hidden: bool) -> HomeRowRule {
    HomeRowRule { id: HomeRowId { sid, hub: hub.into(), section }, hidden }
}

fn scope(rows: &[HomeRowRule]) -> BrowseScope<'_> {
    BrowseScope { sections_gen: 0, pins: Vec::new(), clients: Default::default(), rows }
}

/// Our server with a deck and three shelves, and a friend's with one shelf of the same family.
fn sources() -> [Src; 2] {
    [
        src(0, "", HubState::Ready, Some(built(0, &[(1, "cw")], vec![
            shelf(0, "Films", "a.recent", &["a"]),
            shelf(0, "Shows", "b.recent", &["b"]),
            shelf(0, "New", "c.recent", &["c"]),
        ]))),
        src(1, "friend", HubState::Ready, Some(built(1, &[], vec![shelf(1, "Club", "a.recent", &["d"])]))),
    ]
}

fn titles(hubs: &[HubRow]) -> Vec<&str> {
    hubs.iter().map(|hub| hub.title.as_str()).collect()
}

#[test]
fn the_layout_orders_home_and_leaves_hidden_rows_out() {
    let _g = plx_base::testlock::serial();
    let srcs = sources();
    let rows = [
        rule(Some(sid(0)), "b.recent", 0, false),
        rule(None, DECK_ID, 0, false),
        rule(Some(sid(0)), "a.recent", 0, true),
    ];
    let (_, hubs, _) = merge_with_scope(&srcs, &scope(&rows));
    assert_eq!(titles(&hubs), ["Shows", "Continue Watching", "New", "Club"],
        "named rows lead in the layout's order, the hidden one is gone, the rest follow in Home's order");
    let (_, plain, _) = merge_with_scope(&srcs, &scope(&[]));
    assert_eq!(titles(&plain), ["Continue Watching", "Films", "Shows", "New", "Club"]);
    assert_ne!(scope(&rows).cache_key(), scope(&[]).cache_key(), "a new layout re-merges Home");
}

#[test]
fn the_ring_pairs_each_shelf_with_its_row_wherever_the_deck_sits() {
    let _g = plx_base::testlock::serial();
    let srcs = sources();
    let rows = [rule(Some(sid(0)), "b.recent", 0, false), rule(None, DECK_ID, 0, false)];
    let scope = scope(&rows);
    let (_, hubs, _) = merge_with_scope(&srcs, &scope);
    let pairs: Vec<(&str, &str)> = published_order(&srcs, &scope).into_iter().zip(shelf_rows(&hubs))
        .map(|((i, j), row)| (slot_shelf(&srcs, i, j).map_or("", |sh| sh.title.as_str()), hubs[row].title.as_str()))
        .collect();
    assert_eq!(pairs, [("Shows", "Shows"), ("Films", "Films"), ("New", "New"), ("Club", "Club")]);
}

#[test]
fn settings_lists_every_row_once_hidden_ones_included() {
    let _g = plx_base::testlock::serial();
    let srcs = [src(0, "", HubState::Ready, Some(built(0, &[(1, "cw")], vec![
        shelf_in(0, 1, "Starring A", "movie.by.actor.or.director.1.11", &["a"]),
        shelf_in(0, 1, "Starring B", "movie.by.actor.or.director.1.22", &["b"]),
        shelf(0, "Films", "a.recent", &["c"]),
    ])))];
    let rows = [rule(Some(sid(0)), "a.recent", 0, true)];
    let offers = offered_rows(&srcs, &scope(&rows));
    let listed: Vec<(&str, bool)> = offers.iter().map(|offer| (offer.title.as_str(), offer.hidden)).collect();
    assert_eq!(listed, [("Films", true), ("Continue Watching", false), ("Starring A", false)],
        "the actor rails of one library are one row, titled by the first");
    assert_eq!(offers[2].id, HomeRowId { sid: Some(sid(0)), hub: "movie.by.actor.or.director".into(), section: 1 });
}

#[test]
fn settings_shows_the_layout_as_recorded() {
    let offer = |hub: &str| HomeRowOffer {
        id: HomeRowId { sid: Some(sid(0)), hub: hub.into(), section: 0 },
        title: hub.into(),
        source: String::new(),
        hidden: false,
    };
    let offers = [offer("a"), offer("b"), offer("c")];
    let layout = [
        rule(Some(sid(0)), "c", 0, false),
        rule(Some(sid(0)), "gone", 0, false),
        rule(Some(sid(0)), "a", 0, true),
    ];
    let shown: Vec<(String, bool)> = arrange_offers(&offers, &layout).into_iter()
        .map(|offer| (offer.title, offer.hidden))
        .collect();
    assert_eq!(shown, [("c".to_string(), false), ("a".to_string(), true), ("b".to_string(), false)]);
}
