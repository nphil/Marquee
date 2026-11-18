//! The profile pictures of the "who's watching" picker, resident before its first draw.
//!
//! The picker draws each avatar through the ordinary poster pipeline: a lookup claims a slot, a
//! worker reads the disk cache (or fetches), decodes a JPEG and hands the pixels over, and a later
//! frame uploads them. On a television that chain took a second or two, and the picker showed blank
//! circles meanwhile. Here the pictures are kept already decoded in one small file
//! ([`plx_platform::imgcache::tiles`]); [`start`] reads it on a thread of its own at launch, and
//! [`frame`] files every picture on the roster into the store as a finished, uploaded poster just
//! before the picker's first draw, which then finds an ordinary READY slot. Nothing in the screens
//! knows this exists: the key a seed is filed under is the key the picker's own request builds.
//!
//! Three things keep the file true to the server:
//!
//! * **Write-through.** Every avatar the ordinary pipeline decodes ([`capture`], [`persist`]) is
//!   kept, so the file builds itself by use: the first launch after an update fills it, and the
//!   picker is instant from the second.
//! * **Revalidation.** A tile whose roster stamp changed, or that no server confirmed for a day, is
//!   fetched again by the image refresh worker ([`refreshed`]). A changed picture replaces the
//!   resident texture in place ([`drain_swaps`]), so no frame is ever drawn without a picture.
//! * **Roster.** A profile that left the roster takes its tile with it ([`Avatars::pass`]); a
//!   sign-out removes the file with the rest of the image cache and bumps the cache epoch, which
//!   drops every tile held here.
//!
//! Disk and decode work happens on background threads only. The main thread copies pixels and
//! uploads textures, as it does for any poster.

use super::{
    built_key, claim, disk_namespace, refresh, same_art, store, victim, Claim, GfxUploader, CV, P_EMPTY, P_LOADING,
    P_READY, P_WANT,
};
use plx_gfx::img;
use plx_machine::machine::PosterKey;
use plx_platform::imgcache::{
    self,
    tiles::{self, Identity, Tile},
};
use plx_plex::plex::ServerId;
use plx_screens::profiles::AVATAR_BOX;
use plx_session::auth::UserTile;
use plx_ui::tex::{self, Decoded, PosterReady, Uploader};
use std::os::raw::{c_int, c_uchar};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// The longest boot waits for the stored pictures to be read. The read starts at launch and has the
/// whole of boot to finish, so this only matters on a disk slower than all of it.
const BOOT_WAIT: Duration = Duration::from_millis(100);

static AVATARS: Avatars = Avatars::new();

/// The stored pictures and what is waiting to happen to them. One value serves the process
/// ([`AVATARS`]); the tests build their own.
struct Avatars {
    state: Mutex<State>,
    /// Notified once the file has been read.
    read: Condvar,
    /// A refresh queued swaps. Read every frame, so [`drain_swaps`] takes no lock when it is clear.
    waiting: AtomicBool,
}

struct State {
    /// The image cache epoch the tiles belong to. A sign-out moves it, and the pictures of the
    /// account that left are dropped.
    generation: u64,
    tiles: Vec<Arc<Tile>>,
    /// The file has been read (or could not be): nothing waits on it any more.
    loaded: bool,
    /// The roster the last complete pass saw; the same roster is not looked at again.
    roster: Option<Arc<[UserTile]>>,
    /// Pictures a refresh found changed, waiting for the main thread to put them into their
    /// resident textures.
    swaps: Vec<Swap>,
    /// `tiles` differs from the file.
    dirty: bool,
    /// A thread is writing the file. Changes made meanwhile only set `dirty`; that thread writes
    /// again before it stops.
    writing: bool,
}

/// A changed picture for the slot drawn under `key`.
struct Swap {
    srv: ServerId,
    key: String,
    tile: Arc<Tile>,
}

/// What one pass over a roster did, for the caller to act on once no lock is held.
#[derive(Debug, Default, PartialEq, Eq)]
struct Pass {
    /// Pictures made resident.
    seeded: usize,
    /// The requests whose tile is due a check against the server.
    due: Vec<String>,
    /// Tiles of profiles that left the roster were dropped: the file needs rewriting.
    pruned: bool,
}

