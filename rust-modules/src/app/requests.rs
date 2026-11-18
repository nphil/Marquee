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
use plx_plex::plex::seerr::{CurlTransport, Seerr, SeerrError};
use plx_plex::plex::seerr_cache::{self, Hub, Key, Login, Part, Piece, Sink, Work};
use plx_plex::plex::session::SeerrChange;
use plx_screens::registry::{RequestsCmd, RequestsReply, TitlePage};
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
        let user = plx_plex::plex::session::current().ok_or(SeerrError::NoPlexAccount)?;
        let plex_token = plx_plex::plex::session::plex_tv_credential(&user).ok_or(SeerrError::NoPlexAccount)?;
        Ok(Self { cookie: link.session_for(&user.uuid).to_string(), base: link.url, plex_token, account: user.uuid })
    }

    /// Hand a session that changed during the calls to the session record.
    fn keep(&self, after: String) {
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

/// Run `work` against Seerr on a worker. A link that cannot be read, or a worker that cannot be
/// spawned, answers `fail(error)` so the page never waits on nothing.
fn run<W, F>(reply: Sender<RequestsReply>, fail: F, work: W)
where
    W: FnOnce(&Seerr<'_, CurlTransport>, &Sender<RequestsReply>) + Send + 'static,
    F: Fn(SeerrError) -> RequestsReply + Send + 'static,
{
    let refused = fail(SeerrError::Unreachable);
    let back = reply.clone();
    let spawned = plx_base::task::spawn_small("seerr", move || {
        match Link::read(true) {
            Ok(link) => {
                let seerr = Seerr::new(&link.base, &link.cookie, &link.plex_token, &CurlTransport);
                work(&seerr, &reply);
                link.keep(seerr.cookie());
            }
            Err(error) => {
                let _ = reply.send(fail(error));
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
            HUB.disk_read(seerr_cache::load(&paths), seerr_cache::now());
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
    let (ok, cookie) = seerr_cache::refresh(&HUB, key, &login, &CurlTransport);
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

/// Not before this, so the warm-up never competes with starting the app.
const WARM_AFTER_MS: u32 = 8_000;
/// How often the warm-up looks for its moment.
const WARM_EVERY_MS: u32 = 1_000;
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
            RequestsCmd::Shelf { shelf, page, reply } => run(
                reply,
                move |error| RequestsReply::Shelf { shelf, page, result: Err(error) },
                move |seerr, reply| say(reply, RequestsReply::Shelf { shelf, page, result: seerr.discover(shelf, page) }),
            ),
            RequestsCmd::Mine { skip, reply } => run(
                reply,
                move |error| RequestsReply::Mine { skip, result: Err(error) },
                move |seerr, reply| {
                    let known = HUB.snapshot();
                    let result = seerr_cache::mine_page(seerr, skip, known.as_ref()).map(|mine| (mine.items, mine.more));
                    say(reply, RequestsReply::Mine { skip, result });
                },
            ),
            RequestsCmd::Search { query, page, reply } => {
                let failed = query.clone();
                run(
                    reply,
                    move |error| RequestsReply::Search { query: failed.clone(), page, result: Err(error) },
                    move |seerr, reply| {
                        let result = seerr.search(&query, page);
                        say(reply, RequestsReply::Search { query, page, result });
                    },
                );
            }
            RequestsCmd::Detail { kind, tmdb, reply } => run(
                reply,
                move |error| RequestsReply::Detail { kind, tmdb, result: Err(error) },
                move |seerr, reply| {
                    let result = seerr.detail(kind, tmdb).and_then(|detail| {
                        let rules = seerr.rules()?;
                        let plex = detail.plex.as_ref().and_then(|found| {
                            plx_plex::plex::id_of_machine(&found.machine).map(|sid| (sid, found.rating_key.clone()))
                        });
                        Ok(TitlePage { detail, plex, rules })
                    });
                    say(reply, RequestsReply::Detail { kind, tmdb, result });
                },
            ),
            RequestsCmd::Request { kind, tmdb, seasons, reply } => run(
                reply,
                move |error| RequestsReply::Requested { kind, tmdb, result: Err(error) },
                move |seerr, reply| {
                    let result = seerr.rules().and_then(|rules| seerr.request(kind, tmdb, &seasons, rules));
                    // Asked now, or found already asked: every poster of the title says so at once.
                    if matches!(result, Ok(()) | Err(SeerrError::AlreadyRequested)) {
                        HUB.requested(kind, tmdb);
                    }
                    say(reply, RequestsReply::Requested { kind, tmdb, result });
                },
            ),
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
                .first_posters(WARM_ROWS, WARM_PER_ROW)
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
