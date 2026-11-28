//! The title page's words, built from plain inputs so the page's tests can read them without a frame.
use plx_platform::i18n::msg;
use plx_plex::plex::seerr::{Ask, Have, Info, Kind, Role, Season, Stage, VideoKind};

use crate::registry::TitlePage;

/// One labelled fact of the About block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Fact {
    pub label: &'static str,
    pub value: String,
}

pub(super) fn stage_word(stage: Stage) -> &'static str {
    match stage {
        Stage::Released => msg::requests_life_released(),
        Stage::Returning => msg::requests_life_returning(),
        Stage::Ended => msg::requests_life_ended(),
        Stage::Canceled => msg::requests_life_canceled(),
        Stage::InProduction => msg::requests_life_in_production(),
        Stage::Planned => msg::requests_life_planned(),
    }
}

/// What kind of video a tile on the Videos shelf is, as the line under its name.
pub(super) fn video_word(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Trailer => msg::browse_detail_trailer(),
        VideoKind::Teaser => msg::requests_video_teaser(),
        VideoKind::Clip => msg::requests_video_clip(),
        VideoKind::Featurette => msg::requests_video_featurette(),
        VideoKind::Other => msg::requests_video_extra(),
    }
}

/// What a person did on the title, as the line under their name.
pub(super) fn role_word(role: &Role) -> &str {
    match role {
        Role::Director => msg::requests_director(),
        Role::Creator => msg::requests_creator(),
        Role::Character(part) => part,
    }
}

/// A sum of dollars in the short form a facts grid wants: `$63M`, `$1.2B`.
pub(super) fn money(dollars: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    const B: u64 = 1_000_000_000;
    let tenths = |n: u64, unit: u64| {
        let t = n * 10 / unit;
        if t % 10 == 0 { format!("{}", t / 10) } else { format!("{}.{}", t / 10, t % 10) }
    };
    match dollars {
        d if d >= B => format!("${}B", tenths(d, B)),
        d if d >= M => format!("${}M", tenths(d, M)),
        d if d >= K => format!("${}K", tenths(d, K)),
        d => format!("${d}"),
    }
}

/// How long a film runs, or how much a series holds.
pub(super) fn length(page: &TitlePage) -> String {
    let info = &page.detail.info;
    match page.detail.title.kind {
        Kind::Movie if page.detail.minutes > 0 => {
            plx_ui::fmt::duration_minutes_in(plx_platform::i18n::current(), i64::from(page.detail.minutes))
        }
        Kind::Movie => String::new(),
        Kind::Tv => {
            let (seasons, episodes) = if info.seasons == 0 || info.episodes == 0 { counted(page) } else { (0, 0) };
            let seasons = if info.seasons > 0 { info.seasons } else { seasons };
            let episodes = if info.episodes > 0 { info.episodes } else { episodes };
            match (seasons, episodes) {
                (0, 0) => String::new(),
                (s, 0) => msg::browse_detail_seasons(i64::from(s)),
                (0, e) => msg::browse_detail_episodes(i64::from(e)),
                (s, e) => msg::browse_detail_extent(&msg::browse_detail_episodes(i64::from(e)), &msg::browse_detail_seasons(i64::from(s))),
            }
        }
    }
}

/// The seasons and episodes the request picker lists, for a series whose credits gave no totals.
fn counted(page: &TitlePage) -> (u32, u32) {
    announced(page).fold((0, 0), |(s, e), season| (s + 1, e + season.episodes))
}

/// The seasons a request could name, as the picker lists them.
pub(super) fn announced(page: &TitlePage) -> impl Iterator<Item = &Season> {
    let specials = page.rules.specials;
    page.detail.seasons.iter().filter(move |s| (s.number > 0 || specials) && s.episodes > 0)
}

/// The name in its own language, then up to three genres.
pub(super) fn identity(page: &TitlePage) -> [ &str; 4 ] {
    let info = &page.detail.info;
    let original = if info.original_name != page.detail.title.name { info.original_name.as_str() } else { "" };
    let genre = |i| page.detail.genres.get(i).map_or("", String::as_str);
    [original, genre(0), genre(1), genre(2)]
}

fn joined(names: &[String]) -> String {
    names.join(", ")
}