/// What a refresh did to the tile it was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revision {
    /// Not a profile picture, or not one this process keeps.
    Ignored,
    /// The same picture: only its version and confirmation moved.
    Confirmed,
    /// A picture the file did not hold yet.
    Stored,
    /// A different picture: the tile was replaced and a swap queued.
    Changed,
}

impl State {
    const fn new() -> Self {
        Self {
            generation: 0,
            tiles: Vec::new(),
            loaded: false,
            roster: None,
            swaps: Vec::new(),
            dirty: false,
            writing: false,
        }
    }

    /// Forget the pictures of an account that has since signed out: `epoch` is the image cache's
    /// current one. The file went with the cache, so nothing is marked dirty.
    fn follow(&mut self, epoch: u64) {
        if self.generation != epoch {
            self.tiles.clear();
            self.generation = epoch;
        }
    }

    /// Hold `tile` as the picture for its name: replace the one held or add it, and when the file's
    /// tile count is reached let the oldest go.
    fn keep(&mut self, tile: Arc<Tile>) {
        if let Some(held) = self.tiles.iter_mut().find(|t| t.name == tile.name) {
            *held = tile;
            return;
        }
        if self.tiles.len() >= tiles::MAX_TILES {
            self.tiles.remove(0);
        }
        self.tiles.push(tile);
    }
}

fn same_picture(a: &Tile, b: &Tile) -> bool {
    a.w == b.w && a.h == b.h && a.rgba == b.rgba
}

