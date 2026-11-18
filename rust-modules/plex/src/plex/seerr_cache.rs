//! **What the Requests page shows before Seerr has answered.**
//!
//! Opening Requests used to wait on Seerr: a sign-in check, then the viewer's requests (one call for
//! the list and one more for every title in it) and the four discover shelves, one after another,
//! 1 to 3 seconds in all. The page now opens on a [`Snapshot`] of the last answers instead (held in
//! memory, kept on disk across a restart) and reads Seerr again behind it, so the rows are there on
//! the first frame and change in place only when Seerr says something different
//! (stale-while-revalidate).
//!
//! * A snapshot is **fresh** for [`FRESH_SECS`]: opening again inside that asks Seerr nothing. Past
//!   it the snapshot still shows and a refresh runs behind it. Past [`KEEP_SECS`] it is not shown.
//! * It belongs to one Seerr address and one Plex account ([`Key`]); another profile or address
//!   never sees it.
//! * A request made from the app, or found already made, marks the title requested in every row
//!   at once ([`Hub::requested`]) and makes the snapshot stale, so a poster never says "nothing"
//!   for a title that has been asked for.
//! * The refresh ([`refresh`]) reads the shelves and the viewer's requests beside each other, and
//!   a title already known from an earlier read is not asked for again, so a repeat refresh is
//!   five calls instead of twenty-five.
//!
//! The [`Hub`] holds the one snapshot of the process and the pages listening to it ([`Sink`]); it
//! takes the time as an argument so tests need no clock.

use super::seerr::{Asked, Badge, Have, Kind, Page, Rules, Seerr, SeerrError, Shelf, Title, Transport};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// How long a snapshot is served without asking Seerr again.
pub const FRESH_SECS: u64 = 120;
/// How long a snapshot may be shown at all; an older one is dropped rather than shown.
pub const KEEP_SECS: u64 = 14 * 24 * 60 * 60;
/// The file format, so a later change can refuse an older file.
const FORMAT: u32 = 1;
/// Titles read at once when the viewer's requests name some this device has not seen.
const TITLES_AT_ONCE: usize = 3;

/// What a snapshot belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub base: String,
    pub account: String,
}

/// The viewer's own requests, first page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mine {
    pub items: Vec<Asked>,
    pub more: bool,
}

/// One row of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Mine,
    Shelf(Shelf),
}

/// One answer, as a page takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Ready(Rules),
    Mine(Mine),
    Shelf(Shelf, Page<Title>),
    /// This row could not be read and nothing is held for it.
    Lost(Part, SeerrError),
    /// Seerr could not be reached at all and nothing is held.
    Failed(SeerrError),
}

/// A page listening to the hub.
pub trait Sink: Send {
    /// Take one answer; `false` when nobody listens any more. Never calls back into the hub.
    fn take(&self, piece: Piece) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    format: u32,
    base: String,
    account: String,
    /// Unix seconds when the last complete refresh finished.
    read: u64,
    /// Something changed since (a request was made): ask again whatever the age.
    #[serde(default)]
    stale: bool,
    rules: Option<Rules>,
    mine: Option<Mine>,
    shelves: [Option<Page<Title>>; 4],
}

impl Snapshot {
    fn empty(key: &Key) -> Self {
        Self {
            format: FORMAT,
            base: key.base.clone(),
            account: key.account.clone(),
            read: 0,
            stale: true,
            rules: None,
            mine: None,
            shelves: [None, None, None, None],
        }
    }

    fn is_for(&self, key: &Key) -> bool {
        self.base == key.base && self.account == key.account
    }

    /// Seconds since the last complete read; a clock set back since counts as infinitely old.
    fn age(&self, now: u64) -> u64 {
        now.checked_sub(self.read).unwrap_or(u64::MAX)
    }

    fn complete(&self) -> bool {
        self.rules.is_some() && self.mine.is_some() && self.shelves.iter().all(Option::is_some)
    }

    /// Nothing needs asking: whole, read lately, and nothing has happened since.
    pub fn fresh(&self, now: u64) -> bool {
        !self.stale && self.complete() && self.age(now) < FRESH_SECS
    }

