//! **How a profile arranges its Home rows — the rules, pure.**
//!
//! The layout is part of the profile's Home record ([`HomePins::rows`](super::session::HomePins::rows)),
//! beside its library pins. A row is named by what survives a refetch — its server's machine id,
//! its hub family and its library ([`HomeRow`]) — and never by position: Plex reorders its hubs, and
//! a hidden row has to stay hidden through that. The Hubs merge (`data::pms`) orders what it
//! publishes with [`arrange`]; the Settings > Home page records an edit with [`record_edit`].
use super::session::HomeRow;

/// Layout entries one profile keeps. Entries for rows no longer offered are forgotten first; a row
/// whose entry is forgotten returns at the end of Home, shown.
pub const MAX_HOME_ROWS: usize = 128;

/// A hub's family and library, from its `hubIdentifier` and listing `key`: the part of a row's name
/// that holds still while Plex rotates what the row is about.
///
/// The library is the identifier's first all-digit segment (`movie.recentlyadded.1`,
/// `movie.by.actor.or.director.1.3932`), else the key's `sectionID` (`home.television.recent`, which
/// PMS mints once per TV library), else 0. The family is what precedes that segment, so every actor
/// or genre rail of one library is one row to arrange; a collection keeps its whole identifier,
/// each collection being a row of its own. A hub with no identifier is named by its key.
pub fn family_and_section<'a>(identifier: &'a str, key: &'a str) -> (&'a str, i64) {
    if identifier.is_empty() {
        return (key, query_section(key));
    }
    let mut at = 0;
    for segment in identifier.split('.') {
        if !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()) {
            let section = segment.parse().unwrap_or(0);
            if at == 0 || identifier.starts_with("custom.collection.") {
                return (identifier, section);
            }
            return (&identifier[..at - 1], section);
        }
        at += segment.len() + 1;
    }
    (identifier, query_section(key))
}

