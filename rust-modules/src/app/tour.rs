//! When the first-run tour is offered.
//!
//! Once per profile, on the first quiet moment at Home: after sign-in, the consent question, the
//! profile picker and the Favorite-libraries screen have all gone, and nothing else is up. The
//! tour itself is `screens::tour`; the only thing remembered is the seated profile's own
//! `tour_seen` flag, written by `LoopReq::TourSeen` when the tour is finished or skipped.
//!
//! A run offers it to a profile at most once, so a failed save shows it again on the next
//! start rather than looping within this one. Controlled recordings and automated boots never
//! see it, like the consent question.

use plx_screens::family::SettingsPage;
use plx_screens::registry::AppArg;

use super::bridge::{any_surface_up, open_settings_at, top_settled_on, AppHost};

thread_local! {
    /// Frame-thread state; the loop has one, and a test builds its own [`TourPrompt`].
    static PROMPT: std::cell::RefCell<TourPrompt> = std::cell::RefCell::new(TourPrompt::default());
}

/// The frame loop's one call.
pub(crate) fn poll(pages: &mut plx_ui::dispatch::Dispatcher<AppHost>, controlled: bool) {
    PROMPT.with(|prompt| prompt.borrow_mut().poll(pages, controlled));
}

/// Who the tour is still owed to: the seated profile's identity while it has not seen the tour,
/// `None` once it has, or while nobody is seated. Reads the cached session, no IO.
fn owed_seat() -> Option<String> {
    use plx_plex::plex::session::{current_seat, peek, profile_tour_seen, Seat};
    let seat = current_seat();
    let Seat::Seated { user, stamp } = &seat else { return None };
    (!profile_tour_seen(&peek(), &seat)).then(|| format!("{stamp}/{user}"))
}

/// Remember that the seated profile finished or skipped the tour, off the frame thread and best
/// effort: with nobody seated there is nothing to write, and a refused write shows the tour once
/// more on a later start.
pub(crate) fn mark_seen() {
    use plx_plex::plex::session::{persist_profile_tour_seen, ProfileWrite};
    let Some(write) = ProfileWrite::capture() else { return };
    let _ = plx_base::storage_worker::submit_retained(move || {
        if !persist_profile_tour_seen(&write) {
            plx_base::eventlog::log("tour: seen flag not saved: the write was refused");
        }
    });
}

/// The profile this run has already offered the tour to.
#[derive(Default)]
pub(crate) struct TourPrompt {
    offered_to: Option<String>,
}

impl TourPrompt {
    /// One frame's look: cheap unless Home is settled with nothing over it.
    pub(crate) fn poll(&mut self, pages: &mut plx_ui::dispatch::Dispatcher<AppHost>, controlled: bool) {
        if controlled || any_surface_up(pages) || !top_settled_on(pages, &AppArg::Home) {
            return;
        }
        let owed = owed_seat();
        if self.decide(owed, crate::dev::any_trigger_present) {
            open_settings_at(pages, SettingsPage::Tour(true));
        }
    }

    /// Offer the tour to `owed` (the seated profile that has not seen it) unless this run already
    /// did, or an automated boot is driving the app. The decision is remembered even when the boot
    /// is automated, so the trigger directory is not read again every frame.
    fn decide(&mut self, owed: Option<String>, automated: impl FnOnce() -> bool) -> bool {
        let Some(seat) = owed else { return false };
        if self.offered_to.as_ref() == Some(&seat) {
            return false;
        }
        let offer = !automated();
        self.offered_to = Some(seat);
        offer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_is_offered_the_tour_once_a_run() {
        let mut prompt = TourPrompt::default();
        assert!(prompt.decide(Some("a/1".into()), || false));
        assert!(!prompt.decide(Some("a/1".into()), || false));
    }

    #[test]
    fn another_profile_is_offered_it_in_its_turn() {
        let mut prompt = TourPrompt::default();
        assert!(prompt.decide(Some("a/1".into()), || false));
        assert!(prompt.decide(Some("a/2".into()), || false));
    }

    #[test]
    fn nobody_seated_or_nothing_owed_offers_nothing() {
        let mut prompt = TourPrompt::default();
        assert!(!prompt.decide(None, || false));
        assert!(prompt.decide(Some("a/1".into()), || false));
    }

    #[test]
    fn an_automated_boot_is_never_offered_it_and_is_not_asked_twice() {
        let mut prompt = TourPrompt::default();
        assert!(!prompt.decide(Some("a/1".into()), || true));
        assert!(!prompt.decide(Some("a/1".into()), || panic!("asked again")));
    }
}
