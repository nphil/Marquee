//! **Which items of the top bar a profile shows — the rules, pure.**
//!
//! The bar is two glass pills: the middle one (Home, the libraries, Requests) and the one at the top
//! right (Watchlist, Search). A profile hides items it does not use; the record is the list of
//! hidden names in its Home record ([`HomePins::bar_hidden`](super::session::HomePins::bar_hidden)),
//! beside its pins and Home layout, so an empty record shows everything. Home itself is never
//! offered: it is the page every other one returns to. Settings is never an item either; it opens
//! from the profile chip.

/// One hideable item of the top bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarItem {
    Movies,
    Shows,
    Requests,
    Watchlist,
    Search,
}

/// The two pills an item can stand in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarPill {
    Middle,
    Right,
}

impl BarItem {
    /// Every item, in the order the bar shows them: the middle pill's, then the right pill's.
    pub const ALL: [BarItem; 5] = [BarItem::Movies, BarItem::Shows, BarItem::Requests, BarItem::Watchlist, BarItem::Search];

    /// The pill the item stands in.
    pub const fn pill(self) -> BarPill {
        match self {
            BarItem::Movies | BarItem::Shows | BarItem::Requests => BarPill::Middle,
            BarItem::Watchlist | BarItem::Search => BarPill::Right,
        }
    }

    /// The name the profile's record keeps for it. Never reworded: an older build reads the same file.
    pub const fn name(self) -> &'static str {
        match self {
            BarItem::Movies => "movies",
            BarItem::Shows => "shows",
            BarItem::Requests => "requests",
            BarItem::Watchlist => "watchlist",
            BarItem::Search => "search",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The items a profile hides, as a set. `Copy`, so a snapshot carries it by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BarHidden(u8);

impl BarHidden {
    /// Read the record's names. A name this build does not know is left out here and kept in the
    /// record ([`with_item`]), so a newer build's items survive a round trip through this one.
    pub fn from_names(names: &[String]) -> Self {
        BarItem::ALL.iter().filter(|item| names.iter().any(|name| name == item.name()))
            .fold(BarHidden(0), |set, item| BarHidden(set.0 | item.bit()))
    }

    pub const fn hides(self, item: BarItem) -> bool {
        self.0 & item.bit() != 0
    }

    pub const fn none() -> Self {
        BarHidden(0)
    }
}

/// The record's names after `item` is shown (`hide` false) or hidden (`hide` true): every other
/// name, known or not, stays as it was.
pub fn with_item(names: &[String], item: BarItem, hide: bool) -> Vec<String> {
    let mut out: Vec<String> = names.iter().filter(|name| name.as_str() != item.name()).cloned().collect();
    if hide {
        out.push(item.name().to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(of: &[&str]) -> Vec<String> {
        of.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_empty_record_shows_every_item() {
        let hidden = BarHidden::from_names(&[]);
        assert!(BarItem::ALL.iter().all(|item| !hidden.hides(*item)));
        assert_eq!(hidden, BarHidden::none());
    }

    #[test]
    fn a_name_hides_exactly_its_item() {
        let hidden = BarHidden::from_names(&names(&["watchlist", "requests"]));
        let hid: Vec<_> = BarItem::ALL.into_iter().filter(|item| hidden.hides(*item)).collect();
        assert_eq!(hid, [BarItem::Requests, BarItem::Watchlist]);
    }

    #[test]
    fn hiding_and_showing_round_trip_and_keep_names_this_build_does_not_know() {
        let stored = names(&["from-a-newer-build"]);
        let hidden = with_item(&stored, BarItem::Search, true);
        assert_eq!(hidden, names(&["from-a-newer-build", "search"]));
        assert!(BarHidden::from_names(&hidden).hides(BarItem::Search));
        assert_eq!(with_item(&hidden, BarItem::Search, true), hidden, "hiding twice is one entry");
        assert_eq!(with_item(&hidden, BarItem::Search, false), stored);
    }

    #[test]
    fn the_pills_split_as_the_bar_draws_them() {
        let right: Vec<_> = BarItem::ALL.into_iter().filter(|item| item.pill() == BarPill::Right).collect();
        assert_eq!(right, [BarItem::Watchlist, BarItem::Search]);
        let all: std::collections::HashSet<_> = BarItem::ALL.iter().map(|item| item.name()).collect();
        assert_eq!(all.len(), BarItem::ALL.len(), "one name each");
    }
}

#[cfg(test)]
mod record_tests {
    use super::super::session::HomePins;

    /// The record is part of the profile's Home record: it round-trips, an older file without it
    /// shows everything, a damaged value is no record rather than an unreadable file, and an empty
    /// one writes nothing.
    #[test]
    fn the_record_round_trips_and_a_damaged_value_shows_everything() {
        let pins = HomePins { user: "u".into(), bar_hidden: vec!["search".into(), "later".into()], ..Default::default() };
        let json = serde_json::to_string(&pins).unwrap();
        assert_eq!(serde_json::from_str::<HomePins>(&json).unwrap().bar_hidden, pins.bar_hidden);
        let older = serde_json::from_str::<HomePins>(r#"{"user":"u"}"#).unwrap();
        assert!(older.bar_hidden.is_empty());
        let damaged = serde_json::from_str::<HomePins>(r#"{"user":"u","bar_hidden":5}"#).unwrap();
        assert!(damaged.bar_hidden.is_empty());
        assert!(!serde_json::to_string(&older).unwrap().contains("bar_hidden"));
    }
}
