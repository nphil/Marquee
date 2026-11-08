//! Runs confirmed media deletions off the frame thread and reports their outcome.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use plx_data::viewstate::{Scope, ViewAt};
use plx_data::pms::PmsMovie;
use plx_plex::plex::delete_media::{self, DeleteError, DeleteOutcome, EpisodeParent, Listed, MediaKind, WatchlistOutcome};
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

/// What the person is told: never a status code, only what they can act on. A deletion that
/// left the Watchlist alone says nothing about it.
fn words(result: &Result<DeleteOutcome, DeleteError>) -> &'static str {
    use plx_platform::i18n::msg;
    match result {
        Ok(outcome) => match outcome.watchlist {
            WatchlistOutcome::Failed => msg::media_delete_watchlist_failed(),
            WatchlistOutcome::Unknown => msg::media_delete_show_unknown(),
            _ if outcome.already_gone => msg::media_delete_already(),
            _ => msg::media_delete_done(),
        },
        Err(DeleteError::NotAllowed) => msg::media_delete_not_allowed(),
        Err(DeleteError::ServerProblem) => msg::media_delete_server_problem(),
        Err(DeleteError::InvalidItem) => msg::media_delete_invalid(),
    }
}

/// The Watchlist identity of the item the menu acted on, for a deletion whose item the server no
/// longer has: the guid of the movie or show (an episode's is its show's) and an episode's show key.
/// Only the row the menu was opened on carries it; the on-deck episode's menu has no row, and its
/// deletion reads the identity from the server's own record instead.
pub(super) fn listed(item: Option<&PmsMovie>, rk: &str, kind: MediaKind) -> Option<Listed> {
    use plx_data::watchlist::{guid_of, model::Kind};
    let item = item.filter(|item| item.rk == rk)?;
    match kind {
        MediaKind::Movie => Some(Listed { guid: guid_of(Kind::Movie, &item.guid?), show_rk: None }),
        MediaKind::Show => Some(Listed { guid: guid_of(Kind::Show, &item.guid?), show_rk: None }),
        MediaKind::Episode => Some(Listed {
            guid: guid_of(Kind::Show, &item.show_guid?),
            show_rk: Some(item.show_rk.clone()).filter(|show_rk| !show_rk.is_empty()),
        }),
    }
}

