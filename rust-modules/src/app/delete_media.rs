//! Runs confirmed media deletions off the frame thread and reports their outcome.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use plx_data::viewstate::{Scope, ViewAt};
use plx_plex::plex::delete_media::{self, DeleteError, DeleteOutcome, EpisodeParent, MediaKind, WatchlistOutcome};
use plx_plex::plex::ServerId;

/// Does an item menu on this server offer Delete? Also starts the background re-read of the
/// server's "Allow media deletion" setting when the stored answer is missing or old, so the next
/// menu follows the server; host tests stay off the network.
pub(super) fn offered(sid: ServerId) -> bool {
    #[cfg(not(test))]
    plx_plex::plex::serverinfo::refresh_deletion_if_due(sid);
    plx_plex::plex::serverinfo::delete_offered(sid)
}

struct Pending {
    sid: ServerId,
    rk: String,
    kind: MediaKind,
    answer: Receiver<Result<DeleteOutcome, DeleteError>>,
}

#[derive(Default)]
pub(super) struct Manager {
    pending: Option<Pending>,
}

/// A deletion the server confirmed (done now, or already done): what every list on screen needs
/// to follow it.
pub(super) struct Deleted {
    pub rk: String,
    pub kind: MediaKind,
    /// The show and season an episode sat in, when the server's record named them.
    pub parent: Option<EpisodeParent>,
    pub watched: bool,
}

impl Deleted {
    /// The item as the view-state ledger addresses it. A show takes its seasons and episodes with
    /// it; an episode also counts out of the show and season above it.
    pub(super) fn at(&self, sid: ServerId) -> ViewAt {
        let scope = match (self.kind, &self.parent) {
            (MediaKind::Show, _) => Scope::Show,
            (MediaKind::Episode, Some(p)) => Scope::Episode { show_rk: p.show_rk.clone(), index: p.season_index },
            _ => Scope::Leaf,
        };
        ViewAt { sid, rk: self.rk.clone(), scope }
    }
}

pub(super) struct Completion {
    pub sid: ServerId,
    /// `None` when nothing is known to have been deleted. The lists are still asked again, because
    /// a deletion the server never confirmed may have happened anyway.
    pub deleted: Option<Deleted>,
}

fn notice(text: &'static str) {
    plx_base::task::spawn_small("media deletion notice", move || {
        let _ = plx_platform::tv::toast::toast(text);
    });
}

/// What the person is told: never a status code, only what they can act on.
fn words(result: &Result<DeleteOutcome, DeleteError>, kind: MediaKind) -> &'static str {
    match result {
        Ok(outcome) => match outcome.watchlist {
            WatchlistOutcome::Failed if kind == MediaKind::Episode =>
                "Episode deleted. Could not confirm whether the show should leave Watchlist; check Plex.",
            WatchlistOutcome::Failed => "Media deleted. Watchlist cleanup could not be confirmed; check Plex.",
            WatchlistOutcome::Skipped => "Already deleted.",
            _ => "Media deleted.",
        },
        Err(DeleteError::NotAllowed) => "Deleting is not allowed. The Plex server owner has to turn on Allow media deletion.",
        Err(DeleteError::ServerProblem) => "The Plex server had a problem. Try again in a moment.",
        Err(DeleteError::InvalidItem) => "This item cannot be deleted.",
    }
}

impl Manager {
    pub(super) fn start(&mut self, sid: ServerId, rk: String, kind: MediaKind) {
        if self.pending.is_some() {
            notice("A deletion is already in progress.");
            return;
        }
        let Some(client) = plx_plex::plex::client_for(sid) else {
            notice("Cannot reach this Plex server. Nothing was deleted.");
            return;
        };
        let client_key = plx_plex::plex::ClientKey::of(client);
        let auth = super::bridge::Bridge::watchlist_input().auth().cloned();
        let key = rk.clone();
        let (tx, answer) = mpsc::channel();
        let started = plx_base::task::spawn_small("delete media", move || {
            let account = auth.as_ref().map(|auth| plx_plex::plex::account::AccountClient::new(&auth.client_id, Some(&auth.credential)));
            let outcome = delete_media::delete(client, client_key, account.as_ref(), &key, kind);
            let _ = tx.send(outcome);
            plx_machine::idle::invalidate();
        });
        if started {
            self.pending = Some(Pending { sid, rk, kind, answer });
            // The server removes the files before it answers, which on a spinning array takes a while.
            notice("Deleting...");
        } else {
            notice("Could not start deletion. Nothing was deleted.");
        }
    }

