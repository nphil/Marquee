//! **The early Bazarr search.** Once a movie or an episode has played steadily for a while, and
//! only with Bazarr linked, the player asks Bazarr for that item's subtitle candidates in the
//! background, so the Subtitles menu's search opens on its answer. It only searches: nothing is
//! downloaded and the subtitle does not change until the viewer picks a candidate there.
//!
//! The answer lands in the subtitle-search store the Tracks panel reads, in the language the panel
//! opens on, so the panel finds its search already made (`PlayerOverlayScreen`'s Tick re-opens
//! only a search the store does not hold).

use plx_data::subsearch::{SearchStatus, SubSearchCmd, SubSearchView, SubSource};
use plx_plex::plex::{same_item, ServerId};

/// Playing time an item builds up before its search goes out, so the request stays clear of the
/// start of the video and the seeks that often follow it.
const SETTLE_MS: u32 = 10_000;

/// What the subtitle-search store is owed this frame.
#[derive(Debug, PartialEq)]
pub(super) enum Owed {
    Nothing,
    /// Open this search, then pump the store so its request goes out.
    Open(SubSearchCmd),
    /// Pump the store: the search is in flight, and it lands only while the store is pumped.
    Pump,
    /// Drop the search in flight: its item is no longer playing.
    Close,
}

/// The playing item as a search names it, and the language the Tracks panel opens on.
pub(super) struct Wanted {
    pub sid: ServerId,
    pub rk: String,
    pub lang: String,
}

#[derive(Default)]
enum Stage {
    /// Counting playing time towards [`SETTLE_MS`].
    #[default]
    Settling,
    /// Asked for this item, and pumped until the answer lands.
    InFlight { sid: ServerId, rk: String },
    /// Nothing more for this part: answered, already searched by the viewer, or not a title.
    Done,
}

/// One playing part's early search.
#[derive(Default)]
pub(super) struct Prefetch {
    /// The part (`route::cur_part_id`) the count belongs to; a new one starts it again.
    part: i64,
    played_ms: u32,
    stage: Stage,
}

impl Prefetch {
    /// One frame of `part`, which played for `dt_ms` if `playing`. `wanted` names the item, `None`
    /// for one Bazarr does not manage; it is called once, when the search falls due.
    pub(super) fn step(&mut self, part: i64, playing: bool, dt_ms: u32, view: Option<SubSearchView<'_>>,
        wanted: impl FnOnce() -> Option<Wanted>) -> Owed {
        if part != self.part {
            let in_flight = self.leave();
            *self = Self { part, ..Self::default() };
            if in_flight {
                return Owed::Close;
            }
        }
        let Some(view) = view else { return Owed::Nothing };
        match &self.stage {
            Stage::Settling => {
                if playing {
                    self.played_ms = self.played_ms.saturating_add(dt_ms);
                }
                if self.played_ms < SETTLE_MS || view.bazarr_host().is_empty() {
                    return Owed::Nothing;
                }
                let Some(Wanted { sid, rk, lang }) = wanted() else {
                    self.stage = Stage::Done;
                    return Owed::Nothing;
                };
                // a search the viewer already made on this item, from either source, stands
                if view.item().is_some_and(|item| same_item(item, (sid, &rk))) {
                    self.stage = Stage::Done;
                    return Owed::Nothing;
                }
                self.stage = Stage::InFlight { sid, rk: rk.clone() };
                Owed::Open(SubSearchCmd::OpenBazarr { sid, rk, lang })
            }
            Stage::InFlight { sid, rk } => {
                let ours = view.item().is_some_and(|item| same_item(item, (*sid, rk)));
                if ours && view.source() == SubSource::Bazarr && view.status() == SearchStatus::Searching {
                    return Owed::Pump;
                }
                self.stage = Stage::Done;
                Owed::Nothing
            }
            Stage::Done => Owed::Nothing,
        }
    }

