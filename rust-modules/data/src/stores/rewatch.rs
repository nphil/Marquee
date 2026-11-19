//! Continue Watching for a show watched again.
//!
//! `/hubs/continueWatching` lists a show by its next UNWATCHED episode, so once every episode after
//! the one just finished is already watched the show drops off the row. This module is the client
//! side of that gap: it reads what the profile watched in the last [`WINDOW_WEEKS`] weeks, takes the
//! show's most recent view (E) and, when Plex's own entry for the show does not cover it, offers the
//! episode after E (N) instead. The iPhone app implements the same rule.
//!
//! The rule is pure here ([`candidates`], [`next_episode`], [`keeps`], [`overlay`]); the reads that
//! feed it are [`read`] (Home) and [`show_next`] (a show page), both off the frame thread.
//! Everything is read-only: nothing is written to Plex.

use std::sync::Arc;

use plx_plex::plex::{Client, Metadata, SectionQuery, ServerId};

use crate::pms::{shelf_card, CwItem, PmsMovie};

/// How far back a view still counts as a show being watched.
pub(crate) const WINDOW_WEEKS: i64 = 16;
/// Shows whose next-episode lists are read in one pass.
pub(crate) const MAX_SHOWS: usize = 10;
const PAGE: i64 = 50;
const PAGES: i64 = 3;
/// Hides kept per profile; the oldest go first.
pub(crate) const MAX_HIDES: usize = 64;

/// One episode the profile viewed, as a library listing gives it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Viewed {
    pub(crate) sec: i64,
    pub(crate) show_rk: String,
    pub(crate) season: i64,
    pub(crate) index: i64,
    /// `viewOffset`: above 0 the episode is in progress and Plex's own entry covers it.
    pub(crate) offset: i64,
    pub(crate) at: i64,
}

/// What Plex's own entry for a show says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plex {
    pub(crate) resume_ms: i64,
    pub(crate) viewed_at: i64,
    pub(crate) unwatched: bool,
    pub(crate) at: (i64, i64),
}

impl Plex {
    pub(crate) fn of(c: &CwItem) -> Plex {
        Plex { resume_ms: c.m.resume_ms, viewed_at: c.last_viewed_at, unwatched: c.m.unwatched,
            at: (i64::from(c.m.season_index), i64::from(c.m.ep_index)) }
    }
}

/// Whether Plex's entry stands: it is in progress and newer than E, or it is unwatched and comes
/// at or after `first_after`, the earliest position past E.
pub(crate) fn keeps(p: &Plex, e_viewed_at: i64, first_after: (i64, i64)) -> bool {
    (p.resume_ms > 0 && p.viewed_at > e_viewed_at) || (p.unwatched && p.at >= first_after)
}

/// The shows that need their next episode looked up: each show's most recent view, newest first,
/// unless it is in progress or Plex's entry (`plex(show_rk)`) already covers it; at most `max`.
pub(crate) fn candidates(mut rows: Vec<Viewed>, plex: impl Fn(&str) -> Option<Plex>, max: usize) -> Vec<Viewed> {
    rows.sort_by(|a, b| b.at.cmp(&a.at));
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for e in rows {
        if seen.contains(&e.show_rk) { continue; }
        seen.push(e.show_rk.clone());
        let covered = e.offset > 0 || plex(&e.show_rk).is_some_and(|p| keeps(&p, e.at, (e.season, e.index + 1)));
        if !covered { out.push(e); }
        if out.len() == max { break; }
    }
    out
}

fn first_after<T>(eps: Vec<(i64, T)>, index: i64) -> Option<T> {
    eps.into_iter().filter(|(i, _)| *i > index).min_by_key(|(i, _)| *i).map(|(_, t)| t)
}