    /// Young enough to show.
    pub fn keeps(&self, now: u64) -> bool {
        self.age(now) <= KEEP_SECS
    }

    /// A title this snapshot already knows, from the viewer's requests first.
    pub fn title(&self, kind: Kind, tmdb: u32) -> Option<&Title> {
        let same = |t: &&Title| t.kind == kind && t.tmdb == tmdb;
        let mine = self.mine.iter().flat_map(|m| m.items.iter().map(|a| &a.title));
        let shelves = self.shelves.iter().flatten().flat_map(|p| p.items.iter());
        mine.chain(shelves).find(same)
    }

    /// Every held answer, in the order a page wants them.
    fn pieces(&self) -> Vec<Piece> {
        let mut out = Vec::with_capacity(6);
        out.extend(self.rules.map(Piece::Ready));
        out.extend(self.mine.clone().map(Piece::Mine));
        for (shelf, page) in Shelf::ALL.into_iter().zip(&self.shelves) {
            out.extend(page.clone().map(|page| Piece::Shelf(shelf, page)));
        }
        out
    }

    fn apply(&mut self, piece: &Piece) {
        match piece {
            Piece::Ready(rules) => self.rules = Some(*rules),
            Piece::Mine(mine) => self.mine = Some(mine.clone()),
            Piece::Shelf(shelf, page) => self.shelves[shelf.index()] = Some(page.clone()),
            Piece::Lost(..) | Piece::Failed(_) => {}
        }
    }

    fn holds(&self, part: Part) -> bool {
        match part {
            Part::Mine => self.mine.is_some(),
            Part::Shelf(shelf) => self.shelves[shelf.index()].is_some(),
        }
    }

    /// The title is asked for now: every row that shows it as unknown shows it requested, and
    /// the viewer's requests gain it. Returns the rows that changed.
    fn mark_requested(&mut self, kind: Kind, tmdb: u32) -> Vec<Piece> {
        let mut changed = Vec::new();
        for (shelf, slot) in Shelf::ALL.into_iter().zip(&mut self.shelves) {
            let Some(page) = slot else { continue };
            let mut hit = false;
            for t in page.items.iter_mut().filter(|t| t.kind == kind && t.tmdb == tmdb && t.state.have == Have::Nothing) {
                t.state.have = Have::Pending;
                hit = true;
            }
            if hit {
                changed.push(Piece::Shelf(shelf, page.clone()));
            }
        }
        let known = self.title(kind, tmdb).cloned();
        if let (Some(mine), Some(title)) = (&mut self.mine, known) {
            if !mine.items.iter().any(|a| a.title.kind == kind && a.title.tmdb == tmdb) {
                mine.items.insert(0, Asked { title, badge: Some(Badge::Requested) });
                changed.push(Piece::Mine(mine.clone()));
            }
        }
        self.stale = true;
        changed
    }
}

// ---- the hub -----------------------------------------------------------------------------------------

struct Inner {
    key: Option<Key>,
    snap: Option<Snapshot>,
    /// The file has been looked at for this key.
    disk_read: bool,
    /// A worker is reading or refreshing.
    running: bool,
    /// Asked again while the worker ran: go round once more.
    again: bool,
    /// Moves whenever the snapshot changes.
    gen: u64,
    sinks: Vec<Box<dyn Sink>>,
}

/// The one snapshot of the process and the pages listening to it.
pub struct Hub {
    inner: Mutex<Inner>,
}

/// Whether the caller must start the worker that reads the file and refreshes ([`sync`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Idle,
    Start,
}

