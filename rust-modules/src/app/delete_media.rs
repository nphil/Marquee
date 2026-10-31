//! Runs confirmed media deletions off the frame thread and reports their outcome.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use plx_plex::plex::delete_media::{self, DeleteError, DeleteOutcome, MediaKind, WatchlistOutcome};
use plx_plex::plex::ServerId;

struct Pending {
    sid: ServerId,
    rk: String,
    kind: MediaKind,
    answer: Receiver<Result<DeleteOutcome, DeleteError>>,
}

#[derive(Default)]
pub(super) struct Manager {
    pending: Option<Pending>,
    uncertain: Vec<(ServerId, String)>,
}

pub(super) struct Completion {
    pub sid: ServerId,
    pub deleted: Option<(String, MediaKind)>,
}

fn notice(text: &'static str) {
    plx_base::task::spawn_small("media deletion notice", move || {
        let _ = plx_platform::tv::toast::toast(text);
    });
}

impl Manager {
    pub(super) fn start(&mut self, sid: ServerId, rk: String, kind: MediaKind) {
        if self.pending.is_some() {
            notice("A deletion is already in progress.");
            return;
        }
        if self.uncertain.iter().any(|(s, key)| *s == sid && *key == rk) {
            notice("Check Plex, then restart Marquee before trying again.");
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
        } else {
            notice("Could not start deletion. Nothing was deleted.");
        }
    }

    /// A confirmed deletion carries its identity; other outcomes only request a refresh.
    pub(super) fn poll(&mut self) -> Option<Completion> {
        let pending = self.pending.as_ref()?;
        let result = match pending.answer.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DeleteError::Uncertain),
        };
        let Pending { sid, rk, kind, .. } = self.pending.take()?;
        match result {
            Ok(outcome) => {
                if outcome.watchlist == WatchlistOutcome::Failed {
                    if kind == MediaKind::Episode {
                        notice("Episode deleted. Could not confirm whether the show should leave Watchlist; check Plex.");
                    } else {
                        notice("Media deleted. Watchlist cleanup could not be confirmed; check Plex.");
                    }
                } else {
                    notice("Media deleted.");
                }
                Some(Completion { sid, deleted: Some((rk, kind)) })
            }
            Err(DeleteError::Uncertain) => {
                self.uncertain.push((sid, rk));
                notice("Deletion not confirmed. Check Plex, then restart Marquee before retrying.");
                Some(Completion { sid, deleted: None })
            }
            Err(DeleteError::Denied) => {
                notice("Plex does not allow this account to delete media.");
                None
            }
            Err(_) => {
                notice("Deletion failed or was incomplete. Check Plex before trying again.");
                Some(Completion { sid, deleted: None })
            }
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
        }), ..Default::default() };
        manager.poll()
    }

    #[test]
    fn delete_membership_only_confirmed_outcomes_carry_identity() {
        let confirmed = complete(Ok(DeleteOutcome { watchlist: WatchlistOutcome::Failed })).unwrap();
        assert_eq!(confirmed.deleted, Some(("42".into(), MediaKind::Show)));
        assert!(complete(Err(DeleteError::Denied)).is_none());
        assert!(complete(Err(DeleteError::Rejected(403))).unwrap().deleted.is_none());
        assert!(complete(Err(DeleteError::Uncertain)).unwrap().deleted.is_none());
    }
}
