//! **The Requests area, application side**: the Seerr calls the Requests page and a title's page
//! ask for.
//!
//! The page opens on the last answers held ([`seerr_cache`]): `Open` hands them over on the spot and
//! one worker (`sync`) reads the saved copy from disk and refreshes behind it when it is old, so
//! nothing here waits on Seerr for the first frame. The rest is a child of `bridge`, like
//! `watchlist`: each command runs on its own short-lived worker, reads the Seerr link and the
//! seated Plex account when it starts, and answers down the channel the command carries. The Seerr
//! session cookie is the only thing that outlives a call: a worker that signed in again hands the
//! new one to the session record.
//!
//! A request is never sent blind: [`Seerr::request`] reads the title's status first and answers
//! `AlreadyRequested` when Seerr (or Pulsarr, which asks for every Watchlist add) already has it.
use super::*;
use plx_plex::plex::seerr::{Browse, CurlTransport, Kind, Seerr, SeerrError, Shelf, Sort};
use plx_plex::plex::ServerId;
use plx_plex::plex::seerr_cache::{self, Hub, Key, Login, Part, Piece, Sink, Work};
use plx_plex::plex::session::SeerrChange;
#[cfg(all(feature = "hostsim", feature = "devtriggers", not(all(target_os = "linux", target_arch = "arm"))))]
use plx_plex::plex::session::sim_link;
use plx_screens::registry::{RequestsCmd, RequestsReply, TitlePage};
use plx_plex::plex::arr::CurlTransport as ArrCurl;
use plx_plex::plex::progress::{self, Fail as ProgressFail};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// The answers held for the Requests page, one per process.
static HUB: Hub = Hub::new();

/// What a call needs, read once when it starts.
struct Link {
    base: String,
    cookie: String,
    plex_token: String,
    account: String,
}

impl Link {
    /// The link as a worker should use it. `ready` is `false` for Test connection, which may run
    /// before the switch is turned on.
    fn read(ready: bool) -> Result<Self, SeerrError> {
        let link = plx_plex::plex::session::peek().seerr();
        if link.url.is_empty() || (ready && !link.is_ready()) {
            return Err(SeerrError::NotSet);
        }
        #[cfg(all(feature = "hostsim", feature = "devtriggers", not(all(target_os = "linux", target_arch = "arm"))))]
        if sim_link::base().is_some() {
            return Ok(Self { base: link.url, cookie: String::new(), plex_token: sim_link::PLEX_TOKEN.into(),
                             account: sim_link::ACCOUNT.into() });
        }
        let user = plx_plex::plex::session::current().ok_or(SeerrError::NoPlexAccount)?;
        let plex_token = plx_plex::plex::session::plex_tv_credential(&user).ok_or(SeerrError::NoPlexAccount)?;
        Ok(Self { cookie: link.session_for(&user.uuid).to_string(), base: link.url, plex_token, account: user.uuid })
    }

    /// Hand a session that changed during the calls to the session record.
    fn keep(&self, after: String) {
        #[cfg(all(feature = "hostsim", feature = "devtriggers", not(all(target_os = "linux", target_arch = "arm"))))]
        if sim_link::base().is_some() {
            return;
        }
        if after == self.cookie {
            return;
        }
        let change = if after.is_empty() {
            SeerrChange::SignedOut
        } else {
            SeerrChange::SignedIn { url: self.base.clone(), cookie: after, account: self.account.clone() }
        };
        plx_plex::plex::session::queue_update_settled(
            move |current| Some(current.with_seerr(&change.applied(&current.seerr()))),
            |_| plx_machine::idle::invalidate(),
        );
    }
}

/// A concrete job keeps the worker launch and login code shared across Seerr commands.
enum SeerrJob {
    Shelf { shelf: Shelf, page: u32 },
    Mine { skip: u32 },
    Search { query: String, page: u32 },
    Detail { kind: Kind, tmdb: u32, art: Option<ServerId> },
    Stills { tmdb: u32, seasons: Vec<u32> },
    Request { kind: Kind, tmdb: u32, seasons: Vec<u32> },
    Browse { filter: Browse, sort: Sort, page: u32, gen: u32 },
    Genres,
}

