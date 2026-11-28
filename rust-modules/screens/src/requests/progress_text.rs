//! The words of a request's progress, from a [`Snapshot`]: the status lines under a title's hero,
//! the short word on a season, and the chip on the viewer's own requests. Only what a source
//! reported is said; a part nothing reported is left out, never worded as a guess.
use super::cards::badge_word;
use plx_platform::i18n::msg;
use plx_plex::plex::progress::{self, Origin, SeasonRow, Snapshot, Stage, Trouble, Who};
use plx_plex::plex::seerr::{Badge, Kind};
use std::borrow::Cow;

/// What a title's page says about its progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lines {
    /// The stage, with the percent while bytes are in play: "Downloading · 42%".
    pub headline: String,
    /// The time left, the download client and the library count, as far as they are known.
    pub detail: Option<String>,
    /// Where this came from, how old it is, or why it is missing.
    pub note: Option<String>,
}

fn join(parts: Vec<String>) -> Option<String> {
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn percent(n: u8) -> String {
    msg::requests_progress_percent(i64::from(n))
}

/// "About 12 min left"; under a minute, "Almost done".
pub(super) fn eta(secs: u32) -> String {
    if secs < 60 {
        return msg::requests_progress_eta_soon().to_string();
    }
    let minutes = secs.div_ceil(60);
    match (minutes / 60, minutes % 60) {
        (0, m) => msg::requests_progress_eta_minutes(i64::from(m)),
        (h, 0) => msg::requests_progress_eta_whole_hours(i64::from(h)),
        (h, m) => msg::requests_progress_eta_hours(i64::from(h), i64::from(m)),
    }
}

fn library(have: u32, total: u32) -> String {
    msg::requests_progress_library_count(i64::from(have), i64::from(total))
}

/// The short word for a stage: what a season or a poster says.
pub(super) fn short(stage: Stage, done: Option<u8>) -> Cow<'static, str> {
    let word = match stage {
        Stage::Downloading => match done {
            Some(n) => return Cow::Owned(msg::requests_progress_chip_downloading(i64::from(n))),
            None => badge_word(Badge::Downloading),
        },
        Stage::Finishing => msg::requests_progress_chip_finishing(),
        Stage::Importing => msg::requests_progress_chip_importing(),
        Stage::Imported => msg::requests_progress_chip_imported(),
        Stage::Queued => msg::requests_progress_chip_queued(),
        Stage::Paused => msg::requests_progress_chip_paused(),
        Stage::Attention => msg::requests_progress_chip_attention(),
        Stage::Requested => badge_word(Badge::Requested),
        Stage::Approved => badge_word(Badge::Approved),
        Stage::Partial => badge_word(Badge::PartlyAvailable),
        Stage::Failed => badge_word(Badge::Failed),
        Stage::Declined => badge_word(Badge::Declined),
        Stage::Available => badge_word(Badge::Available),
    };
    Cow::Borrowed(word)
}

/// The full sentence for a stage, as the page's status line says it.
fn long(stage: Stage, kind: Kind) -> String {
    match stage {
        Stage::Requested => msg::requests_progress_stage_requested().to_string(),
        Stage::Approved => msg::requests_progress_stage_approved().to_string(),
        Stage::Queued => msg::requests_progress_stage_queued().to_string(),
        Stage::Downloading => msg::requests_progress_stage_downloading().to_string(),
        Stage::Paused => msg::requests_progress_stage_paused().to_string(),
        Stage::Finishing => msg::requests_progress_stage_finishing().to_string(),
        Stage::Importing => msg::requests_progress_stage_importing().to_string(),
        Stage::Imported => msg::requests_progress_stage_imported().to_string(),
        Stage::Partial => msg::requests_progress_stage_partial().to_string(),
        Stage::Attention => msg::requests_progress_stage_attention(progress::arr_of(kind).name()),
        Stage::Failed => msg::requests_progress_stage_failed().to_string(),
        Stage::Declined => msg::requests_progress_stage_declined().to_string(),
        Stage::Available => msg::requests_progress_stage_available().to_string(),
    }
}

fn who(who: Who) -> &'static str {
    match who {
        Who::Seerr => "Seerr",
        Who::Arr(app) => app.name(),
    }
}

/// The line under the status: why the numbers may be missing or old, else where they came from.
fn note(snap: &Snapshot, kind: Kind) -> Option<String> {
    Some(match (snap.trouble, snap.from) {
        (Some(Trouble::Refused(w)), _) => msg::requests_progress_note_refused(who(w)),
        (Some(Trouble::Unreachable(w)), _) => msg::requests_progress_note_offline(who(w)),
        _ if snap.not_sent => msg::requests_progress_not_sent(progress::arr_of(kind).name()),
        _ if snap.stale => match snap.age_s? {
            s if s < 90 => msg::requests_progress_note_age(i64::from(s)),
            s => msg::requests_progress_note_age_min(i64::from(s / 60)),
        },
        (_, Some(Origin::Seerr)) => msg::requests_progress_note_seerr().to_string(),
        (_, Some(Origin::Arr(app))) => msg::requests_progress_note_live(app.name()),
        _ => return None,
    })
}