/// What a worker does after a pass.
pub struct Finish {
    /// The snapshot to write to disk: set after a pass that read everything.
    pub save: Option<Snapshot>,
    /// A request or a profile change came in during the pass: run it again.
    pub again: bool,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub const fn new() -> Self {
        Self { inner: Mutex::new(Inner { key: None, snap: None, disk_read: false, running: false, again: false, gen: 0, sinks: Vec::new() }) }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A page opens (`sink`) or the app wants the answers ready (`None`). A listener is handed what
    /// is held at once. `Start` means no worker is running and one is needed: the snapshot is
    /// stale, or the file has not been read.
    pub fn open(&self, key: &Key, sink: Option<Box<dyn Sink>>, now: u64) -> Work {
        let mut g = self.lock();
        if g.key.as_ref() != Some(key) {
            g.key = Some(key.clone());
            g.snap = None;
            g.disk_read = false;
            g.sinks.clear();
            g.gen += 1;
            g.again |= g.running;
        }
        if let Some(sink) = sink {
            let live = g.snap.as_ref().is_none_or(|s| s.pieces().into_iter().all(|piece| sink.take(piece)));
            if live {
                g.sinks.push(sink);
            }
        }
        let stale = g.snap.as_ref().is_none_or(|s| !s.fresh(now));
        if g.running || !(stale || !g.disk_read) {
            return Work::Idle;
        }
        g.running = true;
        Work::Start
    }

    /// The address and account the hub is working for.
    pub fn key(&self) -> Option<Key> {
        self.lock().key.clone()
    }

    /// The file has been read: what it held (if it is for this key and young enough) is shown
    /// unless something newer is already here.
    pub fn disk_read(&self, found: Option<Snapshot>, now: u64) {
        let mut g = self.lock();
        g.disk_read = true;
        let Some(snap) = found else { return };
        let usable = g.snap.is_none() && g.key.as_ref().is_some_and(|k| snap.is_for(k)) && snap.keeps(now);
        if !usable {
            return;
        }
        for piece in snap.pieces() {
            g.sinks.retain(|s| s.take(piece.clone()));
        }
        g.snap = Some(snap);
        g.gen += 1;
    }

    /// Whether the held snapshot needs reading again.
    pub fn wants_refresh(&self, now: u64) -> bool {
        self.lock().snap.as_ref().is_none_or(|s| !s.fresh(now))
    }

    /// The file has not been looked at for this key yet.
    pub fn disk_unread(&self) -> bool {
        !self.lock().disk_read
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.lock().snap.clone()
    }

    /// Moves whenever the snapshot changes.
    pub fn generation(&self) -> u64 {
        self.lock().gen
    }

    /// One answer of a refresh: held and handed to every listener.
    pub fn publish(&self, key: &Key, piece: Piece) {
        let mut g = self.lock();
        if g.key.as_ref() != Some(key) {
            return;
        }
        g.snap.get_or_insert_with(|| Snapshot::empty(key)).apply(&piece);
        g.gen += 1;
        g.sinks.retain(|s| s.take(piece.clone()));
    }

    /// A row could not be read: its listeners hear of it only when nothing is held to show.
    pub fn lost(&self, key: &Key, part: Part, error: SeerrError) {
        let mut g = self.lock();
        if g.key.as_ref() != Some(key) || g.snap.as_ref().is_some_and(|s| s.holds(part)) {
            return;
        }
        g.sinks.retain(|s| s.take(Piece::Lost(part, error)));
    }

    /// Seerr could not be reached at all: listeners hear of it only when nothing is held to show.
    pub fn failed(&self, key: &Key, error: SeerrError) {
        let mut g = self.lock();
        if g.key.as_ref() != Some(key) || g.snap.as_ref().is_some_and(|s| s.rules.is_some()) {
            return;
        }
        g.sinks.retain(|s| s.take(Piece::Failed(error)));
    }

    /// The title was asked for, or Seerr says it already was: show it so everywhere now, and read
    /// Seerr again behind it.
    pub fn requested(&self, kind: Kind, tmdb: u32) {
        let mut g = self.lock();
        g.again |= g.running;
        let Some(snap) = g.snap.as_mut() else { return };
        let changed = snap.mark_requested(kind, tmdb);
        g.gen += 1;
        for piece in changed {
            g.sinks.retain(|s| s.take(piece.clone()));
        }
    }

    /// A pass ended. `ok` is whether every row was read; the snapshot is then fresh.
    pub fn finish(&self, key: &Key, ok: bool, now: u64) -> Finish {
        let mut g = self.lock();
        let mut save = None;
        if ok && g.key.as_ref() == Some(key) {
            if let Some(snap) = g.snap.as_mut() {
                snap.read = now;
                snap.stale = false;
                save = Some(snap.clone());
            }
        }
        let again = std::mem::take(&mut g.again);
        g.running = again;
        Finish { save, again }
    }

    /// The worker could not start: nothing is running.
    pub fn abandon(&self) {
        let mut g = self.lock();
        g.running = false;
        g.again = false;
    }

    /// Forget everything held (the link was forgotten or moved).
    pub fn clear(&self) {
        let mut g = self.lock();
        g.key = None;
        g.snap = None;
        g.disk_read = false;
        g.sinks.clear();
        g.gen += 1;
    }

    /// The poster paths a page shows first: the first `per_row` of each of the first `rows` rows.
    pub fn first_posters(&self, rows: usize, per_row: usize) -> Vec<String> {
        let g = self.lock();
        let Some(snap) = g.snap.as_ref() else { return Vec::new() };
        let mine = snap.mine.iter().map(|m| m.items.iter().map(|a| &a.title).collect::<Vec<_>>());
        let shelves = snap.shelves.iter().flatten().map(|p| p.items.iter().collect::<Vec<_>>());
        let titled = mine.chain(shelves).filter(|row| !row.is_empty());
        titled
            .take(rows)
            .flat_map(|row| row.into_iter().take(per_row).map(|t| super::seerr::tmdb_image(&t.poster, "w342")))
            .filter(|url| !url.is_empty())
            .collect()
    }
}

/// Seconds since 1970: the clock the held answers are aged by.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ---- the file ----------------------------------------------------------------------------------------

/// The snapshot a file holds, from the first of `paths` that holds one.
pub fn load(paths: &[PathBuf]) -> Option<Snapshot> {
    paths.iter().find_map(|path| {
        let snap: Snapshot = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (snap.format == FORMAT).then_some(snap)
    })
}

/// Write the snapshot to the first of `paths` that takes it. Owner-only: it lists what the viewer
/// asked for.
pub fn save(paths: &[PathBuf], snap: &Snapshot) -> bool {
    let Ok(json) = serde_json::to_vec(snap) else { return false };
    paths.iter().any(|path| write_private(path, &json).is_ok())
}

/// Remove every file of `paths`.
pub fn purge(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    let written = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    match written.and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error)
        }
    }
}

