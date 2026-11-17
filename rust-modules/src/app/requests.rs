//! **The Requests area, application side**: the Seerr calls the Requests page and a title's page
//! ask for.
//!
//! A child of `bridge`, like `watchlist`. Each command runs on its own short-lived worker, reads
//! the Seerr link and the seated Plex account when it starts, and answers down the channel the
//! command carries; nothing here blocks the frame. The Seerr session cookie is the only thing that
//! outlives a call: a worker that signed in again hands the new one to the session record.
//!
//! A request is never sent blind: [`Seerr::request`] reads the title's status first and answers
//! `AlreadyRequested` when Seerr (or Pulsarr, which asks for every Watchlist add) already has it.
use super::*;
use plx_plex::plex::seerr::{Badge, CurlTransport, Seerr, SeerrError, Shelf};
use plx_plex::plex::session::SeerrChange;
use plx_screens::registry::{AskedTitle, RequestsCmd, RequestsReply, TitlePage};
use std::sync::mpsc::Sender;

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

/// The viewer's requests after `skip`, each with its title read for the poster and name.
fn mine(seerr: &Seerr<'_, CurlTransport>, skip: u32) -> Result<(Vec<AskedTitle>, bool), SeerrError> {
    let me = seerr.me()?;
    let (requests, more) = seerr.my_requests(me.id, skip)?;
    let mut titles = Vec::with_capacity(requests.len());
    for request in requests {
        // A title Seerr cannot read any more is left out rather than failing the whole row.
        if let Ok(detail) = seerr.detail(request.kind, request.tmdb) {
            titles.push(AskedTitle { title: detail.title, badge: request.badge.or(Some(Badge::Requested)) });
        }
    }
    Ok((titles, more))
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
impl Bridge {
    pub(crate) fn requests_command(&mut self, command: RequestsCmd) {
        match command {
            RequestsCmd::Open(reply) => {
                let art = self.stores.watchlist.view().art_server();
                run(reply, RequestsReply::Failed, move |seerr, reply| {
                    let rules = match seerr.rules() {
                        Ok(rules) => rules,
                        Err(error) => return say(reply, RequestsReply::Failed(error)),
                    };
                    say(reply, RequestsReply::Ready { rules, art });
                    say(reply, RequestsReply::Mine { skip: 0, result: mine(seerr, 0) });
                    for shelf in Shelf::ALL {
                        say(reply, RequestsReply::Shelf { shelf, page: 1, result: seerr.discover(shelf, 1) });
                    }
                });
            }
            RequestsCmd::Shelf { shelf, page, reply } => run(
                reply,
                move |error| RequestsReply::Shelf { shelf, page, result: Err(error) },
                move |seerr, reply| say(reply, RequestsReply::Shelf { shelf, page, result: seerr.discover(shelf, page) }),
            ),
            RequestsCmd::Mine { skip, reply } => run(
                reply,
                move |error| RequestsReply::Mine { skip, result: Err(error) },
                move |seerr, reply| say(reply, RequestsReply::Mine { skip, result: mine(seerr, skip) }),
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
                    say(reply, RequestsReply::Requested { kind, tmdb, result });
                },
            ),
        }
    }
}