impl Manager {
    pub(super) fn start(&mut self, sid: ServerId, rk: String, kind: MediaKind, listed: Option<Listed>) {
        use plx_platform::i18n::msg;
        if self.pending.is_some() {
            notice(msg::media_delete_busy());
            return;
        }
        let Some(client) = plx_plex::plex::client_for(sid) else {
            notice(msg::media_delete_unreachable());
            return;
        };
        let client_key = plx_plex::plex::ClientKey::of(client);
        let auth = super::bridge::Bridge::watchlist_input().auth().cloned();
        let key = rk.clone();
        let (tx, answer) = mpsc::channel();
        let started = plx_base::task::spawn_small("delete media", move || {
            let account = auth.as_ref().map(|auth| plx_plex::plex::account::AccountClient::new(&auth.client_id, Some(&auth.credential)));
            let outcome = delete_media::delete(client, client_key, account.as_ref(), &key, kind, listed.as_ref());
            let _ = tx.send(outcome);
            plx_machine::idle::invalidate();
        });
        if started {
            self.pending = Some(Pending { sid, rk, kind, answer });
            // The server removes the files before it answers, which on a spinning array takes a while.
            notice(msg::media_delete_working());
        } else {
            notice(msg::media_delete_not_started());
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
        notice(words(&result));
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
        DeleteOutcome { watchlist, parent: None, watched: false, already_gone: false }
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
            already_gone: false,
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
        let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
        let told = |error| words(&Err(error));
        assert!(told(DeleteError::NotAllowed).contains("owner"), "who can fix it");
        assert!(told(DeleteError::ServerProblem).contains("Try again"));
        assert_eq!(told(DeleteError::InvalidItem), "This item can't be deleted.");
        for error in [DeleteError::NotAllowed, DeleteError::ServerProblem, DeleteError::InvalidItem] {
            assert!(!told(error).chars().any(|c| c.is_ascii_digit()), "{error:?}: {}", told(error));
        }
    }

    #[test]
    fn a_deletion_says_what_happened_to_the_item_and_to_the_watchlist() {
        let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
        let said = |watchlist, already_gone| words(&Ok(DeleteOutcome { already_gone, ..outcome(watchlist) }));
        for watchlist in [WatchlistOutcome::Removed, WatchlistOutcome::NotListed, WatchlistOutcome::Preserved] {
            assert_eq!(said(watchlist, false), "Deleted.", "{watchlist:?}");
            assert_eq!(said(watchlist, true), "Already deleted.", "{watchlist:?}: the Watchlist is cleaned without a word");
        }
        assert_eq!(said(WatchlistOutcome::Skipped, true), "Already deleted.");
        for already_gone in [false, true] {
            assert_eq!(said(WatchlistOutcome::Failed, already_gone), "Deleted, but it couldn't be removed from your Watchlist.");
            assert_eq!(said(WatchlistOutcome::Unknown, already_gone), "Episode deleted. Check your Watchlist for the show.");
        }
        assert!(complete(Ok(DeleteOutcome { already_gone: true, ..outcome(WatchlistOutcome::Removed) })).unwrap().deleted.is_some(),
            "already deleted is a success in effect too");
    }

    /// The toast wraps at about 40 characters a line and shows three lines (`clock_notice`).
    #[test]
    fn every_shipped_language_fits_every_deletion_toast() {
        use plx_platform::i18n::msg;
        const LIMIT: usize = 120;
        const LINE: usize = 40;
        const LINES: usize = 3;
        for language in plx_platform::i18n::SHIPPED {
            let _guard = plx_platform::i18n::language_on_this_thread_for_test(language);
            for text in [
                msg::media_delete_done(), msg::media_delete_already(), msg::media_delete_watchlist_failed(),
                msg::media_delete_show_unknown(), msg::media_delete_not_allowed(), msg::media_delete_server_problem(),
                msg::media_delete_invalid(), msg::media_delete_busy(), msg::media_delete_unreachable(),
                msg::media_delete_working(), msg::media_delete_not_started(),
            ] {
                let tag = language.tag();
                assert!(!text.trim().is_empty(), "{tag}: empty");
                let length = text.chars().count();
                assert!(length <= LIMIT, "{tag}: {length} characters, over {LIMIT}: {text}");
                let (mut lines, mut width) = (1, 0);
                for word in text.split_whitespace() {
                    let w = word.chars().count();
                    if width != 0 && width + 1 + w > LINE {
                        lines += 1;
                        width = w;
                    } else {
                        width += if width == 0 { w } else { 1 + w };
                    }
                }
                assert!(lines <= LINES, "{tag}: wraps to {lines} lines at {LINE}: {text}");
            }
        }
    }

    #[test]
    fn the_watchlist_identity_comes_from_the_row_the_menu_acted_on() {
        let hex = |n: u8| [n; 12];
        let id = |n: u8| format!("{n:02x}").repeat(12);
        let row = |rk: &str, guid, show_guid| PmsMovie { rk: rk.into(), show_rk: "7".into(), guid, show_guid, ..Default::default() };
        let listed = |item: &PmsMovie, rk, kind| listed(Some(item), rk, kind);
        assert_eq!(listed(&row("42", Some(hex(1)), Some(hex(2))), "42", MediaKind::Movie),
            Some(Listed { guid: format!("plex://movie/{}", id(1)), show_rk: None }));
        assert_eq!(listed(&row("42", Some(hex(1)), None), "42", MediaKind::Show),
            Some(Listed { guid: format!("plex://show/{}", id(1)), show_rk: None }));
        assert_eq!(listed(&row("42", Some(hex(1)), Some(hex(2))), "42", MediaKind::Episode),
            Some(Listed { guid: format!("plex://show/{}", id(2)), show_rk: Some("7".into()) }), "an episode names its show");
        let no_show_rk = PmsMovie { show_rk: String::new(), ..row("42", None, Some(hex(2))) };
        assert_eq!(listed(&no_show_rk, "42", MediaKind::Episode).map(|l| l.show_rk), Some(None));
        assert_eq!(listed(&row("42", None, None), "42", MediaKind::Movie), None, "a row without a catalog id has none");
        assert_eq!(listed(&row("42", Some(hex(1)), None), "43", MediaKind::Movie), None, "another item's row is not this one's");
        assert_eq!(super::listed(None, "42", MediaKind::Episode), None, "the on-deck episode's menu has no row");
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