impl Avatars {
    const fn new() -> Self {
        Self { state: Mutex::new(State::new()), read: Condvar::new(), waiting: AtomicBool::new(false) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The file has been read: `tiles` are what it held, read under cache epoch `generation`.
    fn install(&self, generation: u64, tiles: Vec<Tile>) {
        let mut st = self.lock();
        st.generation = generation;
        st.tiles = tiles.into_iter().map(Arc::new).collect();
        st.loaded = true;
        drop(st);
        self.read.notify_all();
    }

    fn wait_loaded(&self, within: Duration) {
        let st = self.lock();
        drop(self.read.wait_timeout_while(st, within, |s| !s.loaded));
    }

    /// Is there a roster on the picker that has not been looked at yet?
    fn wants(&self, users: &Arc<[UserTile]>, on_picker: bool) -> bool {
        let mut st = self.lock();
        if !on_picker {
            // Leaving the picker forgets the roster, so coming back looks at it again.
            st.roster = None;
            return false;
        }
        st.loaded && !st.roster.as_ref().is_some_and(|seen| Arc::ptr_eq(seen, users))
    }

    /// File the stored picture of every profile in `thumbs` as a resident poster (a seed) and
    /// report what else the roster asks for. `None` when that cannot be done yet, because the file
    /// is unread or no server is registered to build the picker's request with: the next frame asks
    /// again.
    fn pass(&self, srv: ServerId, thumbs: &[&str], up: &mut dyn Uploader, now: u64) -> Option<Pass> {
        plx_plex::plex::client_for(srv)?;
        // The roster's pictures exactly as the picker requests them, which is what a draw probes.
        let mut wanted: Vec<(String, Identity)> = Vec::with_capacity(thumbs.len());
        let mut complete = true;
        for thumb in thumbs.iter().filter(|t| !t.is_empty()) {
            let asked = built_key(srv, thumb, AVATAR_BOX, AVATAR_BOX, false)
                .and_then(|key| tiles::identify(&key).map(|id| (key.to_string(), id)));
            match asked {
                Some(w) => wanted.push(w),
                None => complete = false,
            }
        }
        let mut out = Pass::default();
        let held = {
            let mut st = self.lock();
            if !st.loaded {
                return None;
            }
            st.follow(imgcache::generation());
            // A profile that left takes its picture with it. Only a roster read whole may say so:
            // one entry this build cannot name must not cost the rest their pictures.
            if complete && !wanted.is_empty() {
                let before = st.tiles.len();
                st.tiles.retain(|t| wanted.iter().any(|(_, id)| id.name == t.name));
                out.pruned = st.tiles.len() != before;
                st.dirty |= out.pruned;
            }
            wanted
                .iter()
                .filter_map(|(key, id)| {
                    let tile = st.tiles.iter().find(|t| t.name == id.name)?;
                    Some((key.clone(), id.stamp.clone(), Arc::clone(tile)))
                })
                .collect::<Vec<_>>()
        };
        for (key, stamp, tile) in held {
            if tile.due(&stamp, now) {
                out.due.push(key.clone());
            }
            if seed(srv, &key, &tile, up) {
                out.seeded += 1;
            }
        }
        Some(out)
    }

    /// The tile for a picture the ordinary path just decoded under `key`, or `None` when that is
    /// not a profile picture or the tile held already says as much. `fetched` is 0 for a picture
    /// read back from disk, which no server has confirmed lately.
    fn capture(&self, key: &str, rgba: &[u8], w: usize, h: usize, fetched: u64) -> Option<Tile> {
        let id = tiles::identify(key)?;
        let known = self.lock().tiles.iter().any(|t| {
            t.name == id.name
                && t.stamp == id.stamp
                && usize::from(t.w) == w
                && usize::from(t.h) == h
                && *t.rgba == *rgba
                && t.fetched >= fetched
        });
        if known {
            return None;
        }
        Tile::new(id, fetched, w, h, rgba)
    }

    /// Keep a captured `tile` and mark the file out of date. `epoch` is the cache epoch the
    /// picture was decoded under; `false` when it no longer is the current one, or the file is not
    /// read yet (the picture then simply waits for its next decode).
    fn persist(&self, epoch: u64, tile: Tile) -> bool {
        let mut st = self.lock();
        if !st.loaded || epoch != imgcache::generation() {
            return false;
        }
        st.follow(epoch);
        st.keep(Arc::new(tile));
        st.dirty = true;
        true
    }

    /// A refresh fetched and decoded the picture for `key`, a request built under cache epoch
    /// `epoch` for server `srv`.
    fn refreshed(&self, srv: ServerId, epoch: u64, key: &str, rgba: &[u8], w: usize, h: usize, now: u64) -> Revision {
        let Some(id) = tiles::identify(key) else { return Revision::Ignored };
        let Some(fresh) = Tile::new(id, now, w, h, rgba) else { return Revision::Ignored };
        let fresh = Arc::new(fresh);
        let mut st = self.lock();
        if !st.loaded || epoch != imgcache::generation() {
            return Revision::Ignored;
        }
        st.follow(epoch);
        let revision = match st.tiles.iter().position(|t| t.name == fresh.name) {
            Some(i) if same_picture(&st.tiles[i], &fresh) => {
                st.tiles[i] = fresh;
                Revision::Confirmed
            }
            Some(i) => {
                st.tiles[i] = Arc::clone(&fresh);
                st.swaps.push(Swap { srv, key: key.to_string(), tile: fresh });
                Revision::Changed
            }
            None => {
                st.keep(fresh);
                Revision::Stored
            }
        };
        st.dirty = true;
        if revision == Revision::Changed {
            self.waiting.store(true, Ordering::Release);
        }
        revision
    }

    /// MAIN thread. Put every queued swap into the texture its picture is resident in: the cache
    /// replaces a resident texture in place, so the draw has a picture on every frame. A picture no
    /// slot is showing any more is dropped here; the next picker seeds from the tile.
    fn apply_swaps(&self) {
        if !self.waiting.swap(false, Ordering::AcqRel) {
            return;
        }
        let swaps = std::mem::take(&mut self.lock().swaps);
        for swap in swaps {
            let slot = {
                let mut g = store();
                let found = g.slots.iter().position(|s| s.state == P_READY && same_art(s, swap.srv, swap.key.as_bytes()));
                if let Some(i) = found {
                    g.slots[i].pw = c_int::from(swap.tile.w);
                    g.slots[i].ph = c_int::from(swap.tile.h);
                }
                found
            };
            if let Some(i) = slot {
                tex::accept(PosterReady {
                    key: PosterKey(i as u32),
                    result: Ok(Decoded { w: swap.tile.w, h: swap.tile.h, rgba: swap.tile.rgba.clone() }),
                });
            }
        }
    }

    /// Write the file if it is out of date. Callers combine: a thread that finds another writing
    /// returns at once, and the writer takes what they changed before it stops, so the file is
    /// never written twice at the same time and no change is left behind.
    fn flush(&self, save: &mut dyn FnMut(u64, &[&Tile]) -> bool) {
        if std::mem::replace(&mut self.lock().writing, true) {
            return;
        }
        loop {
            let (generation, snapshot) = {
                let mut st = self.lock();
                if !st.dirty {
                    st.writing = false;
                    return;
                }
                st.dirty = false;
                (st.generation, st.tiles.clone())
            };
            let refs: Vec<&Tile> = snapshot.iter().map(|t| t.as_ref()).collect();
            save(generation, &refs);
        }
    }
}

/// File `tile` as the finished picture for `key` and make its texture resident, or `false` when
/// that was not done: a slot already answers this draw (the ordinary path owns it), or none is free,
/// or the cache refused the texture, in which case the ordinary path loads it as it always did.
fn seed(srv: ServerId, key: &str, tile: &Tile, up: &mut dyn Uploader) -> bool {
    let Some(client) = plx_plex::plex::client_for(srv) else { return false };
    let (cache_gen, token_gen, grant_epoch) = (imgcache::generation(), client.token_gen(), client.grant_epoch());
    let (idx, gen, was_ready, old_px) = {
        let mut g = store();
        if g.slots.iter().any(|s| {
            s.state != P_EMPTY
                && s.cache_gen == cache_gen
                && s.grant_epoch == grant_epoch
                && same_art(s, srv, key.as_bytes())
        }) {
            return false;
        }
        let Some(idx) = victim(&g.slots, g.frame) else { return false };
        let (was_ready, old_px) = (g.slots[idx].state == P_READY, g.slots[idx].px);
        g.clock = g.clock.wrapping_add(1);
        let (use_, frame) = (g.clock, g.frame);
        // LOADING, which no worker picks up and no victim search takes, while the main thread
        // makes the texture resident below.
        let gen = claim(
            &mut g.slots[idx],
            Claim {
                srv,
                key,
                cache_gen,
                token_gen,
                grant_epoch,
                state: P_LOADING,
                visible: true,
                ahead: false,
                use_,
                frame,
            },
        );
        (idx, gen, was_ready, old_px)
    };
    let slot = PosterKey(idx as u32);
    // Release what the recycled slot held, off the lock, as a lookup's claim does.
    if was_ready {
        tex::free(slot, up);
    }
    if old_px != 0 {
        img::img_free(old_px as *mut c_uchar);
    }
    let landed = tex::prime(slot, Decoded { w: tile.w, h: tile.h, rgba: tile.rgba.clone() }, up);
    let mut g = store();
    let s = &mut g.slots[idx];
    if s.gen != gen || s.state != P_LOADING {
        return landed;
    }
    if landed {
        s.state = P_READY;
        s.pw = c_int::from(tile.w);
        s.ph = c_int::from(tile.h);
    } else {
        s.state = P_WANT;
        drop(g);
        CV.notify_one();
    }
    landed
}

/// Read the stored pictures, and warm the image cache's index, on a thread of their own from the
/// start of the launch, so the picker's first frame finds both in memory. After the identity lines
/// of the event log: the thread logs.
pub(crate) fn start() {
    let spawned = plx_base::task::spawn("avatar-tiles", || {
        let (generation, stored) = tiles::load();
        let n = stored.len();
        AVATARS.install(generation, stored);
        plx_base::diag::startup::mark_with(plx_base::diag::startup::Stage::Avatars, &format!("tiles={n}"));
        // The directory scan (about a thousand files on the television) the first poster read
        // would otherwise wait for.
        imgcache::prewarm();
    });
    if spawned.is_none() {
        // No thread: nothing will ever read the file, and nothing may wait for it.
        AVATARS.install(imgcache::generation(), Vec::new());
    }
}

/// Boot, once, before the first frame: wait (bounded) for [`start`]'s read, so the picker's first
/// frame is not drawn without the pictures on a disk slower than the rest of boot.
pub(crate) fn await_loaded() {
    AVATARS.wait_loaded(BOOT_WAIT);
}

/// MAIN thread, once per presenting frame, before that frame's uploads and draw. On the picker,
/// with a roster not looked at yet: file every stored picture on it ([`Avatars::pass`]), ask the
/// refresh worker to check the ones that are due, and prune the file when a profile left.
pub(crate) fn frame(users: &Arc<[UserTile]>, on_picker: bool) {
    if !AVATARS.wants(users, on_picker) || crate::dev::scenarios::imagecache_bypass_armed() {
        return;
    }
    let began = plx_base::diag::heartbeat::now_us();
    let srv = plx_plex::plex::current_server();
    let thumbs: Vec<&str> = users.iter().map(|u| u.thumb.as_str()).collect();
    let Some(done) = AVATARS.pass(srv, &thumbs, &mut GfxUploader, tiles::now()) else { return };
    AVATARS.lock().roster = Some(Arc::clone(users));
    if done.seeded > 0 {
        let micros = plx_base::diag::heartbeat::now_us().saturating_sub(began);
        plx_base::eventlog::log(&format!(
            "avatars: seeded {} of {} profile pictures in {:.1} ms",
            done.seeded,
            thumbs.len(),
            micros as f64 / 1000.0
        ));
    }
    for key in done.due {
        let Some(client) = plx_plex::plex::client_for(srv) else { break };
        if let Some(disk) = imgcache::classify(&disk_namespace(client), &key) {
            refresh::enqueue(client, key, disk, imgcache::generation(), client.token_gen());
        }
    }
    if done.pruned {
        flush_in_background();
    }
}

/// WORKER thread. The pixels behind a decoder's output pointer, with their size, when it is one a
/// picture can have.
///
/// # Safety
/// `px` is the pointer `img::img_decode_rgba` returned for exactly this `w` and `h`, and stays
/// allocated for as long as the slice is used.
pub(super) unsafe fn view<'a>(px: *const c_uchar, w: c_int, h: c_int) -> Option<(&'a [u8], usize, usize)> {
    let sides = 1..=c_int::from(u16::MAX);
    if px.is_null() || !sides.contains(&w) || !sides.contains(&h) {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    // SAFETY: the contract above: w * h RGBA bytes at `px`, and both sides are positive here.
    Some((unsafe { std::slice::from_raw_parts(px, w * h * 4) }, w, h))
}

/// WORKER thread. The tile for a picture the poster worker decoded under `key`, if it is a profile
/// picture the file does not hold yet. `confirmed`: the pixels came from the server just now, not
/// from the disk cache.
pub(super) fn capture(key: &str, rgba: &[u8], w: usize, h: usize, confirmed: bool) -> Option<Tile> {
    AVATARS.capture(key, rgba, w, h, if confirmed { tiles::now() } else { 0 })
}

/// WORKER thread. Keep what [`capture`] returned once the slot accepted the decode, and write the
/// file.
pub(super) fn persist(cache_gen: u64, tile: Tile) {
    if AVATARS.persist(cache_gen, tile) {
        flush_to_disk();
    }
}

/// REFRESH thread. The picture the image refresh just fetched and decoded for `key`: update the
/// tile, and when the picture changed, queue the swap into the resident texture.
pub(super) fn refreshed(srv: ServerId, epoch: u64, key: &str, rgba: &[u8], w: usize, h: usize) {
    match AVATARS.refreshed(srv, epoch, key, rgba, w, h, tiles::now()) {
        Revision::Ignored => {}
        Revision::Confirmed | Revision::Stored => flush_to_disk(),
        Revision::Changed => {
            plx_base::eventlog::log("avatars: a profile picture changed on the server, swapping it in");
            plx_machine::idle::wake();
            flush_to_disk();
        }
    }
}

/// MAIN thread, once per frame, after the decoded posters were handed over.
pub(super) fn drain_swaps() {
    AVATARS.apply_swaps();
}

fn flush_to_disk() {
    AVATARS.flush(&mut |generation, kept| {
        let saved = tiles::save(generation, kept);
        plx_base::eventlog::log(&format!(
            "avatars: {} profile picture(s) {}",
            kept.len(),
            if saved { "stored" } else { "not stored" }
        ));
        saved
    });
}

/// For the main thread, which does no disk work.
fn flush_in_background() {
    let _ = plx_base::task::spawn("avatar-write", flush_to_disk);
}

#[cfg(test)]
mod tests {
    use super::super::{reset_key_memo, Pslot, PT_CAP, SOURCE};
    use super::*;
    use plx_ui::tex::Tex;

    /// A time a little after the versions below were made.
    const NOW: u64 = 1_700_000_100;
    const STAMP: &str = "1700000000";
    const NEWER: &str = "1800000000";

    /// An avatar address of the shape plex.tv serves, versioned by `c=`.
    fn thumb(user: &str, stamp: &str) -> String {
        format!("https://plex.tv/users/{user}/avatar?c={stamp}")
    }

    #[derive(Default)]
    struct Fake {
        next: u32,
        uploads: u32,
        freed: Vec<u32>,
    }
    impl Uploader for Fake {
        fn upload(&mut self, d: &Decoded) -> tex::Tex {
            self.next += 1;
            self.uploads += 1;
            Tex { id: self.next, w: d.w, h: d.h }
        }
        fn warm(&mut self, _: Tex) {}
        fn free(&mut self, t: Tex) {
            self.freed.push(t.id);
        }
    }

    /// A registry holding one server, an empty store and an empty texture cache, put back on the
    /// way out. `register_for_test`, not `register`: the latter mints and persists a device id.
    struct Fresh(#[allow(dead_code)] plx_base::testlock::Serial, ServerId);
    impl Drop for Fresh {
        fn drop(&mut self) {
            tex::shutdown(&mut Fake::default());
            store().slots = [Pslot::ZERO; PT_CAP];
            plx_plex::plex::reset_servers_for_test();
            reset_key_memo();
        }
    }
    fn fresh() -> Fresh {
        let serial = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        reset_key_memo();
        let srv = plx_plex::plex::register_for_test("avatar-test", "127.0.0.1", 32400, "tok-avatar-test", "cid-avatar-test");
        tex::install(&SOURCE);
        tex::reset_for_test(8 * 1024 * 1024);
        let mut g = store();
        g.slots = [Pslot::ZERO; PT_CAP];
        g.quit = false;
        drop(g);
        Fresh(serial, srv)
    }

    /// The request the picker builds for `thumb`.
    fn asked(srv: ServerId, thumb: &str) -> String {
        built_key(srv, thumb, AVATAR_BOX, AVATAR_BOX, false).unwrap().to_string()
    }

    /// The stored 2x2 picture of `thumb`, every byte `shade`.
    fn tile(srv: ServerId, thumb: &str, shade: u8, fetched: u64) -> Tile {
        Tile::new(tiles::identify(&asked(srv, thumb)).unwrap(), fetched, 2, 2, &[shade; 16]).unwrap()
    }

    fn loaded(tiles: Vec<Tile>) -> Avatars {
        let a = Avatars::new();
        a.install(imgcache::generation(), tiles);
        a
    }

    fn draw(srv: ServerId, thumb: &str) -> u32 {
        tex::resolve_on(srv.raw(), thumb, AVATAR_BOX, AVATAR_BOX, false)
    }

    /// The upload step of one presenting frame.
    fn prepare(up: &mut Fake) -> usize {
        let mut budget = plx_ui::frame::Budget::new();
        budget.begin_frame(0);
        let mut present = plx_machine::present::Present::new();
        let mut handle = plx_machine::machine::PresentHandle::of(&mut present);
        tex::prepare(&mut budget, up, &mut handle, || 0)
    }

    fn saved(a: &Avatars) -> Vec<(String, u8)> {
        let mut out = Vec::new();
        a.flush(&mut |_, kept| {
            out = kept.iter().map(|t| (t.stamp.clone(), t.rgba[0])).collect();
            true
        });
        out
    }

    #[test]
    fn a_stored_picture_is_resident_before_the_picker_draws() {
        let f = fresh();
        let (alex, sam) = (thumb("aaa111", STAMP), thumb("bbb222", STAMP));
        let a = Avatars::new();
        let mut up = Fake::default();
        assert!(a.pass(f.1, &[&alex, &sam], &mut up, NOW).is_none(), "the file is unread: ask again next frame");

        a.install(imgcache::generation(), vec![tile(f.1, &alex, 10, NOW), tile(f.1, &sam, 20, NOW)]);
        let done = a.pass(f.1, &[&alex, &sam], &mut up, NOW).expect("read, and a server");
        assert_eq!((done.seeded, up.uploads), (2, 2));
        assert!(done.due.is_empty() && !done.pruned, "{done:?}");

        // The draw hits on its first probe: nothing is queued for a worker, nothing uploads.
        assert_ne!(draw(f.1, &alex), 0);
        assert_ne!(draw(f.1, &sam), 0);
        assert!(store().slots.iter().all(|s| s.state != P_WANT && s.state != P_LOADING));
        assert_eq!(up.uploads, 2);
    }

    #[test]
    fn a_changed_picture_shows_the_stored_one_then_swaps_in_place() {
        let f = fresh();
        let (stored, roster) = (thumb("aaa111", STAMP), thumb("aaa111", NEWER));
        let a = loaded(vec![tile(f.1, &stored, 10, NOW)]);
        let mut up = Fake::default();

        // The roster carries a newer version: the stored picture goes up at once under the new
        // request, and a check is asked for.
        let done = a.pass(f.1, &[&roster], &mut up, NOW + 60).unwrap();
        assert_eq!((done.seeded, done.due.len()), (1, 1));
        let before = draw(f.1, &roster);
        assert_ne!(before, 0);

        // The check brings different pixels: the tile is replaced and the swap queued, while the
        // resident texture is still the old one.
        let epoch = imgcache::generation();
        assert_eq!(a.refreshed(f.1, epoch, &done.due[0], &[99; 16], 2, 2, NOW + 120), Revision::Changed);
        a.apply_swaps();
        assert_eq!(draw(f.1, &roster), before, "a swap waits for the upload step");
        assert_eq!(prepare(&mut up), 1);
        let after = draw(f.1, &roster);
        assert!(after != 0 && after != before, "the texture was replaced");
        assert_eq!(up.freed, vec![before]);
        assert_eq!(store().slots.iter().filter(|s| s.state == P_READY).count(), 1, "the same slot, still READY");

        assert_eq!(saved(&a), vec![(NEWER.to_string(), 99)]);
    }

    #[test]
    fn a_picture_the_server_still_has_only_moves_the_confirmation() {
        let f = fresh();
        let (alex, sam) = (thumb("aaa111", STAMP), thumb("bbb222", STAMP));
        let a = loaded(vec![tile(f.1, &alex, 10, 0), tile(f.1, &sam, 20, NOW)]);
        let mut up = Fake::default();

        let done = a.pass(f.1, &[&alex, &sam], &mut up, NOW).unwrap();
        assert_eq!(done.due, vec![asked(f.1, &alex)], "only the picture no server confirmed is checked");
        let epoch = imgcache::generation();
        assert_eq!(a.refreshed(f.1, epoch, &done.due[0], &[10; 16], 2, 2, NOW), Revision::Confirmed);
        assert!(a.lock().swaps.is_empty(), "nothing to swap");
        assert!(a.pass(f.1, &[&alex, &sam], &mut up, NOW + 60).unwrap().due.is_empty());
    }

    #[test]
    fn a_profile_that_left_the_roster_loses_its_stored_picture() {
        let f = fresh();
        let (alex, sam, kids) = (thumb("aaa111", STAMP), thumb("bbb222", STAMP), thumb("ccc333", STAMP));
        let a = loaded(vec![tile(f.1, &alex, 10, NOW), tile(f.1, &sam, 20, NOW), tile(f.1, &kids, 30, NOW)]);
        let mut up = Fake::default();

        let done = a.pass(f.1, &[&alex, &kids], &mut up, NOW).unwrap();
        assert!(done.pruned);
        assert_eq!((done.seeded, up.uploads), (2, 2), "the removed profile's picture is never put up");
        assert_eq!(saved(&a).iter().map(|(_, shade)| *shade).collect::<Vec<_>>(), vec![10, 30]);

        // A roster with an entry this build cannot name is not read whole, so it prunes nothing.
        let partial = a.pass(f.1, &[&alex, "/library/metadata/1/thumb"], &mut up, NOW).unwrap();
        assert!(!partial.pruned);
        assert_eq!(a.lock().tiles.len(), 2);
    }

    #[test]
    fn only_new_or_changed_profile_pictures_are_captured_and_kept() {
        let f = fresh();
        let alex = asked(f.1, &thumb("aaa111", STAMP));
        let a = loaded(Vec::new());

        assert!(a.capture("/library/metadata/42/thumb", &[1; 16], 2, 2, NOW).is_none(), "a poster is no profile picture");
        let first = a.capture(&alex, &[10; 16], 2, 2, NOW).expect("a picture the file lacks");
        assert!(a.persist(imgcache::generation(), first));
        assert!(a.capture(&alex, &[10; 16], 2, 2, 0).is_none(), "a disk hit of the picture held adds nothing");
        assert!(a.capture(&alex, &[11; 16], 2, 2, 0).is_some(), "other pixels are kept");
        assert!(!a.persist(imgcache::generation() + 1, tile(f.1, &thumb("aaa111", STAMP), 1, NOW)), "an old epoch is dropped");
    }
}
