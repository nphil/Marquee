//! Runs confirmed media deletions off the frame thread and reports their outcome.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use plx_data::viewstate::{Scope, ViewAt};
use plx_data::pms::PmsMovie;
use plx_plex::plex::delete_media::{self, Cleanup, DeleteError, DeleteOutcome, EpisodeParent, Listed, MediaKind, Retried, Step, WatchlistOutcome};
use plx_plex::plex::session::ArrKind;
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
    /// The lists were already told the item is gone.
    applied: bool,
    answer: Receiver<Update>,
}

/// What the worker reports: first that the item is gone, later how the cleanup after it went.
enum Update {
    Gone { parent: Option<EpisodeParent>, watched: bool },
    Done(Result<DeleteOutcome, DeleteError>),
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

fn notice(text: impl Into<String>) {
    let text = text.into();
    plx_base::task::spawn_small("media deletion notice", move || {
        let _ = plx_platform::tv::toast::toast(&text);
    });
}

/// The app that holds a title of this kind: Radarr keeps movies, Sonarr shows.
fn arr_name(kind: MediaKind) -> &'static str {
    if kind == MediaKind::Movie { ArrKind::Radarr.name() } else { ArrKind::Sonarr.name() }
}

/// Which cleanup steps went wrong, in plain words: a step that failed (retryable) or was refused.
fn problems(watchlist: WatchlistOutcome, cleanup: Cleanup, kind: MediaKind) -> Option<String> {
    use plx_platform::i18n::msg;
    let arr = arr_name(kind);
    let failed = [(watchlist == WatchlistOutcome::Failed, "Watchlist"), (cleanup.arr == Step::Failed, arr), (cleanup.seerr == Step::Failed, "Seerr")];
    let failed: Vec<_> = failed.iter().filter(|(failed, _)| *failed).collect();
    match failed.as_slice() {
        [] => {}
        [(_, "Watchlist")] => return Some(msg::media_delete_watchlist_failed().to_owned()),
        [(_, name)] => return Some(msg::media_delete_step_failed_named(name)),
        _ => return Some(msg::media_delete_step_failed_many().to_owned()),
    }
    if cleanup.arr == Step::Refused {
        return Some(msg::media_delete_step_refused(arr));
    }
    (cleanup.seerr == Step::Refused).then(|| msg::media_delete_step_refused("Seerr"))
}

/// What the person is told: never a status code, only what they can act on. A deletion that
/// left the Watchlist alone says nothing about it.
fn words(result: &Result<DeleteOutcome, DeleteError>, kind: MediaKind) -> String {
    use plx_platform::i18n::msg;
    match result {
        Ok(outcome) => {
            if let Some(problem) = problems(outcome.watchlist, outcome.cleanup, kind) {
                return problem;
            }
            let arr = arr_name(kind);
            match (outcome.cleanup.arr, outcome.watchlist) {
                (Step::NotConnected, _) => msg::media_delete_arr_not_connected(arr),
                (Step::Unmatched, _) => msg::media_delete_arr_unmatched(arr),
                (_, WatchlistOutcome::Unknown) => msg::media_delete_show_unknown().to_owned(),
                _ if outcome.already_gone => msg::media_delete_already().to_owned(),
                _ => msg::media_delete_done().to_owned(),
            }
        }
        Err(DeleteError::NotAllowed) => msg::media_delete_not_allowed().to_owned(),
        Err(DeleteError::ServerProblem) => msg::media_delete_server_problem().to_owned(),
        Err(DeleteError::InvalidItem) => msg::media_delete_invalid().to_owned(),
    }
}