/// The About block's rows: a film's credits and money, a series' makers and run.
pub(super) fn about_facts(page: &TitlePage) -> Vec<Fact> {
    let info: &Info = &page.detail.info;
    let mut rows = Vec::new();
    let mut add = |label: &'static str, value: String| {
        if !value.is_empty() {
            rows.push(Fact { label, value });
        }
    };
    match page.detail.title.kind {
        Kind::Movie => {
            add(msg::requests_director(), joined(&info.directors));
            add(msg::requests_writer(), joined(&info.writers));
            add(msg::requests_studio(), joined(&info.studios));
            add(msg::requests_budget(), if info.budget > 0 { money(info.budget) } else { String::new() });
            add(msg::requests_revenue(), if info.revenue > 0 { money(info.revenue) } else { String::new() });
        }
        Kind::Tv => {
            add(msg::requests_creator(), joined(&info.creators));
            add(msg::requests_network(), joined(&info.networks));
            add(msg::requests_studio(), joined(&info.studios));
        }
    }
    add(msg::requests_status(), info.stage.map_or(String::new(), |s| stage_word(s).to_string()));
    add(msg::sort_facts_audience_rating(), if info.score > 0 { format!("{}%", info.score) } else { String::new() });
    rows
}

/// The seasons a request could name and the word standing beside one that cannot be picked.
pub(super) fn season_word(season: &Season) -> Option<&'static str> {
    use plx_plex::plex::seerr::Badge;
    let badge = |b| Some(super::super::cards::badge_word(b));
    match season.have {
        Have::All => badge(Badge::Available),
        Have::Partly => badge(Badge::PartlyAvailable),
        Have::Pending | Have::Processing => badge(Badge::Requested),
        _ if season.requested => badge(Badge::Requested),
        _ => None,
    }
}

/// A season's name under its poster.
pub(super) fn season_name(number: u32) -> String {
    if number == 0 { msg::requests_specials().to_string() } else { msg::requests_season(i64::from(number)) }
}

/// One action the hero offers, from plain inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act {
    Play,
    Request,
    /// A state with nothing to press: a label, never focused.
    Status,
    Trailer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Action {
    pub act: Act,
    pub label: &'static std::ffi::CStr,
    pub icon: plx_ui::icons::Icon,
    pub enabled: bool,
}

impl Action {
    pub(super) fn word(&self) -> &str {
        self.label.to_str().unwrap_or_default()
    }

    /// Whether the D-pad can land on it.
    pub(super) fn focusable(&self) -> bool {
        self.act != Act::Status
    }
}

/// The hero's actions, left to right: Play when Plex has the title, the request (or why there is
/// none), and the trailer when the title has one. `picked` and `sending` only gate Request.
pub(super) fn actions(page: &TitlePage, picked: &[u32], sending: bool) -> Vec<Action> {
    use plx_ui::icons::Icon;
    let ask = page.detail.ask(page.rules);
    let mut out = Vec::new();
    if page.plex.is_some() {
        out.push(Action { act: Act::Play, label: msg::requests_play_c(), icon: Icon::Play, enabled: true });
    }
    let partial = page.rules.partial;
    let state = |label, icon| Action { act: Act::Status, label, icon, enabled: false };
    match &ask {
        Ask::Movie => out.push(Action { act: Act::Request, label: msg::requests_request_c(), icon: Icon::Bookmark, enabled: !sending }),
        Ask::Seasons(_) => out.push(Action {
            act: Act::Request,
            label: msg::requests_request_c(),
            icon: Icon::Bookmark,
            enabled: !(sending || (partial && picked.is_empty())),
        }),
        Ask::Requested => out.push(state(msg::requests_already_c(), Icon::Clock)),
        // In Plex, Play already says so; otherwise Seerr alone knows it is there.
        Ask::Have if page.plex.is_none() => out.push(state(msg::requests_available_c(), Icon::Check)),
        Ask::Have => {}
        Ask::Blocked => out.push(state(msg::requests_unavailable_c(), Icon::Alert)),
        Ask::NotYet => out.push(state(msg::requests_not_yet_c(), Icon::Clock)),
    }
    if page.detail.info.trailer().is_some() {
        out.push(Action { act: Act::Trailer, label: msg::browse_detail_trailer_c(), icon: Icon::Trailer, enabled: true });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_reads_short() {
        assert_eq!(money(63_000_000), "$63M");
        assert_eq!(money(463_517_383), "$463.5M");
        assert_eq!(money(1_200_000_000), "$1.2B");
        assert_eq!(money(950), "$950");
        assert_eq!(money(12_000), "$12K");
    }
}