/// The episode after (`e_season`, `e_index`): the rest of its season, then the first episode of
/// each later season, never the specials (season 0). Seasons without episodes are passed over.
/// `seasons` are the season numbers the show has; `episodes_of(season)` reads one season's
/// `(episode number, episode)` pairs, and `Err` (a read that failed) stops the search, because
/// skipping a season that could not be read would name the wrong episode.
pub(crate) fn next_episode<T, E>(e_season: i64, e_index: i64, seasons: &[i64],
    mut episodes_of: impl FnMut(i64) -> Result<Vec<(i64, T)>, E>) -> Result<Option<T>, E> {
    if e_season > 0 {
        if let Some(t) = first_after(episodes_of(e_season)?, e_index) { return Ok(Some(t)); }
    }
    let mut later: Vec<i64> = seasons.iter().copied().filter(|s| *s > 0 && *s > e_season).collect();
    later.sort_unstable();
    later.dedup();
    for season in later {
        if let Some(t) = first_after(episodes_of(season)?, i64::MIN) { return Ok(Some(t)); }
    }
    Ok(None)
}

/// A next-episode entry, and whether the profile removed it from Continue Watching.
#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) item: CwItem,
    pub(crate) hidden: bool,
}

/// A show's next-episode entry the profile removed, valid until the show is viewed again.
#[derive(Clone, Debug, PartialEq)]
pub struct Hide {
    /// The portable show identity; the only one that outlives the session.
    pub guid: Option<[u8; 12]>,
    pub sid: Option<ServerId>,
    pub show_rk: String,
    /// `lastViewedAt` of the episode the entry followed.
    pub viewed_at: i64,
}

impl Hide {
    /// The hide for removing the next-episode entry `m`.
    pub fn of(m: &PmsMovie) -> Hide {
        Hide { guid: m.show_guid, sid: Some(m.sid), show_rk: m.show_rk.clone(), viewed_at: m.rewatch_e }
    }

    fn same_show(&self, other: &Hide) -> bool {
        match (self.guid, other.guid) {
            (Some(a), Some(b)) => a == b,
            _ => self.sid.is_some() && self.sid == other.sid && !self.show_rk.is_empty() && self.show_rk == other.show_rk,
        }
    }

    /// Adds `self` to `hides`, replacing the show's earlier hide and dropping the oldest past
    /// [`MAX_HIDES`].
    pub fn add_to(self, hides: &mut Vec<Hide>) {
        hides.retain(|h| !h.same_show(&self));
        hides.push(self);
        let extra = hides.len().saturating_sub(MAX_HIDES);
        hides.drain(..extra);
    }

    /// What outlives the session: the show's catalog id as 24 hex digits, and the view it followed.
    /// A show without one is hidden for this run only.
    pub fn stored(&self) -> Option<(String, i64)> {
        self.guid.map(|g| (g.iter().map(|b| format!("{b:02x}")).collect(), self.viewed_at))
    }