fn retry_words(retried: &Retried) -> String {
    problems(retried.watchlist, retried.cleanup, retried.kind)
        .unwrap_or_else(|| plx_platform::i18n::msg::media_delete_retry_done().to_owned())
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
            let gone = tx.clone();
            let outcome = delete_media::delete(client, client_key, account.as_ref(), &key, kind, listed.as_ref(), &mut |parent, watched| {
                let _ = gone.send(Update::Gone { parent: parent.cloned(), watched });
                plx_machine::idle::invalidate();
            });
            let _ = tx.send(Update::Done(outcome));
            plx_machine::idle::invalidate();
        });
        if started {
            self.pending = Some(Pending { sid, rk, kind, applied: false, answer });
            // The server removes the files before it answers, which on a spinning array takes a while.
            notice(msg::media_delete_working());
        } else {
            notice(msg::media_delete_not_started());
        }
    }

    /// The lists follow a deletion the moment the server has it gone, before any cleanup runs, so a
    /// slow or failing Radarr or Seerr never keeps the title on screen. A refusal changed nothing and
    /// asks for nothing; any other failure only requests a refresh. Trying again is always allowed: a
    /// deletion that did happen reads as already deleted.
    pub(super) fn poll(&mut self) -> Option<Completion> {
        let pending = self.pending.as_mut()?;
        let result = match pending.answer.try_recv() {
            Ok(Update::Gone { parent, watched }) => {
                pending.applied = true;
                let deleted = Deleted { rk: pending.rk.clone(), kind: pending.kind, parent, watched };
                return Some(Completion { sid: pending.sid, deleted: Some(deleted) });
            }
            Ok(Update::Done(result)) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DeleteError::ServerProblem),
        };
        let Pending { sid, rk, kind, applied, .. } = self.pending.take()?;
        notice(words(&result, kind));
        match result {
            Ok(_) if applied => None,
            Ok(outcome) => Some(Completion {
                sid,
                deleted: Some(Deleted { rk, kind, parent: outcome.parent, watched: outcome.watched }),
            }),
            Err(DeleteError::ServerProblem) => Some(Completion { sid, deleted: None }),
            Err(DeleteError::NotAllowed | DeleteError::InvalidItem) => None,
        }
    }
}