// ---- the reads ---------------------------------------------------------------------------------------

/// One page of the viewer's requests, each with its title. A title `known` already is not asked for
/// again; the others are read a few at a time. A title Seerr cannot read any more is left out
/// rather than failing the row.
pub fn mine_page<T: Transport + Sync>(seerr: &Seerr<'_, T>, skip: u32, known: Option<&Snapshot>) -> Result<Mine, SeerrError> {
    let me = seerr.me()?;
    let (requests, more) = seerr.my_requests(me.id, skip)?;
    let mut titles: Vec<Option<Title>> =
        requests.iter().map(|r| known.and_then(|s| s.title(r.kind, r.tmdb)).cloned()).collect();
    let missing: Vec<usize> = (0..titles.len()).filter(|&i| titles[i].is_none()).collect();
    for chunk in missing.chunks(TITLES_AT_ONCE) {
        std::thread::scope(|scope| {
            let reads: Vec<_> = chunk
                .iter()
                .map(|&i| {
                    let (one, kind, tmdb) = (seerr.sibling(), requests[i].kind, requests[i].tmdb);
                    (i, plx_base::task::beside(scope, "seerr-title", move || one.detail(kind, tmdb).ok().map(|d| d.title)))
                })
                .collect();
            for (i, read) in reads {
                titles[i] = read.finish();
            }
        });
    }
    let items = requests
        .iter()
        .zip(titles)
        .filter_map(|(request, title)| Some(Asked { title: title?, badge: request.badge.or(Some(Badge::Requested)) }))
        .collect();
    Ok(Mine { items, more })
}

/// What a refresh signs in with.
pub struct Login<'a> {
    pub base: &'a str,
    pub cookie: &'a str,
    pub plex_token: &'a str,
}