    /// A confirmed deletion carries its identity so every list can drop it at once. A refusal
    /// changed nothing and asks for nothing; any other failure only requests a refresh. Trying
    /// again is always allowed: a deletion that did happen reads as already deleted.
    pub(super) fn poll(&mut self) -> Option<Completion> {
        let pending = self.pending.as_ref()?;
        let result = match pending.answer.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DeleteError::ServerProblem),
        };
        let Pending { sid, rk, kind, .. } = self.pending.take()?;
        notice(words(&result, kind));
        match result {
            Ok(outcome) => Some(Completion {
                sid,
                deleted: Some(Deleted { rk, kind, parent: outcome.parent, watched: outcome.watched }),
            }),
            Err(DeleteError::ServerProblem) => Some(Completion { sid, deleted: None }),
            Err(DeleteError::NotAllowed | DeleteError::InvalidItem) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(result: Result<DeleteOutcome, DeleteError>) -> Option<Completion> {
        let (tx, answer) = mpsc::channel();
        tx.send(result).unwrap();
        let mut manager = Manager { pending: Some(Pending {
            sid: ServerId::from_raw(1), rk: "42".into(), kind: MediaKind::Show, answer,
        }) };
        manager.poll()
    }

    fn outcome(watchlist: WatchlistOutcome) -> DeleteOutcome {
        DeleteOutcome { watchlist, parent: None, watched: false }
    }

    #[test]
    fn a_confirmed_deletion_carries_what_the_lists_need_and_a_failure_carries_nothing() {
        for watchlist in [WatchlistOutcome::Removed, WatchlistOutcome::NotListed, WatchlistOutcome::Failed, WatchlistOutcome::Skipped] {
            let done = complete(Ok(outcome(watchlist))).unwrap().deleted.expect("confirmed");
            assert_eq!((done.rk.as_str(), done.kind), ("42", MediaKind::Show), "{watchlist:?}");
        }
        let episode = DeleteOutcome {
            watchlist: WatchlistOutcome::Preserved,
            parent: Some(EpisodeParent { show_rk: "7".into(), season_index: 2 }),
            watched: true,
        };
        let done = complete(Ok(episode.clone())).unwrap().deleted.unwrap();
        assert_eq!((done.parent, done.watched), (episode.parent, true));
        assert!(complete(Err(DeleteError::NotAllowed)).is_none(), "refused: nothing changed, nothing to refresh");
        assert!(complete(Err(DeleteError::InvalidItem)).is_none());
        let unsure = complete(Err(DeleteError::ServerProblem)).expect("the lists are asked again");
        assert!(unsure.deleted.is_none(), "and nothing is removed on a guess");
    }

    #[test]
    fn a_worker_that_died_without_answering_is_a_server_problem() {
        let (tx, answer) = mpsc::channel::<Result<DeleteOutcome, DeleteError>>();
        drop(tx);
        let mut manager = Manager { pending: Some(Pending {
            sid: ServerId::from_raw(1), rk: "42".into(), kind: MediaKind::Movie, answer,
        }) };
        assert!(manager.poll().is_some_and(|c| c.deleted.is_none()));
        assert!(manager.pending.is_none(), "and the next deletion may start");
    }

    #[test]
    fn each_failure_is_told_in_plain_words_never_as_a_status_code() {
        let told = |error| words(&Err(error), MediaKind::Movie);
        assert!(told(DeleteError::NotAllowed).contains("owner"), "who can fix it");
        assert!(told(DeleteError::ServerProblem).contains("Try again"));
        assert_eq!(told(DeleteError::InvalidItem), "This item cannot be deleted.");
        for error in [DeleteError::NotAllowed, DeleteError::ServerProblem, DeleteError::InvalidItem] {
            assert!(!told(error).chars().any(|c| c.is_ascii_digit()), "{error:?}: {}", told(error));
        }
    }

    #[test]
    fn already_deleted_is_a_success_in_words_and_in_effect() {
        assert_eq!(words(&Ok(outcome(WatchlistOutcome::Skipped)), MediaKind::Movie), "Already deleted.");
        assert!(complete(Ok(outcome(WatchlistOutcome::Skipped))).unwrap().deleted.is_some());
        assert_eq!(words(&Ok(outcome(WatchlistOutcome::Removed)), MediaKind::Movie), "Media deleted.");
        assert!(words(&Ok(outcome(WatchlistOutcome::Failed)), MediaKind::Episode).starts_with("Episode deleted."));
    }

    #[test]
    fn what_is_deleted_is_addressed_by_what_it_takes_with_it() {
        let sid = ServerId::from_raw(1);
        let parent = Some(EpisodeParent { show_rk: "7".into(), season_index: 2 });
        let at = |kind, parent| Deleted { rk: "42".into(), kind, parent, watched: false }.at(sid).scope;
        assert_eq!(at(MediaKind::Show, None), Scope::Show);
        assert_eq!(at(MediaKind::Movie, None), Scope::Leaf);
        assert_eq!(at(MediaKind::Episode, parent), Scope::Episode { show_rk: "7".into(), index: 2 });
        assert_eq!(at(MediaKind::Episode, None), Scope::Leaf, "no season known: the refresh settles the counts");
    }
}