/// The status lines of a title's page; `None` when there is nothing to say (nothing asked for, or
/// already ready to watch).
pub(super) fn lines(snap: &Snapshot, kind: Kind) -> Option<Lines> {
    let stage = snap.stage.filter(|s| *s != Stage::Available)?;
    let mut headline = long(stage, kind);
    if matches!(stage, Stage::Downloading | Stage::Finishing | Stage::Paused) {
        if let Some(n) = snap.percent {
            headline = format!("{headline} · {}", percent(n));
        }
    }
    let mut detail = Vec::new();
    if stage == Stage::Downloading {
        detail.extend(snap.eta_s.map(eta));
    }
    if let Some(client) = snap.client.as_deref().filter(|_| matches!(stage, Stage::Downloading | Stage::Finishing | Stage::Paused | Stage::Queued)) {
        detail.push(msg::requests_progress_via_client(client));
    }
    if let Some((have, total)) = snap.library {
        detail.push(library(have, total));
    }
    Some(Lines { headline, detail: join(detail), note: note(snap, kind) })
}

/// A season's short word: "Downloading 75%", "Importing", "Ready to watch".
pub(super) fn season_word(row: &SeasonRow) -> String {
    short(row.stage, row.percent).into_owned()
}

/// What stands under a season's word: the time left and the library count.
pub(super) fn season_detail(row: &SeasonRow) -> Option<String> {
    let mut parts = Vec::new();
    if row.stage == Stage::Downloading {
        parts.extend(row.eta_s.map(eta));
    }
    if let Some((have, total)) = row.have.filter(|(_, total)| *total > 0) {
        parts.push(library(have, total));
    }
    join(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_plex::plex::session::ArrKind;
    use plx_base::fontcov::advances::ShippedMeasure;
    use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    fn snap(stage: Option<Stage>) -> Snapshot {
        Snapshot {
            loading: false,
            stage,
            percent: None,
            eta_s: None,
            client: None,
            library: None,
            seasons: Vec::new(),
            from: Some(Origin::Arr(ArrKind::Radarr)),
            age_s: None,
            stale: false,
            not_sent: false,
            trouble: None,
        }
    }

    #[test]
    fn a_running_download_says_its_percent_its_time_its_client_and_where_that_came_from() {
        let live = Snapshot { percent: Some(42), eta_s: Some(12 * 60), client: Some("NZBGet".into()), ..snap(Some(Stage::Downloading)) };
        let got = lines(&live, Kind::Movie).unwrap();
        assert!(got.headline.contains("42"), "{}", got.headline);
        let detail = got.detail.unwrap();
        assert!(detail.contains("12") && detail.contains("NZBGet"), "{detail}");
        assert!(got.note.unwrap().contains("Radarr"));
    }

    #[test]
    fn nothing_is_said_of_a_title_that_is_ready_or_not_asked_for() {
        assert_eq!(lines(&snap(None), Kind::Movie), None);
        assert_eq!(lines(&snap(Some(Stage::Available)), Kind::Movie), None);
    }

    #[test]
    fn a_stage_without_numbers_says_no_numbers() {
        let got = lines(&snap(Some(Stage::Approved)), Kind::Movie).unwrap();
        assert!(!got.headline.contains('%') && got.detail.is_none());
    }

    #[test]
    fn what_could_not_be_reached_is_named_and_an_old_answer_says_its_age() {
        let off = Snapshot { trouble: Some(Trouble::Unreachable(Who::Arr(ArrKind::Sonarr))), ..snap(Some(Stage::Approved)) };
        assert!(lines(&off, Kind::Tv).unwrap().note.unwrap().contains("Sonarr"));
        let old = Snapshot { stale: true, age_s: Some(30), ..snap(Some(Stage::Downloading)) };
        assert!(lines(&old, Kind::Movie).unwrap().note.unwrap().contains("30"));
        let older = Snapshot { stale: true, age_s: Some(300), ..snap(Some(Stage::Downloading)) };
        assert!(lines(&older, Kind::Movie).unwrap().note.unwrap().contains('5'));
    }

    #[test]
    fn a_time_left_is_rounded_up_to_the_minute_and_split_into_hours_past_one() {
        assert_ne!(eta(30), eta(60), "under a minute is its own word");
        assert_eq!(eta(61), eta(120), "61 s is two minutes");
        assert!(eta(3 * 3600).contains('3') && !eta(3 * 3600).contains("30"));
        assert!(eta(3600 + 600).contains('1') && eta(3600 + 600).contains("10"));
    }

    #[test]
    fn every_progress_chip_fits_a_poster_corner_in_all_shipped_languages() {
        let stages = [
            Stage::Queued, Stage::Downloading, Stage::Paused, Stage::Finishing,
            Stage::Importing, Stage::Imported, Stage::Attention,
        ];
        let room = plx_ui::consts::CARD_W - 2.0 * plx_ui::theme::space::SM;
        for language in SHIPPED {
            let _guard = language_on_this_thread_for_test(language);
            for stage in stages {
                let word = short(stage, Some(100));
                let w = plx_ui::widgets::badge_w(&word, None, &ShippedMeasure);
                assert!(w <= room, "{}: {word:?} is {w}px in a {room}px corner", language.tag());
            }
        }
    }
}