impl SeerrJob {
    fn failed(&self, error: SeerrError) -> RequestsReply {
        match *self {
            Self::Shelf { shelf, page } => RequestsReply::Shelf { shelf, page, result: Err(error) },
            Self::Mine { skip } => RequestsReply::Mine { skip, result: Err(error) },
            Self::Search { ref query, page } => RequestsReply::Search { query: query.clone(), page, result: Err(error) },
            Self::Detail { kind, tmdb, .. } => RequestsReply::Detail { kind, tmdb, result: Err(error) },
            Self::Stills { tmdb, .. } => RequestsReply::Stills { tmdb, result: Err(error) },
            Self::Request { kind, tmdb, .. } => RequestsReply::Requested { kind, tmdb, result: Err(error) },
            Self::Browse { gen, sort, page, .. } => RequestsReply::Browse { gen, sort, page, result: Err(error) },
            Self::Genres => RequestsReply::Genres(Err(error)),
        }
    }

    fn answer(self, seerr: &Seerr<'_, CurlTransport>) -> RequestsReply {
        match self {
            Self::Shelf { shelf, page } => RequestsReply::Shelf { shelf, page, result: seerr.discover(shelf, page) },
            Self::Mine { skip } => {
                let now = seerr_cache::now();
                let known = HUB.snapshot(now);
                let result = seerr_cache::mine_page(seerr, skip, known.as_ref(), now).map(|mine| (mine.items, mine.more));
                RequestsReply::Mine { skip, result }
            }
            Self::Search { query, page } => {
                let result = seerr.search(&query, page);
                RequestsReply::Search { query, page, result }
            }
            Self::Detail { kind, tmdb, art } => {
                let result = seerr.detail(kind, tmdb).and_then(|detail| {
                    let rules = seerr.rules()?;
                    let plex = detail.plex.as_ref().and_then(|found| {
                        plx_plex::plex::id_of_machine(&found.machine).map(|sid| (sid, found.rating_key.clone()))
                    });
                    Ok(TitlePage { detail, plex, rules, art })
                });
                RequestsReply::Detail { kind, tmdb, result }
            }
            Self::Stills { tmdb, seasons } => {
                // A season that cannot be read leaves the others' stills standing.
                let mut stills = Vec::new();
                let mut failure = None;
                for season in seasons {
                    match seerr.stills(tmdb, season) {
                        Ok(read) => stills.extend(read),
                        Err(error) => failure = Some(error),
                    }
                }
                let result = match failure {
                    Some(error) if stills.is_empty() => Err(error),
                    _ => Ok(stills),
                };
                RequestsReply::Stills { tmdb, result }
            }
            Self::Request { kind, tmdb, seasons } => {
                let result = seerr.rules().and_then(|rules| seerr.request(kind, tmdb, &seasons, rules));
                // Asked now, or found already asked: every poster of the title says so at once.
                if matches!(result, Ok(()) | Err(SeerrError::AlreadyRequested)) {
                    HUB.requested(kind, tmdb);
                }
                RequestsReply::Requested { kind, tmdb, result }
            }
            Self::Browse { filter, sort, page, gen } => {
                let result = seerr.browse(&filter, sort, page, &today());
                RequestsReply::Browse { gen, sort, page, result }
            }
            Self::Genres => RequestsReply::Genres(seerr.genres()),
        }
    }
}