/// Read what the page opens with, each row beside the others, handing each to the hub as it lands.
/// Returns whether every row was read, and the session to keep.
pub fn refresh<T: Transport + Sync>(hub: &Hub, key: &Key, login: &Login<'_>, http: &T) -> (bool, String) {
    let seerr = Seerr::new(login.base, login.cookie, login.plex_token, http);
    // The first call signs in when the session is gone, so the rows below start from a good one.
    let rules = match seerr.rules() {
        Ok(rules) => rules,
        Err(error) => {
            hub.failed(key, error);
            return (false, seerr.cookie());
        }
    };
    hub.publish(key, Piece::Ready(rules));
    let prior = hub.snapshot();
    let prior = prior.as_ref();
    let land = |part: Part, read: Result<Piece, SeerrError>| match read {
        Ok(piece) => {
            hub.publish(key, piece);
            true
        }
        Err(error) => {
            hub.lost(key, part, error);
            false
        }
    };
    let ok = std::thread::scope(|scope| {
        let land = &land;
        let one = seerr.sibling();
        let mine = plx_base::task::beside(scope, "seerr-mine", move || {
            land(Part::Mine, mine_page(&one, 0, prior).map(Piece::Mine))
        });
        let shelves: Vec<_> = Shelf::ALL
            .into_iter()
            .map(|shelf| {
                let one = seerr.sibling();
                plx_base::task::beside(scope, "seerr-shelf", move || {
                    land(Part::Shelf(shelf), one.discover(shelf, 1).map(|page| Piece::Shelf(shelf, page)))
                })
            })
            .collect();
        let mut ok = mine.finish();
        for shelf in shelves {
            ok &= shelf.finish();
        }
        ok
    });
    (ok, seerr.cookie())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::seerr::{MediaState, Reply, Request, TransportError, Verb};
    use std::sync::Arc;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../../tests/fixtures/seerr/", $name))
        };
    }

    const NOW: u64 = 1_000_000;

    fn key() -> Key {
        Key { base: "https://requests.example.net".into(), account: "u1".into() }
    }

    fn title(kind: Kind, tmdb: u32, have: Have) -> Title {
        Title {
            kind,
            tmdb,
            name: format!("Title {tmdb}"),
            year: "2026".into(),
            poster: format!("/p{tmdb}.jpg"),
            backdrop: String::new(),
            overview: String::new(),
            state: MediaState { have, downloading: false },
        }
    }

    fn page(titles: Vec<Title>) -> Page<Title> {
        Page { items: titles, page: 1, pages: 3 }
    }

    /// Records every piece it is given.
    #[derive(Clone, Default)]
    struct Ear(Arc<Mutex<Vec<Piece>>>);

    impl Sink for Ear {
        fn take(&self, piece: Piece) -> bool {
            self.0.lock().unwrap().push(piece);
            true
        }
    }

    impl Ear {
        fn heard(&self) -> Vec<Piece> {
            self.0.lock().unwrap().clone()
        }
    }

    /// A complete snapshot, read at `NOW`.
    fn whole() -> Snapshot {
        let mut snap = Snapshot::empty(&key());
        snap.apply(&Piece::Ready(Rules { partial: true, specials: false }));
        snap.apply(&Piece::Mine(Mine { items: vec![Asked { title: title(Kind::Movie, 1, Have::Pending), badge: Some(Badge::Requested) }], more: false }));
        for shelf in Shelf::ALL {
            snap.apply(&Piece::Shelf(shelf, page(vec![title(Kind::Movie, 10 + shelf.index() as u32, Have::Nothing)])));
        }
        snap.read = NOW;
        snap.stale = false;
        snap
    }

    #[test]
    fn a_snapshot_is_fresh_for_the_ttl_then_stale_but_shown_until_it_is_old() {
        let snap = whole();
        assert!(snap.fresh(NOW) && snap.fresh(NOW + FRESH_SECS - 1));
        assert!(!snap.fresh(NOW + FRESH_SECS), "the ttl is up");
        assert!(snap.keeps(NOW + FRESH_SECS) && snap.keeps(NOW + KEEP_SECS));
        assert!(!snap.keeps(NOW + KEEP_SECS + 1), "too old to show");
        assert!(!snap.fresh(NOW - 1) && !snap.keeps(NOW - 1), "a clock set back since is not trusted");
        let mut partial = whole();
        partial.mine = None;
        assert!(!partial.fresh(NOW), "a snapshot missing a row is never fresh");
    }

    #[test]
    fn opening_hands_a_listener_what_is_held_and_starts_work_only_when_it_is_needed() {
        let hub = Hub::new();
        let ear = Ear::default();
        assert_eq!(hub.open(&key(), Some(Box::new(ear.clone())), NOW), Work::Start, "nothing held, the file unread");
        assert!(ear.heard().is_empty());
        hub.disk_read(Some(whole()), NOW);
        assert_eq!(ear.heard().len(), 6, "the rules, the viewer's requests and four shelves");
        assert!(matches!(ear.heard()[0], Piece::Ready(_)));
        assert_eq!(hub.finish(&key(), true, NOW).again, false);

        let second = Ear::default();
        assert_eq!(hub.open(&key(), Some(Box::new(second.clone())), NOW + 5), Work::Idle, "fresh: nothing to ask");
        assert_eq!(second.heard().len(), 6, "held rows are on the first frame");
        assert_eq!(hub.open(&key(), None, NOW + FRESH_SECS), Work::Start, "stale: asked again behind what is shown");
        assert_eq!(hub.open(&key(), None, NOW + FRESH_SECS), Work::Idle, "one worker at a time");
    }

    #[test]
    fn a_snapshot_of_another_account_or_address_is_never_shown() {
        let hub = Hub::new();
        let ear = Ear::default();
        hub.open(&key(), Some(Box::new(ear.clone())), NOW);
        let mut other = whole();
        other.account = "u2".into();
        hub.disk_read(Some(other), NOW);
        assert!(ear.heard().is_empty() && hub.snapshot().is_none());

        let later = Key { account: "u2".into(), ..key() };
        let second = Ear::default();
        hub.finish(&key(), false, NOW);
        assert_eq!(hub.open(&later, Some(Box::new(second.clone())), NOW), Work::Start, "a new profile reads its own");
        hub.disk_read(Some(whole()), NOW);
        assert!(second.heard().is_empty(), "the held snapshot is u1's");
    }

    #[test]
    fn a_title_asked_for_shows_requested_in_every_row_and_forces_a_read() {
        let hub = Hub::new();
        let ear = Ear::default();
        hub.open(&key(), Some(Box::new(ear.clone())), NOW);
        hub.disk_read(Some(whole()), NOW);
        hub.finish(&key(), true, NOW);
        let before = ear.heard().len();
        hub.requested(Kind::Movie, 10);
        let heard = ear.heard();
        let Piece::Shelf(Shelf::Trending, trending) = &heard[before] else { panic!("{heard:?}") };
        assert_eq!(trending.items[0].state.badge(), Some(Badge::Requested), "never 'nothing' for a title asked for");
        let Piece::Mine(mine) = &heard[before + 1] else { panic!("{heard:?}") };
        assert_eq!((mine.items[0].title.tmdb, mine.items[0].badge), (10, Some(Badge::Requested)), "it joins My requests at once");
        assert!(!hub.snapshot().unwrap().fresh(NOW), "stale whatever the age");
        assert_eq!(hub.open(&key(), None, NOW + 1), Work::Start, "so the next open reads Seerr");
        hub.requested(Kind::Movie, 10);
        assert_eq!(ear.heard().len(), before + 2, "a title already shown as requested changes nothing");
    }

    #[test]
    fn a_title_asked_for_while_a_read_runs_runs_it_once_more() {
        let hub = Hub::new();
        assert_eq!(hub.open(&key(), None, NOW), Work::Start);
        hub.requested(Kind::Movie, 10);
        assert!(hub.finish(&key(), true, NOW).again);
        assert!(!hub.finish(&key(), true, NOW).again);
        assert_eq!(hub.open(&key(), None, NOW), Work::Start, "and the worker is released afterwards");
    }

    #[test]
    fn a_row_that_fails_is_reported_only_when_nothing_is_held_for_it() {
        let hub = Hub::new();
        let ear = Ear::default();
        hub.open(&key(), Some(Box::new(ear.clone())), NOW);
        hub.lost(&key(), Part::Mine, SeerrError::Unreachable);
        assert_eq!(ear.heard(), [Piece::Lost(Part::Mine, SeerrError::Unreachable)]);
        hub.disk_read(Some(whole()), NOW);
        let heard = ear.heard().len();
        hub.lost(&key(), Part::Mine, SeerrError::Unreachable);
        hub.failed(&key(), SeerrError::Unreachable);
        assert_eq!(ear.heard().len(), heard, "the held rows stay as they are");
    }

    #[test]
    fn the_first_posters_are_the_first_rows_the_page_shows() {
        let hub = Hub::new();
        hub.open(&key(), None, NOW);
        hub.disk_read(Some(whole()), NOW);
        let urls = hub.first_posters(2, 5);
        assert_eq!(urls, ["https://image.tmdb.org/t/p/w342/p1.jpg", "https://image.tmdb.org/t/p/w342/p10.jpg"]);
    }

    #[test]
    fn the_file_round_trips_owner_only_and_a_stranger_file_is_ignored() {
        let dir = plx_base::testscratch::process_dir("seerr-cache");
        let (a, b) = (dir.join("first.json"), dir.join("missing").join("second.json"));
        assert!(save(&[b.clone(), a.clone()], &whole()), "the next place takes it when one refuses");
        assert_eq!(load(&[b.clone(), a.clone()]), Some(whole()));
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&a, b"{\"format\":99}").unwrap();
        assert_eq!(load(&[a.clone()]), None);
        std::fs::write(&a, b"not json").unwrap();
        assert_eq!(load(&[a.clone()]), None);
        purge(&[a.clone()]);
        assert!(!a.exists());
    }

    /// A Seerr that answers by path from several threads, counting what it was asked.
    struct Fake {
        routes: Vec<(&'static str, u16, &'static str)>,
        seen: Mutex<Vec<String>>,
    }

    impl Transport for Fake {
        fn send(&self, _base: &str, _cookie: &str, request: &Request) -> Result<Reply, TransportError> {
            assert_eq!(request.verb, Verb::Get);
            self.seen.lock().unwrap().push(request.path.clone());
            let (status, body) = match self.routes.iter().find(|(p, ..)| *p == request.path) {
                Some((_, status, body)) => (*status, *body),
                None => (404, "{}"),
            };
            Ok(Reply { status, body: body.as_bytes().to_vec(), cookies: Vec::new() })
        }
    }

    impl Fake {
        fn new(routes: &[(&'static str, u16, &'static str)]) -> Self {
            Self { routes: routes.to_vec(), seen: Mutex::new(Vec::new()) }
        }
        fn titles_read(&self) -> Vec<String> {
            self.seen.lock().unwrap().iter().filter(|p| p.starts_with("/api/v1/movie/") || p.starts_with("/api/v1/tv/")).cloned().collect()
        }
    }

    const LOGIN: Login<'static> = Login { base: "https://requests.example.net", cookie: "connect.sid=s%3Afresh", plex_token: "plex-token" };

    fn seerr_routes() -> Vec<(&'static str, u16, &'static str)> {
        vec![
            ("/api/v1/settings/public", 200, fixture!("settings_public.json")),
            ("/api/v1/auth/me", 200, fixture!("auth_me.json")),
            ("/api/v1/request?take=20&skip=0&sort=added&requestedBy=1", 200, fixture!("requests_mine.json")),
            ("/api/v1/discover/trending?page=1", 200, fixture!("discover_trending.json")),
            ("/api/v1/discover/movies?page=1", 200, fixture!("discover_trending.json")),
            ("/api/v1/discover/tv?page=1", 200, fixture!("discover_trending.json")),
            ("/api/v1/discover/movies/upcoming?page=1", 200, fixture!("discover_movies_upcoming.json")),
            ("/api/v1/movie/1423191", 200, fixture!("movie_new.json")),
        ]
    }

    #[test]
    fn a_refresh_lands_every_row_and_reads_only_the_titles_it_does_not_know() {
        let hub = Hub::new();
        let ear = Ear::default();
        hub.open(&key(), Some(Box::new(ear.clone())), NOW);
        let mut known = whole();
        // two of the viewer's eight requests are held already
        known.apply(&Piece::Mine(Mine {
            items: vec![
                Asked { title: title(Kind::Movie, 656908, Have::Processing), badge: None },
                Asked { title: title(Kind::Movie, 1170608, Have::Processing), badge: None },
            ],
            more: false,
        }));
        hub.disk_read(Some(known), NOW);
        let held = ear.heard().len();
        let fake = Fake::new(&seerr_routes());
        let (ok, cookie) = refresh(&hub, &key(), &LOGIN, &fake);
        assert!(ok);
        assert_eq!(cookie, LOGIN.cookie, "the session held is kept");
        let heard = ear.heard();
        assert_eq!(heard.len() - held, 6, "the rules, the viewer's requests and four shelves");
        let Some(Piece::Mine(mine)) = heard[held..].iter().find(|p| matches!(p, Piece::Mine(_))) else { panic!("{heard:?}") };
        let ids: Vec<u32> = mine.items.iter().map(|a| a.title.tmdb).collect();
        assert_eq!(&ids[..2], [656908, 1170608], "the held titles keep their place and are not read again");
        let read = fake.titles_read();
        assert!(read.iter().all(|p| !p.ends_with("/656908") && !p.ends_with("/1170608")), "{read:?}");
        assert_eq!(read.len(), 6, "only the other six requests are read");
        let done = hub.finish(&key(), ok, NOW + 7);
        assert!(done.save.is_some_and(|s| s.fresh(NOW + 7)), "a complete read is saved and fresh");
    }

    #[test]
    fn a_second_refresh_does_not_read_a_title_it_already_holds() {
        let hub = Hub::new();
        hub.open(&key(), None, NOW);
        hub.disk_read(None, NOW);
        let fake = Fake::new(&seerr_routes());
        assert!(refresh(&hub, &key(), &LOGIN, &fake).0);
        assert_eq!(fake.titles_read().len(), 8, "a cold read asks for every title of the viewer's requests");
        let again = Fake::new(&seerr_routes());
        assert!(refresh(&hub, &key(), &LOGIN, &again).0);
        // Titles the first pass saw in a shelf are not read again.
        assert!(again.titles_read().len() < 8);
    }

    #[test]
    fn a_refresh_that_fails_leaves_the_rows_shown_and_says_nothing() {
        let hub = Hub::new();
        let ear = Ear::default();
        hub.open(&key(), Some(Box::new(ear.clone())), NOW);
        hub.disk_read(Some(whole()), NOW);
        let held = ear.heard().len();
        let mut routes = seerr_routes();
        routes.retain(|(p, ..)| !p.contains("upcoming"));
        let (ok, _) = refresh(&hub, &key(), &LOGIN, &Fake::new(&routes));
        assert!(!ok, "a row failed");
        assert!(ear.heard()[held..].iter().all(|p| !matches!(p, Piece::Lost(..) | Piece::Failed(_))), "the held row stays");
        assert!(hub.snapshot().unwrap().shelves[Shelf::Upcoming.index()].is_some());
        assert!(hub.finish(&key(), ok, NOW + 1).save.is_none(), "an incomplete pass is not saved or made fresh");
        assert!(hub.wants_refresh(NOW + FRESH_SECS), "so the next open reads Seerr again");

        let none = Hub::new();
        let blank = Ear::default();
        none.open(&key(), Some(Box::new(blank.clone())), NOW);
        none.disk_read(None, NOW);
        assert!(!refresh(&none, &key(), &LOGIN, &Fake::new(&[])).0);
        assert_eq!(blank.heard(), [Piece::Failed(SeerrError::NotFound)], "with nothing held the page is told");
    }
}