/// Run again only what failed after the last deletion (Settings > Finish clean-up) and say how it
/// went. The Plex delete is never repeated. One at a time.
pub(super) fn finish_cleanup() {
    use plx_platform::i18n::msg;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        notice(msg::media_delete_busy());
        return;
    }
    let auth = super::bridge::Bridge::watchlist_input().auth().cloned();
    let started = plx_base::task::spawn_small("finish media clean-up", move || {
        let account = auth.as_ref().map(|auth| plx_plex::plex::account::AccountClient::new(&auth.client_id, Some(&auth.credential)));
        let said = match delete_media::retry_pending(account.as_ref()) {
            Some(retried) => retry_words(&retried),
            None => msg::media_delete_retry_nothing().to_owned(),
        };
        RUNNING.store(false, Ordering::Release);
        notice(said);
        plx_machine::idle::invalidate();
    });
    if !started {
        RUNNING.store(false, Ordering::Release);
        notice(msg::media_delete_not_started());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(result: Result<DeleteOutcome, DeleteError>) -> Option<Completion> {
        let (tx, answer) = mpsc::channel();
        tx.send(Update::Done(result)).unwrap();
        let mut manager = Manager { pending: Some(Pending {
            sid: ServerId::from_raw(1), rk: "42".into(), kind: MediaKind::Show, applied: false, answer,
        }) };
        manager.poll()
    }

    fn outcome(watchlist: WatchlistOutcome) -> DeleteOutcome {
        DeleteOutcome { watchlist, parent: None, watched: false, already_gone: false, cleanup: Cleanup::default() }
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
            cleanup: Cleanup::PARTIAL,
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
        let (tx, answer) = mpsc::channel::<Update>();
        drop(tx);
        let mut manager = Manager { pending: Some(Pending {
            sid: ServerId::from_raw(1), rk: "42".into(), kind: MediaKind::Movie, applied: false, answer,
        }) };
        assert!(manager.poll().is_some_and(|c| c.deleted.is_none()));
        assert!(manager.pending.is_none(), "and the next deletion may start");
    }

    #[test]
    fn each_failure_is_told_in_plain_words_never_as_a_status_code() {
        let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
        let told = |error| words(&Err(error), MediaKind::Movie);
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
        let said = |watchlist, already_gone| words(&Ok(DeleteOutcome { already_gone, cleanup: Cleanup { arr: Step::Removed, seerr: Step::NotConnected }, ..outcome(watchlist) }), MediaKind::Movie);
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
            let mut texts: Vec<String> = [
                msg::media_delete_done(), msg::media_delete_already(), msg::media_delete_watchlist_failed(),
                msg::media_delete_show_unknown(), msg::media_delete_not_allowed(), msg::media_delete_server_problem(),
                msg::media_delete_invalid(), msg::media_delete_busy(), msg::media_delete_unreachable(),
                msg::media_delete_working(), msg::media_delete_not_started(), msg::media_delete_step_failed_many(),
                msg::media_delete_retry_done(), msg::media_delete_retry_nothing(),
            ].map(str::to_owned).into();
            texts.extend([
                msg::media_delete_step_failed_named("Sonarr"), msg::media_delete_step_refused("Sonarr"),
                msg::media_delete_arr_not_connected("Sonarr"), msg::media_delete_arr_unmatched("Sonarr"),
            ]);
            for text in texts.iter().map(String::as_str) {
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

    #[test]
    fn the_lists_follow_the_deletion_before_the_cleanup_is_done() {
        let (tx, answer) = mpsc::channel();
        let mut manager = Manager { pending: Some(Pending {
            sid: ServerId::from_raw(1), rk: "42".into(), kind: MediaKind::Movie, applied: false, answer,
        }) };
        assert!(manager.poll().is_none(), "nothing yet");
        tx.send(Update::Gone { parent: None, watched: true }).unwrap();
        let early = manager.poll().expect("the item is gone: the lists follow now");
        assert!(early.deleted.is_some_and(|d| d.watched));
        assert!(manager.pending.is_some(), "the cleanup is still running");
        let failed = Cleanup { arr: Step::Failed, seerr: Step::Failed };
        tx.send(Update::Done(Ok(DeleteOutcome { cleanup: failed, ..outcome(WatchlistOutcome::Removed) }))).unwrap();
        assert!(manager.poll().is_none(), "a failing cleanup never puts the title back or asks the lists again");
        assert!(manager.pending.is_none());
    }

    #[test]
    fn each_cleanup_step_is_told_alone_in_plain_words() {
        use plx_platform::i18n::msg;
        let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
        let said = |watchlist, arr, seerr, kind| words(&Ok(DeleteOutcome { cleanup: Cleanup { arr, seerr }, ..outcome(watchlist) }), kind);
        let (r, s) = (WatchlistOutcome::Removed, Step::Removed);
        assert_eq!(said(r, s, s, MediaKind::Movie), "Deleted.");
        assert!(said(r, Step::Failed, s, MediaKind::Movie).contains("Radarr"), "a movie's app is Radarr");
        assert!(said(r, s, Step::Failed, MediaKind::Show).contains("Seerr"));
        assert!(said(r, Step::Failed, s, MediaKind::Show).contains("Sonarr"), "a show's app is Sonarr");
        assert_eq!(said(r, Step::Failed, Step::Failed, MediaKind::Show), msg::media_delete_step_failed_many());
        assert_eq!(said(WatchlistOutcome::Failed, Step::Failed, s, MediaKind::Show), msg::media_delete_step_failed_many());
        assert!(said(r, Step::Refused, s, MediaKind::Movie).contains("Radarr"));
        assert!(said(r, Step::NotConnected, Step::NotConnected, MediaKind::Movie).contains("Radarr isn't set up"), "says Radarr was not asked");
        assert!(said(r, Step::Unmatched, s, MediaKind::Show).contains("Sonarr"));
        assert_eq!(said(r, Step::NotApplicable, Step::NotApplicable, MediaKind::Show), "Deleted.", "partial deletes leave the apps alone, silently");
        let retried = |watchlist, arr, seerr| retry_words(&Retried { kind: MediaKind::Movie, watchlist, cleanup: Cleanup { arr, seerr } });
        assert_eq!(retried(r, s, s), "Clean-up finished.");
        assert!(retried(r, Step::Failed, s).contains("Radarr"), "a retry that fails again says so");
    }
}