    /// The screen is going: whether a search is still in flight, for the caller to drop.
    pub(super) fn leave(&mut self) -> bool {
        matches!(std::mem::take(&mut self.stage), Stage::InFlight { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::subsearch::{Landing, SearchFailure, SubSearchAdapter, SubSearchState};
    use plx_plex::plex::session::BazarrLink;

    const SID: ServerId = ServerId::from_raw(1);
    const PART: i64 = 7;

    fn linked() -> SubSearchState {
        let mut state = SubSearchState::default();
        state.set_link(&BazarrLink { url: "http://192.168.1.5:6767".into(), api_key: "k".into() });
        state
    }

    fn movie() -> Option<Wanted> {
        Some(Wanted { sid: SID, rk: "42".into(), lang: "en".into() })
    }

    #[test]
    fn a_settled_title_is_searched_once_and_pumped_until_it_lands() {
        let (mut state, adapter) = (linked(), SubSearchAdapter::default());
        let mut p = Prefetch::default();
        assert_eq!(p.step(PART, true, SETTLE_MS - 1, Some(state.view()), movie), Owed::Nothing, "not settled yet");
        assert_eq!(p.step(PART, false, 60_000, Some(state.view()), movie), Owed::Nothing, "paused time does not count");
        let Owed::Open(cmd) = p.step(PART, true, 1, Some(state.view()), movie) else { panic!("the search is due") };
        assert_eq!(cmd, SubSearchCmd::OpenBazarr { sid: SID, rk: "42".into(), lang: "en".into() });
        assert!(state.run(&adapter, cmd));
        assert_eq!(p.step(PART, true, 16, Some(state.view()), movie), Owed::Pump);
        state.apply_for_test(Landing::Bazarr(Err(SearchFailure::Unreachable)));
        assert_eq!(p.step(PART, true, 16, Some(state.view()), movie), Owed::Nothing);
        assert_eq!(p.step(PART, true, 60_000, Some(state.view()), movie), Owed::Nothing, "once per item");
        assert!(!p.leave(), "an answered search stays for the menu");
    }

    #[test]
    fn nothing_is_asked_without_bazarr_or_over_the_viewers_own_search() {
        let adapter = SubSearchAdapter::default();
        let unlinked = SubSearchState::default();
        assert_eq!(Prefetch::default().step(PART, true, SETTLE_MS, Some(unlinked.view()), movie), Owed::Nothing);
        assert_eq!(Prefetch::default().step(PART, true, SETTLE_MS, None, movie), Owed::Nothing, "a host with no search");
        let state = linked();
        assert_eq!(Prefetch::default().step(PART, true, SETTLE_MS, Some(state.view()), || None), Owed::Nothing,
            "not a title Bazarr manages");
        let mut state = linked();
        assert!(state.run(&adapter, SubSearchCmd::Open { sid: SID, rk: "42".into(), lang: "en".into() }));
        assert_eq!(Prefetch::default().step(PART, true, SETTLE_MS, Some(state.view()), movie), Owed::Nothing,
            "the viewer's server search stands");
    }

    #[test]
    fn a_search_in_flight_goes_with_its_item() {
        let (mut state, adapter) = (linked(), SubSearchAdapter::default());
        let mut p = Prefetch::default();
        let Owed::Open(cmd) = p.step(PART, true, SETTLE_MS, Some(state.view()), movie) else { panic!("due") };
        assert!(state.run(&adapter, cmd));
        assert_eq!(p.step(PART + 1, true, 16, Some(state.view()), movie), Owed::Close, "the next item drops it");
        assert!(state.run(&adapter, SubSearchCmd::Close));
        let Owed::Open(cmd) = p.step(PART + 1, true, SETTLE_MS, Some(state.view()), movie) else {
            panic!("the next item counts afresh")
        };
        assert!(state.run(&adapter, cmd));
        assert!(p.leave(), "leaving the player drops it too");
    }
}