fn query_section(key: &str) -> i64 {
    key.split_once('?')
        .and_then(|(_, query)| query.split('&').find_map(|pair| pair.strip_prefix("sectionID=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// The display order of the live rows. `ranks[i]` is the layout entry live row `i` matched, as
/// `(entry index, hidden)`, or `None` for a row the layout does not name. Named rows come first in
/// the layout's order (rows sharing an entry stay together, in live order); the rest follow in live
/// order, shown. Returns `(live index, hidden)` for every row, hidden ones included.
pub fn arrange(ranks: &[Option<(usize, bool)>]) -> Vec<(usize, bool)> {
    let mut order: Vec<usize> = (0..ranks.len()).collect();
    order.sort_by_key(|&i| ranks[i].map_or(usize::MAX, |(at, _)| at));
    order.into_iter().map(|i| (i, ranks[i].is_some_and(|(_, hidden)| hidden))).collect()
}

fn same_row(a: &HomeRow, b: &HomeRow) -> bool {
    a.server == b.server && a.hub == b.hub && a.section == b.section
}

/// The layout after an edit. `offered` is every row the page listed, in its new order with its new
/// hidden flag. An entry for a row not on offer now (its server asleep, its hub absent for the
/// moment) keeps its place after the offered row that preceded it, so the row comes back where the
/// profile left it.
pub fn record_edit(stored: &[HomeRow], offered: Vec<HomeRow>) -> Vec<HomeRow> {
    // each entry not on offer, with the offered row it follows (`None`: it leads the layout)
    let mut resting: Vec<(Option<usize>, &HomeRow)> = Vec::new();
    let mut anchor = None;
    for entry in stored {
        if let Some(at) = offered.iter().position(|row| same_row(row, entry)) {
            anchor = Some(at);
        } else if !resting.iter().any(|(_, kept)| same_row(kept, entry)) {
            resting.push((anchor, entry));
        }
    }
    let following = |at: Option<usize>| resting.iter()
        .filter(move |(anchor, _)| *anchor == at)
        .map(|(_, entry)| ((*entry).clone(), false));
    let mut out: Vec<(HomeRow, bool)> = following(None).collect();
    for (at, mut row) in offered.into_iter().enumerate() {
        if out.iter().any(|(kept, _)| same_row(kept, &row)) {
            continue;
        }
        if let Some(entry) = stored.iter().find(|entry| same_row(entry, &row)) {
            row.extensions = entry.extensions.clone();
        }
        out.push((row, true));
        out.extend(following(Some(at)));
    }
    while out.len() > MAX_HOME_ROWS {
        let at = out.iter().rposition(|(_, offered)| !offered).unwrap_or(out.len() - 1);
        out.remove(at);
    }
    out.into_iter().map(|(row, _)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(server: &str, hub: &str, section: i64, hidden: bool) -> HomeRow {
        HomeRow { server: server.into(), hub: hub.into(), section, hidden, ..Default::default() }
    }

    #[test]
    fn a_row_is_named_by_its_family_and_library() {
        assert_eq!(family_and_section("movie.recentlyadded.1", "/library/sections/1/all"), ("movie.recentlyadded", 1));
        assert_eq!(family_and_section("movie.by.actor.or.director.1.3932", ""), ("movie.by.actor.or.director", 1));
        assert_eq!(family_and_section("home.television.recent", "/hubs/home/recentlyAdded?type=2&sectionID=2"),
            ("home.television.recent", 2));
        assert_eq!(family_and_section("custom.collection.1.5432.5432", ""), ("custom.collection.1.5432.5432", 1));
        assert_eq!(family_and_section("home.continue", ""), ("home.continue", 0));
        assert_eq!(family_and_section("", "/library/sections/3/all?type=1"), ("/library/sections/3/all?type=1", 0));
    }

    #[test]
    fn named_rows_lead_in_layout_order_and_new_rows_follow_shown() {
        // live: A (entry 2), B (new), C (entry 0, hidden), D (entry 2), E (new)
        let ranks = [Some((2, false)), None, Some((0, true)), Some((2, false)), None];
        assert_eq!(arrange(&ranks), vec![(2, true), (0, false), (3, false), (1, false), (4, false)]);
        assert_eq!(arrange(&[None, None]), vec![(0, false), (1, false)], "no layout is the live order");
    }

    #[test]
    fn an_edit_keeps_entries_for_rows_not_on_offer_where_they_stood() {
        let stored = [row("m9", "gone", 0, false), row("", "home.continue", 0, false),
            row("m1", "asleep.head", 0, true), row("m1", "a", 1, false), row("m2", "asleep", 1, true),
            row("m1", "b", 1, false)];
        let edited = record_edit(&stored, vec![row("m1", "b", 1, false), row("m1", "a", 1, true),
            row("", "home.continue", 0, false), row("m3", "new", 2, false)]);
        let names: Vec<_> = edited.iter().map(|r| (r.hub.as_str(), r.hidden)).collect();
        assert_eq!(names, [("gone", false), ("b", false), ("a", true), ("asleep", true),
            ("home.continue", false), ("asleep.head", true), ("new", false)]);
    }

    #[test]
    fn the_cap_forgets_rows_not_on_offer_first() {
        let stored: Vec<_> = (0..MAX_HOME_ROWS as i64).map(|n| row("old", "gone", n, true)).collect();
        let offered: Vec<_> = (0..4).map(|n| row("m1", "live", n, false)).collect();
        let edited = record_edit(&stored, offered);
        assert_eq!(edited.len(), MAX_HOME_ROWS);
        assert_eq!(edited.iter().filter(|r| r.server == "m1").count(), 4, "every offered row is kept");
        let last_resting = edited.iter().rev().find(|r| r.server == "old").map(|r| r.section);
        assert_eq!(last_resting, Some(MAX_HOME_ROWS as i64 - 5), "the last resting entries go");
    }
}