    /// [`Hide::stored`] read back; `None` for anything but 24 hex digits.
    pub fn from_stored(guid: &str, viewed_at: i64) -> Option<Hide> {
        if guid.len() != 24 || !guid.is_ascii() { return None; }
        let mut id = [0u8; 12];
        for (i, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&guid[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Hide { guid: Some(id), sid: None, show_rk: String::new(), viewed_at })
    }

    fn matches(&self, m: &PmsMovie) -> bool {
        match (self.guid, m.show_guid) {
            (Some(a), Some(b)) => a == b,
            _ => self.sid == Some(m.sid) && !self.show_rk.is_empty() && self.show_rk == m.show_rk,
        }
    }
}

/// Sets each entry's `hidden` from `hides`: a hide holds until the show has a newer view.
pub(crate) fn mark_hidden(entries: &mut [Entry], hides: &[Hide]) {
    for e in entries {
        e.hidden = hides.iter().any(|h| h.matches(&e.item.m) && h.viewed_at >= e.item.m.rewatch_e);
    }
}

fn same_show(a: &PmsMovie, b: &PmsMovie) -> bool {
    if a.kind != 3 || b.kind != 3 { return false; }
    match (a.show_guid, b.show_guid) {
        (Some(x), Some(y)) => x == y,
        _ => a.sid == b.sid && !a.show_rk.is_empty() && a.show_rk == b.show_rk,
    }
}

/// Puts the next-episode entries into the merged deck `cards`: for each show, its entry with the
/// newest E (one per show across servers) replaces Plex's card for the show unless Plex's stands
/// ([`keeps`]), and sits where its E's time puts it. A hidden entry adds nothing but still
/// replaces Plex's card, which it had superseded.
pub(crate) fn overlay<'a>(cards: &mut Vec<(&'a str, &'a CwItem)>, entries: &[(&'a str, &'a Entry)]) {
    let mut chosen: Vec<(&'a str, &'a Entry)> = Vec::new();
    for &(handle, e) in entries {
        match chosen.iter_mut().find(|(_, c)| same_show(&c.item.m, &e.item.m)) {
            Some(slot) => if e.item.m.rewatch_e > slot.1.item.m.rewatch_e { *slot = (handle, e) },
            None => chosen.push((handle, e)),
        }
    }
    for (handle, e) in chosen {
        let n = &e.item;
        let plex = cards.iter().position(|(_, c)| same_show(&c.m, &n.m));
        let first_after = (i64::from(n.m.season_index), i64::from(n.m.ep_index));
        if plex.is_some_and(|i| keeps(&Plex::of(cards[i].1), n.m.rewatch_e, first_after)) { continue; }
        if let Some(i) = plex { cards.remove(i); }
        if e.hidden { continue; }
        let at = cards.iter().position(|(_, c)| c.last_viewed_at < n.last_viewed_at).unwrap_or(cards.len());
        cards.insert(at, (handle, n));
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The profile's episodes viewed since `since` in one TV library, newest first.
fn viewed_in(client: &Client, sec: i64, since: i64) -> Option<Vec<Viewed>> {
    let filters = [("type".to_string(), "4".to_string()), ("lastViewedAt>".to_string(), since.to_string())];
    let mut out = Vec::new();
    for page in 0..PAGES {
        let list = client.section_items_query(&SectionQuery { section_key: sec, sort: "lastViewedAt:desc",
            filters: &filters, start: page * PAGE, size: PAGE, include_meta: false })?;
        let n = list.metadata.len() as i64;
        out.extend(list.metadata.iter().filter(|m| m.kind == "episode" && !m.grandparent_rating_key.is_empty())
            .map(|m| Viewed { sec, show_rk: m.grandparent_rating_key.clone(), season: m.parent_index.unwrap_or(0),
                index: m.index, offset: m.view_offset, at: m.last_viewed_at }));
        if n < PAGE { break; }
    }
    Some(out)
}

/// The episode after (`season`, `index`) of the show `show_rk`, as the server lists it. `Err` is a
/// read that failed; `Ok(None)` is a series that has ended.
fn next_of(client: &Client, show_rk: &str, season: i64, index: i64) -> Result<Option<Metadata>, ()> {
    let seasons: Vec<(i64, String)> = client.children(show_rk).ok_or(())?.metadata.iter()
        .filter(|m| m.kind == "season").map(|m| (m.index, m.rating_key.clone())).collect();
    let numbers: Vec<i64> = seasons.iter().map(|(n, _)| *n).collect();
    next_episode(season, index, &numbers, |number| {
        let Some((_, rk)) = seasons.iter().find(|(n, _)| *n == number) else { return Ok(Vec::new()) };
        let list = client.children(rk).ok_or(())?;
        Ok(list.metadata.into_iter().filter(|m| m.kind == "episode").map(|m| (m.index, m)).collect())
    })
}

/// The entry for `e`: the episode after it, shown as a next-episode card that plays from the start.
fn entry_for(client: &Client, sid: ServerId, e: &Viewed) -> Option<CwItem> {
    let next = next_of(client, &e.show_rk, e.season, e.index).ok()??;
    let mut m = shelf_card(&next, sid)?;
    m.sec = e.sec;
    m.resume_ms = 0;
    m.rewatch_e = e.at;
    Some(CwItem { last_viewed_at: e.at, m: Arc::new(m), position: 0 })
}

/// What Home's read of one server brings: the next-episode entries for the shows the profile
/// watched lately, `known` being the server's own Continue Watching cards. `None` is a failed read.
pub(crate) fn read(client: &'static Client, sid: ServerId, hidden: &[i64], known: &[CwItem]) -> Option<Vec<CwItem>> {
    let since = now_secs() - WINDOW_WEEKS * 7 * 86_400;
    let mut rows = Vec::new();
    for dir in client.sections()?.directory.iter().filter(|d| d.kind == "show") {
        let Ok(sec) = dir.key.parse::<i64>() else { continue };
        if hidden.contains(&sec) { continue; }
        rows.extend(viewed_in(client, sec, since)?);
    }
    let plex = |show: &str| known.iter().find(|c| c.m.kind == 3 && c.m.show_rk == show).map(Plex::of);
    let pending = candidates(rows, plex, MAX_SHOWS);
    std::thread::scope(|s| {
        let jobs: Vec<_> = pending.iter()
            .map(|e| plx_base::task::beside(s, "rewatch-next", move || entry_for(client, sid, e))).collect();
        Some(jobs.into_iter().filter_map(|job| job.finish()).collect())
    })
}

/// A show page's next episode for a show watched to the end that the server has nothing on deck
/// for: the episode after the show's most recently viewed one, when that one is not part-way.
pub(crate) fn show_next(client: &Client, section: i64, show_rk: &str) -> Option<Metadata> {
    let filters = [("type".to_string(), "4".to_string()), ("show.id".to_string(), show_rk.to_string())];
    let list = client.section_items_query(&SectionQuery { section_key: section, sort: "lastViewedAt:desc",
        filters: &filters, start: 0, size: 1, include_meta: false })?;
    let e = list.metadata.first().filter(|m| m.view_count > 0 && m.view_offset == 0)?;
    next_of(client, show_rk, e.parent_index.unwrap_or(0), e.index).ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: [u8; 12] = [7; 12];

    fn viewed(show: &str, season: i64, index: i64, at: i64, offset: i64) -> Viewed {
        Viewed { sec: 2, show_rk: show.into(), season, index, offset, at }
    }

    fn episode(rk: &str, season: i32, index: i32) -> PmsMovie {
        PmsMovie { sid: ServerId::from_raw(1), rk: rk.into(), kind: 3, show_rk: "100".into(), show_guid: Some(SHOW),
            season_index: season, ep_index: index, ..PmsMovie::default() }
    }

    /// Plex's own entry: `resume` ms in, or a next episode nobody has watched.
    fn plex_card(rk: &str, season: i32, index: i32, resume: i64, unwatched: bool, at: i64) -> CwItem {
        CwItem { last_viewed_at: at, position: 0,
            m: Arc::new(PmsMovie { resume_ms: resume, unwatched, ..episode(rk, season, index) }) }
    }

    fn next_entry(rk: &str, season: i32, index: i32, e_at: i64, hidden: bool) -> Entry {
        let m = PmsMovie { rewatch_e: e_at, ..episode(rk, season, index) };
        Entry { item: CwItem { last_viewed_at: e_at, m: Arc::new(m), position: 0 }, hidden }
    }

    fn movie(rk: &str, at: i64) -> CwItem {
        CwItem { last_viewed_at: at, position: 0, m: Arc::new(PmsMovie { rk: rk.into(), ..PmsMovie::default() }) }
    }

    fn after(seasons: &[(i64, Vec<i64>)], e: (i64, i64)) -> Option<(i64, i64)> {
        let numbers: Vec<i64> = seasons.iter().map(|(n, _)| *n).collect();
        next_episode::<(i64, i64), ()>(e.0, e.1, &numbers, |n| {
            Ok(seasons.iter().find(|(s, _)| *s == n).map(|(s, eps)| eps.iter().map(|i| (*i, (*s, *i))).collect()).unwrap_or_default())
        }).unwrap()
    }

    fn rks<'a>(cards: &[(&'a str, &'a CwItem)]) -> Vec<&'a str> { cards.iter().map(|(_, c)| c.m.rk.as_str()).collect() }

    #[test]
    fn a_finished_episode_of_a_rewatched_show_names_the_next_one() {
        // Community: S3E1 finished last, nothing in progress, Plex has no entry.
        let rows = vec![viewed("100", 2, 7, 50, 0), viewed("100", 3, 1, 90, 0), viewed("100", 3, 1, 10, 0)];
        let got = candidates(rows, |_| None, MAX_SHOWS);
        assert_eq!(got, vec![viewed("100", 3, 1, 90, 0)]);
        let seasons = [(1, (1..=25).collect()), (2, (1..=24).collect()), (3, (1..=22).collect())];
        assert_eq!(after(&seasons, (3, 1)), Some((3, 2)));
    }

    #[test]
    fn the_next_episode_crosses_seasons_and_skips_the_specials() {
        let seasons = [(0, vec![1, 2]), (1, vec![1, 2, 3]), (2, vec![]), (3, vec![1, 2])];
        assert_eq!(after(&seasons, (1, 3)), Some((3, 1)), "past the last episode, an empty season is passed over");
        assert_eq!(after(&seasons, (1, 1)), Some((1, 2)));
        assert_eq!(after(&seasons, (0, 2)), Some((1, 1)), "a special leads to the first regular episode");
        assert_eq!(after(&[(0, vec![1, 2]), (1, vec![1])], (1, 1)), None, "specials are never the next episode");
    }

    #[test]
    fn a_series_that_ended_has_no_next_episode() {
        assert_eq!(after(&[(1, vec![1, 2])], (1, 2)), None);
    }

    #[test]
    fn a_season_that_cannot_be_read_does_not_hide_the_next_one() {
        let got = next_episode::<i64, ()>(1, 2, &[1, 2], |n| if n == 1 { Ok(vec![(1, 1), (2, 2)]) } else { Err(()) });
        assert_eq!(got, Err(()));
    }

    #[test]
    fn only_a_show_nothing_covers_needs_a_lookup() {
        let in_progress = viewed("a", 1, 4, 80, 5_000);
        let covered = viewed("b", 1, 4, 70, 0);
        let bare = viewed("c", 1, 4, 60, 0);
        let plex = |show: &str| (show == "b").then_some(Plex { resume_ms: 0, viewed_at: 70, unwatched: true, at: (1, 5) });
        let got = candidates(vec![bare.clone(), covered, in_progress], plex, MAX_SHOWS);
        assert_eq!(got, vec![bare], "an episode in progress and a show Plex lists for its next episode are skipped");
        let many: Vec<Viewed> = (0..25).map(|i| viewed(&i.to_string(), 1, 1, 100 - i, 0)).collect();
        assert_eq!(candidates(many, |_| None, MAX_SHOWS).len(), MAX_SHOWS);
    }

    #[test]
    fn plex_keeps_its_entry_when_it_is_newer_progress_or_a_later_unwatched_episode() {
        let progress = |at| Plex { resume_ms: 9, viewed_at: at, unwatched: false, at: (1, 1) };
        assert!(keeps(&progress(100), 50, (1, 2)));
        assert!(!keeps(&progress(40), 50, (1, 2)));
        let unwatched = |at| Plex { resume_ms: 0, viewed_at: 0, unwatched: true, at };
        assert!(keeps(&unwatched((1, 2)), 50, (1, 2)));
        assert!(!keeps(&unwatched((1, 1)), 50, (1, 2)));
    }

    #[test]
    fn a_show_with_no_plex_entry_gets_the_next_episode_in_time_order() {
        let (a, b) = (movie("a", 200), movie("b", 20));
        let n = next_entry("n", 3, 2, 90, false);
        let mut cards = vec![("", &a), ("", &b)];
        overlay(&mut cards, &[("", &n)]);
        assert_eq!(rks(&cards), ["a", "n", "b"]);
        assert_eq!(cards[1].1.m.resume_ms, 0);
    }

    #[test]
    fn plexs_entry_stays_when_in_progress_and_newer_and_goes_when_older() {
        let n = next_entry("n", 3, 2, 90, false);
        let newer = plex_card("p", 3, 4, 8_000, false, 120);
        let mut cards = vec![("", &newer)];
        overlay(&mut cards, &[("", &n)]);
        assert_eq!(rks(&cards), ["p"]);

        let older = plex_card("p", 3, 4, 8_000, false, 60);
        let mut cards = vec![("", &older)];
        overlay(&mut cards, &[("", &n)]);
        assert_eq!(rks(&cards), ["n"], "one entry per show");
    }

    #[test]
    fn plexs_unwatched_next_episode_stays_when_it_comes_after_e() {
        let n = next_entry("n", 3, 2, 90, false);
        let later = plex_card("p", 3, 2, 0, true, 30);
        let mut cards = vec![("", &later)];
        overlay(&mut cards, &[("", &n)]);
        assert_eq!(rks(&cards), ["p"]);

        let earlier = plex_card("p", 1, 9, 0, true, 30);
        let mut cards = vec![("", &earlier)];
        overlay(&mut cards, &[("", &n)]);
        assert_eq!(rks(&cards), ["n"]);
    }

    #[test]
    fn two_servers_with_the_same_show_give_one_entry_the_newest_view() {
        let mut other = next_entry("m", 3, 9, 95, false);
        let mut m = (*other.item.m).clone();
        m.sid = ServerId::from_raw(2);
        other.item.m = Arc::new(m);
        let n = next_entry("n", 3, 2, 90, false);
        let mut cards: Vec<(&str, &CwItem)> = Vec::new();
        overlay(&mut cards, &[("a", &n), ("b", &other)]);
        assert_eq!(rks(&cards), ["m"]);
    }

    #[test]
    fn a_removed_entry_stays_hidden_until_the_show_is_viewed_again() {
        let mut entries = vec![next_entry("n", 3, 2, 90, false)];
        let hide = Hide::of(&entries[0].item.m);
        mark_hidden(&mut entries, std::slice::from_ref(&hide));
        assert!(entries[0].hidden);

        let old = plex_card("p", 3, 4, 8_000, false, 60);
        let mut cards = vec![("", &old)];
        overlay(&mut cards, &[("", &entries[0])]);
        assert!(cards.is_empty(), "the hidden entry still supersedes Plex's older card");

        let mut viewed_again = vec![next_entry("n2", 3, 3, 130, false)];
        mark_hidden(&mut viewed_again, std::slice::from_ref(&hide));
        assert!(!viewed_again[0].hidden, "a newer view lifts the hide");
    }

    #[test]
    fn hides_keep_one_per_show_and_the_newest_few() {
        let mut hides = Vec::new();
        let hide = |n: u8, at| Hide { guid: Some([n; 12]), sid: None, show_rk: String::new(), viewed_at: at };
        hide(1, 10).add_to(&mut hides);
        hide(1, 20).add_to(&mut hides);
        assert_eq!(hides, vec![hide(1, 20)]);
        for n in 2..=(MAX_HIDES as u8 + 5) { hide(n, 1).add_to(&mut hides); }
        assert_eq!(hides.len(), MAX_HIDES);
        assert!(!hides.contains(&hide(1, 20)), "the oldest went first");
    }

    #[test]
    fn a_hide_that_outlived_the_session_matches_by_the_shows_guid() {
        let mut entries = vec![next_entry("n", 3, 2, 90, false)];
        let hide = Hide { guid: Some(SHOW), sid: None, show_rk: String::new(), viewed_at: 90 };
        mark_hidden(&mut entries, &[hide]);
        assert!(entries[0].hidden);
    }
}
