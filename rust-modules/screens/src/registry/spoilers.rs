//! Spoiler protection on the screens: what stands in for what a covered episode gives away.
//!
//! One rule for every surface (tiles, the episode strip, the detail page, Up Next) lives in
//! [`plx_media::route::covers_spoiler`]: with the profile's switch on, an episode that is neither
//! watched nor started has its still blurred and its description replaced. The blur is the still
//! requested at [`plx_ui::widgets::SPOILER_RES`] (see there).

use plx_platform::i18n::msg;

/// Whether an episode in this state is covered right now (the seated profile's live switch).
pub fn protects(watched: bool, resume_ms: i64) -> bool {
    plx_media::route::spoiler_covers(watched, resume_ms)
}

/// What replaces a covered description: the prompt where OK reveals it, the plain note where
/// nothing on the screen does.
pub fn hidden_note(revealable: bool) -> &'static str {
    if revealable { msg::spoilers_hidden() } else { msg::spoilers_hidden_short() }
}

#[cfg(test)]
mod tests {
    use plx_media::route::covers_spoiler;

    #[test]
    fn only_unwatched_unstarted_episodes_are_covered_and_only_when_on() {
        assert!(covers_spoiler(true, false, 0));
        assert!(!covers_spoiler(false, false, 0), "off covers nothing");
        assert!(!covers_spoiler(true, true, 0), "a watched episode looks normal");
        assert!(!covers_spoiler(true, false, 90_000), "a started episode is not a spoiler");
    }
}