/// Answer even if neither a usable link nor a worker is available.
fn run(job: SeerrJob, reply: Sender<RequestsReply>) {
    let refused = job.failed(SeerrError::Unreachable);
    let back = reply.clone();
    let spawned = plx_base::task::spawn_small("seerr", move || {
        match Link::read(true) {
            Ok(link) => {
                let seerr = Seerr::new(&link.base, &link.cookie, &link.plex_token, &CurlTransport);
                say(&reply, job.answer(&seerr));
                link.keep(seerr.cookie());
            }
            Err(error) => {
                let _ = reply.send(job.failed(error));
            }
        }
        plx_machine::idle::invalidate();
    });
    if !spawned {
        let _ = back.send(refused);
        plx_machine::idle::invalidate();
    }
}

fn say(reply: &Sender<RequestsReply>, answer: RequestsReply) {
    let _ = reply.send(answer);
    plx_machine::idle::invalidate();
}

/// Whether a progress call is out. One at a time across the Requests list's chips and a title page's
/// tracker: the TV's connections are few, and the page showing the numbers is the only thing that
/// wants them.
static PROGRESS_OUT: AtomicBool = AtomicBool::new(false);

/// The right to make the one progress call; given back when dropped, whatever the call did.
struct ProgressSlot;

impl ProgressSlot {
    fn take() -> Option<Self> {
        PROGRESS_OUT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| Self)
    }
}

impl Drop for ProgressSlot {
    fn drop(&mut self) {
        PROGRESS_OUT.store(false, Ordering::Release);
    }
}

enum ProgressJob {
    Title(progress::Order),
    List { gen: u32, kind: Kind },
}

impl ProgressJob {
    fn answer(&self, blocked: Option<ProgressFail>) -> RequestsReply {
        match self {
            Self::Title(order) => {
                let outcome = blocked.map_or_else(|| progress_call(order), Err);
                RequestsReply::Progress { gen: order.gen, step: order.step, outcome }
            }
            Self::List { gen, kind } => {
                let result = blocked.map_or_else(|| progress_list(*kind), Err);
                RequestsReply::ProgressList { gen: *gen, kind: *kind, result }
            }
        }
    }
}

/// Progress uses a single slot; an occupied slot or failed spawn answers immediately.
fn progress_worker(job: ProgressJob, reply: Sender<RequestsReply>) {
    let Some(slot) = ProgressSlot::take() else {
        say(&reply, job.answer(Some(ProgressFail::Busy)));
        return;
    };
    let refused = job.answer(Some(ProgressFail::Failed));
    let back = reply.clone();
    let spawned = plx_base::task::spawn_small("progress", move || {
        let _slot = slot;
        say(&reply, job.answer(None));
    });
    if !spawned {
        say(&back, refused);
    }
}

fn progress_fail(error: SeerrError) -> ProgressFail {
    match error {
        SeerrError::NotSet => ProgressFail::NotSet,
        SeerrError::SignInRefused | SeerrError::Forbidden => ProgressFail::Refused,
        _ => ProgressFail::Failed,
    }
}

/// One step of a title's tracker: Seerr's title answer, or the title's own app (its record, its queue).
fn progress_call(order: &progress::Order) -> progress::Outcome {
    if order.step == progress::Step::Seerr {
        let link = Link::read(true).map_err(progress_fail)?;
        let seerr = Seerr::new(&link.base, &link.cookie, &link.plex_token, &CurlTransport);
        let view = seerr.progress(order.key.kind, order.key.tmdb);
        link.keep(seerr.cookie());
        return view.map(progress::Answer::Seerr).map_err(progress_fail);
    }
    let link = plx_plex::plex::session::peek().arr(progress::arr_of(order.key.kind));
    progress::run_arr(&link, &ArrCurl, order)
}

/// The whole queue of the app that keeps `kind`'s downloads.
fn progress_list(kind: plx_plex::plex::seerr::Kind) -> Result<Vec<progress::Live>, ProgressFail> {
    let link = plx_plex::plex::session::peek().arr(progress::arr_of(kind));
    progress::run_list(&link, &ArrCurl, kind)
}

/// Today's date in the TV's timezone, `YYYY-MM-DD`: the upper bound of a newest-first listing.
fn today() -> String {
    let [year, month, day] = i64::try_from(seerr_cache::now())
        .ok()
        .and_then(plx_platform::i18n::local_calendar_date)
        .unwrap_or([2100, 1, 1]);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A Requests page listening to the hub: its answers become the replies the page reads on its tick.
struct Listener {
    reply: Sender<RequestsReply>,
    art: Option<plx_plex::plex::ServerId>,
}

impl Sink for Listener {
    fn take(&self, piece: Piece) -> bool {
        let answer = match piece {
            Piece::Ready(rules) => RequestsReply::Ready { rules, art: self.art },
            Piece::Mine(mine) => RequestsReply::Mine { skip: 0, result: Ok((mine.items, mine.more)) },
            Piece::Shelf(shelf, page) => RequestsReply::Shelf { shelf, page: 1, result: Ok(page) },
            Piece::Lost(Part::Mine, error) => RequestsReply::Mine { skip: 0, result: Err(error) },
            Piece::Lost(Part::Shelf(shelf), error) => RequestsReply::Shelf { shelf, page: 1, result: Err(error) },
            Piece::Failed(error) => RequestsReply::Failed(error),
        };
        let kept = self.reply.send(answer).is_ok();
        plx_machine::idle::invalidate();
        kept
    }
}

/// What the held answers belong to: the Seerr address and the seated account.
fn key_now() -> Result<Key, SeerrError> {
    let link = plx_plex::plex::session::peek().seerr();
    if !link.is_ready() {
        return Err(SeerrError::NotSet);
    }
    #[cfg(all(feature = "hostsim", feature = "devtriggers", not(all(target_os = "linux", target_arch = "arm"))))]
    if sim_link::base().is_some() {
        return Ok(Key { base: link.url, account: sim_link::ACCOUNT.into() });
    }
    let user = plx_plex::plex::session::current().ok_or(SeerrError::NoPlexAccount)?;
    Ok(Key { base: link.url, account: user.uuid })
}

/// Open the hub for `key` and start the worker when it needs one.
fn open_hub(key: &Key, sink: Option<Box<dyn Sink>>) {
    if HUB.open(key, sink, seerr_cache::now()) == Work::Start && !plx_base::task::spawn_small("seerr-sync", sync) {
        HUB.abandon();
    }
}

/// The hub's one worker: read the saved copy once, refresh when the held answers are old, save, and
/// go round again when a request or a profile change came in meanwhile.
fn sync() {
    let paths = plx_base::paths::requests_cache_candidates();
    loop {
        let Some(key) = HUB.key() else {
            HUB.abandon();
            break;
        };
        if HUB.disk_unread() {
            let now = seerr_cache::now();
            HUB.disk_read(seerr_cache::load(&paths, now), now);
        }
        let ok = HUB.wants_refresh(seerr_cache::now()) && refresh_once(&key);
        let done = HUB.finish(&key, ok, seerr_cache::now());
        if let Some(snapshot) = done.save {
            seerr_cache::save(&paths, &snapshot);
        }
        if !done.again {
            break;
        }
    }
    plx_machine::idle::invalidate();
}

/// One refresh against Seerr; whether every row was read.
fn refresh_once(key: &Key) -> bool {
    let link = match Link::read(true) {
        Ok(link) => link,
        Err(error) => {
            HUB.failed(key, error);
            return false;
        }
    };
    if link.base != key.base || link.account != key.account {
        // The account or address changed since the page opened: the next open reads for the new one.
        return false;
    }
    let login = Login { base: &link.base, cookie: &link.cookie, plex_token: &link.plex_token };
    let (ok, cookie) = seerr_cache::refresh(&HUB, key, &login, &CurlTransport, seerr_cache::now());
    link.keep(cookie);
    ok
}

/// Forget the answers held and the copy on disk (the address changed, or Requests was switched off
/// or forgotten): they list what the viewer asked for.
pub(crate) fn forget_held_answers() {
    HUB.clear();
    let paths = plx_base::paths::requests_cache_candidates();
    let _ = plx_base::storage_worker::submit_retained(move || seerr_cache::purge(&paths));
}

/// The app went to the background: drop the held answers the copy on disk still matches.
pub(crate) fn release_held_answers() {
    HUB.release(seerr_cache::now());
    expire_request_cache();
}

fn expire_request_cache() -> bool {
    plx_base::storage_worker::submit(|| {
        let now = seerr_cache::now();
        HUB.expire(now);
        seerr_cache::expire(&plx_base::paths::requests_cache_candidates(), now);
        plx_platform::imgcache::prewarm();
    })
    .is_ok()
}

/// Not before this, so the warm-up never competes with starting the app.
const WARM_AFTER_MS: u32 = 8_000;
/// How often the warm-up looks for its moment.
const WARM_EVERY_MS: u32 = 1_000;
const CACHE_EXPIRY_EVERY_MS: u32 = 3600 * 1000;
/// Posters warmed ahead: the first row or two of the page, a screenful.
const WARM_ROWS: usize = 2;
const WARM_PER_ROW: usize = 6;
/// Frames a poster the cache keeps refusing is tried before it is left to load when drawn.
const WARM_TRIES: u32 = 120;

/// The Requests page prepared in the background (`Bridge::requests_warm`).
#[derive(Default)]
pub(super) struct RequestsWarm {
    /// The account and address answers were asked for: once each.
    started: Option<Key>,
    last_ms: u32,
    /// The answers the poster queue was made from.
    posters_from: u64,
    /// `(server, path, width, height)` of each poster still to warm, nearest first.
    posters: Vec<(u16, String, std::os::raw::c_int, std::os::raw::c_int)>,
    tries: u32,
    last_expiry_ms: Option<u32>,
}

/// Test connection on the Settings page: sign in and answer Seerr's version and the name it knows
/// the viewer as. A refused spawn drops the reply sender, which the page reads as "could not ask".
pub(crate) fn test_seerr(reply: Sender<Result<(plx_plex::plex::seerr::Status, plx_plex::plex::seerr::Me), SeerrError>>) {
    let _ = plx_base::task::spawn_small("seerr-test", move || {
        let answer = Link::read(false).and_then(|link| {
            let seerr = Seerr::new(&link.base, &link.cookie, &link.plex_token, &CurlTransport);
            let answer = seerr.status().and_then(|status| Ok((status, seerr.me()?)));
            link.keep(seerr.cookie());
            answer
        });
        let _ = reply.send(answer);
        plx_machine::idle::invalidate();
    });
}

/// Whether something is playing or starting to: the warm-up stays out of the way.
fn playback_busy(d: &Dispatcher<AppHost>) -> bool {
    d.nav.top_page().is_some_and(|page| matches!(page.arg, AppArg::Player))
        || player_overlay_up(d)
        || plx_media::player::claim_hold::active()
}

impl Bridge {
    pub(crate) fn requests_command(&mut self, command: RequestsCmd) {
        match command {
            RequestsCmd::Open(reply) => {
                let art = self.stores.watchlist.view().art_server();
                match key_now() {
                    Ok(key) => open_hub(&key, Some(Box::new(Listener { reply, art }))),
                    Err(error) => say(&reply, RequestsReply::Failed(error)),
                }
            }
            RequestsCmd::Refresh => {
                if let Ok(key) = key_now() {
                    open_hub(&key, None);
                }
            }
            RequestsCmd::Shelf { shelf, page, reply } => run(SeerrJob::Shelf { shelf, page }, reply),
            RequestsCmd::Mine { skip, reply } => run(SeerrJob::Mine { skip }, reply),
            RequestsCmd::Search { query, page, reply } => run(SeerrJob::Search { query, page }, reply),
            RequestsCmd::Detail { kind, tmdb, reply } => {
                let art = self.stores.watchlist.view().art_server();
                run(SeerrJob::Detail { kind, tmdb, art }, reply);
            }
            RequestsCmd::Stills { tmdb, seasons, reply } => run(SeerrJob::Stills { tmdb, seasons }, reply),
            RequestsCmd::Trailer { video, reply } => {
                // The launch waits for the system's answer: off the frame thread, like every Seerr call.
                let back = reply.clone();
                let spawned = plx_base::task::spawn_small("trailer", move || {
                    let opened = plx_platform::tv::youtube::watch(&video);
                    say(&reply, RequestsReply::Trailer { opened });
                });
                if !spawned {
                    say(&back, RequestsReply::Trailer { opened: false });
                }
            }
            RequestsCmd::Request { kind, tmdb, seasons, reply } => run(SeerrJob::Request { kind, tmdb, seasons }, reply),
            RequestsCmd::Browse { filter, sort, page, gen, reply } => run(SeerrJob::Browse { filter, sort, page, gen }, reply),
            RequestsCmd::Genres { reply } => run(SeerrJob::Genres, reply),
            RequestsCmd::Progress { order, reply } => progress_worker(ProgressJob::Title(order), reply),
            RequestsCmd::ProgressList { gen, kind, reply } => progress_worker(ProgressJob::List { gen, kind }, reply),
        }
    }

    /// Prepare the Requests page while the app is idle: once the app has settled, ask for the
    /// answers the page opens on, then warm the first posters one per frame. Never while something
    /// is playing or starting to.
    pub(crate) fn requests_warm(&mut self, d: &Dispatcher<AppHost>, tick: Tick) {
        self.warm_requests_posters();
        if tick.ms < WARM_AFTER_MS || tick.ms.wrapping_sub(self.requests_warm.last_ms) < WARM_EVERY_MS {
            return;
        }
        self.requests_warm.last_ms = tick.ms;
        // Defer disk I/O until startup has settled, even if Requests itself is never opened.
        if self.requests_warm.last_expiry_ms.is_none_or(|ms| tick.ms.wrapping_sub(ms) >= CACHE_EXPIRY_EVERY_MS)
            && expire_request_cache()
        {
            self.requests_warm.last_expiry_ms = Some(tick.ms);
        }
        let Ok(key) = key_now() else { return };
        if self.requests_warm.started.as_ref() == Some(&key) || playback_busy(d) {
            return;
        }
        open_hub(&key, None);
        self.requests_warm.started = Some(key);
    }

    /// Hand the poster cache the first posters of the held answers, the way the page will ask for
    /// them, so they are decoded before the page is opened.
    fn warm_requests_posters(&mut self) {
        if self.requests_warm.started.is_none() {
            return;
        }
        let generation = HUB.generation();
        if generation != self.requests_warm.posters_from {
            let Some(server) = self.stores.watchlist.view().art_server() else { return };
            let posters = HUB
                .first_posters(WARM_ROWS, WARM_PER_ROW, seerr_cache::now())
                .iter()
                .filter_map(|url| {
                    let facts = plx_ui::tile::TileFacts { src: server.raw(), thumb: url, ..Default::default() };
                    let (srv, path, w, h) = plx_ui::widgets::card_art_request(&plx_ui::widgets::Art::Poster(Some(facts)))?;
                    Some((srv, path.to_owned(), w, h))
                })
                .collect();
            let warm = &mut self.requests_warm;
            warm.posters_from = generation;
            warm.posters = posters;
            warm.tries = 0;
        }
        let warm = &mut self.requests_warm;
        let Some((srv, path, w, h)) = warm.posters.first() else { return };
        match plx_ui::tex::warm_ahead_on(*srv, path, *w, *h, false) {
            plx_ui::tex::Warm::Full if warm.tries + 1 < WARM_TRIES => warm.tries += 1,
            _ => {
                warm.posters.remove(0);
                warm.tries = 0;
            }
        }
    }
}
