//! Movie filters for Requests. The bar keeps its values visible while a choice panel is open.
//! The panel is a D-pad grid, while the existing TV keyboard optionally searches Seerr for themes.
use super::*;
use plx_plex::plex::seerr::{Genre, Keyword, Page, SeerrError};
use plx_platform::i18n::msg;
use plx_ui::icons::Icon;
use plx_ui::theme::{size, space};
use plx_ui::text_view::TextView;
use std::ffi::CString;

pub(super) const GENRE: u32 = 2;
pub(super) const YEARS: u32 = 3;
pub(super) const THEMES: u32 = 4;
pub(super) const LANGUAGE: u32 = 5;
pub(super) const RUNTIME: u32 = 6;
pub(super) const RESET: u32 = 7;
pub(super) const GROUP: GroupId = GroupId(0x5251_0001);
pub(super) const NOTE_GROUP: GroupId = GroupId(0x5251_0002);
pub(super) const CHOICE_GROUP: GroupId = GroupId(0x5251_0003);
pub(super) const CHIP_BASE: u32 = 0x2000_0000;
pub(super) const COLS: usize = 4;
const VISIBLE_ROWS: usize = 4;
const OLDEST: u16 = 1890;
const GAP: f32 = space::SM;
pub(super) const HEIGHT: f32 = plx_ui::widgets::StatusOverlay::CTRL_H + space::SM;
const TITLE_H: f32 = space::XL;

pub(super) fn is_bar(elem: u32) -> bool {
    (GENRE..=RESET).contains(&elem)
}

pub(super) fn is_chip(elem: u32) -> bool {
    (CHIP_BASE..plx_ui::dispatch::STRIP_BASE).contains(&elem)
}

/// What a control sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    Genre,
    Years,
    Themes,
    Language,
    Runtime,
    Sort,
}

impl Pick {
    const FILTERS: [Self; 5] = [Self::Genre, Self::Years, Self::Themes, Self::Language, Self::Runtime];

    pub(super) fn elem(self) -> u32 {
        match self {
            Self::Genre => GENRE,
            Self::Years => YEARS,
            Self::Themes => THEMES,
            Self::Language => LANGUAGE,
            Self::Runtime => RUNTIME,
            Self::Sort => matches::SORT,
        }
    }

    pub(super) fn of(elem: u32) -> Option<Self> {
        Self::FILTERS.into_iter().chain([Self::Sort]).find(|p| p.elem() == elem)
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Genre => msg::requests_filter_genre(),
            Self::Years => msg::requests_filter_years(),
            Self::Themes => msg::requests_filter_themes(),
            Self::Language => msg::requests_filter_language(),
            Self::Runtime => msg::requests_filter_runtime(),
            Self::Sort => msg::requests_matches_sort(),
        }
    }
}

type Years = (Option<u16>, Option<u16>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bound {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Stage {
    #[default]
    Main,
    Range,
    Decades(Bound),
    Year(Bound, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Genre(Option<u32>),
    Years(Years),
    Range,
    ApplyRange,
    Bound(Bound),
    Decade(Bound, u16),
    Year(Bound, Option<u16>),
    Theme(u32),
    FindTheme,
    ClearThemes,
    ThemePage(u32),
    Language(Option<&'static str>),
    Runtime(Option<u16>, Option<u16>),
    Sort(Sort),
    RetryGenres,
    Note(Note),
}

pub(super) struct Choice {
    pub(super) label: CString,
    pub(super) value: Value,
    pub(super) selected: bool,
}

pub(super) struct Chooser {
    pub(super) pick: Pick,
    pub(super) stage: Stage,
    pub(super) choices: Vec<Choice>,
    pub(super) start: usize,
    /// Custom changes are not sent until Apply.
    pub(super) years: Years,
}

impl Chooser {
    pub(super) fn title(&self) -> &'static std::ffi::CStr {
        match self.stage {
            Stage::Range => msg::requests_filter_custom_years_c(),
            Stage::Decades(Bound::Start) | Stage::Year(Bound::Start, _) => msg::requests_filter_start_year_c(),
            Stage::Decades(Bound::End) | Stage::Year(Bound::End, _) => msg::requests_filter_end_year_c(),
            Stage::Main => match self.pick {
                Pick::Genre => msg::requests_filter_genre_c(),
                Pick::Years => msg::requests_filter_years_c(),
                Pick::Themes => msg::requests_filter_themes_c(),
                Pick::Language => msg::requests_filter_language_c(),
                Pick::Runtime => msg::requests_filter_runtime_c(),
                Pick::Sort => msg::requests_matches_sort_c(),
            },
        }
    }

    pub(super) fn len(&self) -> usize {
        self.choices.len() - usize::from(self.choices.last().is_some_and(|c| matches!(c.value, Value::Note(_))))
    }

    pub(super) fn height(&self) -> f32 {
        let rows = self.choices.len().div_ceil(COLS).min(VISIBLE_ROWS);
        TITLE_H + rows as f32 * (HEIGHT + GAP)
    }

    pub(super) fn visible(&self, focus: Option<usize>) -> std::ops::Range<usize> {
        let max_start = self.choices.len().div_ceil(COLS).saturating_sub(VISIBLE_ROWS);
        let first = (focus.unwrap_or(self.start) / COLS).saturating_sub(1).min(max_start);
        first * COLS..((first + VISIBLE_ROWS) * COLS).min(self.choices.len())
    }

    pub(super) fn rect(&self, focus: Option<usize>, n: usize, section: Rect) -> Option<Rect> {
        let visible = self.visible(focus);
        if !visible.contains(&n) { return None; }
        let row = (n - visible.start) / COLS;
        let w = (SCR_W - 2.0 * plx_ui::consts::MARGIN_X - (COLS - 1) as f32 * GAP) / COLS as f32;
        let y = section.y + TITLE_H + row as f32 * (HEIGHT + GAP);
        Some(Rect::new(plx_ui::consts::MARGIN_X + (n % COLS) as f32 * (w + GAP), y, w, HEIGHT))
    }

    pub(super) fn step(&self, at: usize, dir: Dir) -> Option<usize> {
        let len = self.len();
        let target = match dir {
            Dir::Left if at % COLS != 0 => at.checked_sub(1)?,
            Dir::Right if at % COLS + 1 < COLS => at + 1,
            Dir::Up => at.checked_sub(COLS)?,
            Dir::Down if (at / COLS + 1) * COLS < len => (at + COLS).min(len - 1),
            _ => return None,
        };
        (target < len).then_some(target)
    }
}

/// Where the genres Seerr offers stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Genres {
    Unknown,
    Asked,
    Loaded(Vec<Genre>),
    Failed,
}

/// The first choices don't require typing. Everything else is searchable on Seerr.
pub(super) const MOODS: [(u32, fn() -> &'static str); 27] = {
    use plx_platform::i18n::msg;
    [
        (10051, msg::requests_mood_heist),
        (10123, msg::requests_mood_dark_comedy),
        (207268, msg::requests_mood_neo_noir),
        (9807, msg::requests_mood_film_noir),
        (4565, msg::requests_mood_dystopia),
        (12190, msg::requests_mood_cyberpunk),
        (4379, msg::requests_mood_time_travel),
        (9672, msg::requests_mood_true_story),
        (10683, msg::requests_mood_coming_of_age),
        (4458, msg::requests_mood_post_apocalyptic),
        (12565, msg::requests_mood_psychological_thriller),
        (12339, msg::requests_mood_slasher),
        (7312, msg::requests_mood_road_trip),
        (10349, msg::requests_mood_survival),
        (12570, msg::requests_mood_whodunit),
        (8201, msg::requests_mood_satire),
        (9715, msg::requests_mood_superhero),
        (9748, msg::requests_mood_revenge),
        (167541, msg::requests_mood_buddy_comedy),
        (779, msg::requests_mood_martial_arts),
        (33519, msg::requests_mood_courtroom),
        (470, msg::requests_mood_spy),
        (12377, msg::requests_mood_zombie),
        (3358, msg::requests_mood_haunted_house),
        (14909, msg::requests_mood_alien_invasion),
        (207317, msg::requests_mood_christmas),
        (163053, msg::requests_mood_found_footage),
    ]
};

pub(super) struct Pill {
    pub(super) elem: u32,
    pub(super) label: CString,
    pub(super) icon: Option<Icon>,
}

impl Pill {
    pub(super) fn new(elem: u32, label: &str, icon: Option<Icon>) -> Self {
        Self { elem, label: CString::new(label).unwrap_or_default(), icon }
    }
}

const SUMMARY_W: f32 = SCR_W - 2.0 * plx_ui::consts::MARGIN_X;

#[derive(Default)]
pub(super) struct BarLayout {
    stamp: u64,
    pills: [(f32, f32); Pick::FILTERS.len() + 1],
    pub(super) summary_h: f32,
}

/// A single page of theme names; the query generation belongs to the screen, not to the panel.
pub(super) struct ThemeSearch {
    pub(super) query: String,
    pub(super) page: u32,
    pub(super) waiting: bool,
    pub(super) failed: bool,
    pub(super) found: Option<Page<Keyword>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Note {
    Working,
    Empty,
    Failed,
}

type RuntimeChoice = (Option<u16>, Option<u16>, fn() -> &'static str);
const RUNTIMES: [RuntimeChoice; 4] = [
    (None, None, msg::requests_filter_any_runtime),
    (Some(1), Some(90), msg::requests_filter_short),
    (Some(91), Some(120), msg::requests_filter_medium),
    (Some(121), None, msg::requests_filter_long),
];

fn year_label(years: Years) -> Option<String> {
    match years {
        (None, None) => None,
        (Some(start), Some(end)) if start == end => Some(start.to_string()),
        (Some(start), Some(end)) => Some(msg::requests_filter_year_range(&start.to_string(), &end.to_string())),
        (Some(start), None) => Some(msg::requests_filter_year_since(&start.to_string())),
        (None, Some(end)) => Some(msg::requests_filter_year_until(&end.to_string())),
    }
}

fn language_name(code: &str) -> Option<&'static str> {
    if code == "all" {
        return Some(msg::requests_filter_any_language());
    }
    plx_plex::plex::languages::LANGUAGES.iter().find(|l| l.code == code).map(|l| l.name)
}

fn this_year() -> u16 {
    let stamp = i64::try_from(plx_plex::plex::seerr_cache::now()).unwrap_or(0);
    plx_platform::i18n::local_calendar_date(stamp).and_then(|d| u16::try_from(d[0]).ok()).unwrap_or(2026)
}

fn decade(year: u16) -> u16 {
    year - year % 10
}

fn year_bound(years: Years, bound: Bound) -> Option<u16> {
    match bound { Bound::Start => years.0, Bound::End => years.1 }
}

fn with_year(mut years: Years, bound: Bound, year: Option<u16>) -> Years {
    match bound {
        Bound::Start => {
            years.0 = year;
            if let (Some(from), Some(to)) = (year, years.1) {
                years.1 = Some(to.max(from));
            }
        }
        Bound::End => {
            years.1 = year;
            if let (Some(from), Some(to)) = (years.0, year) {
                years.0 = Some(from.min(to));
            }
        }
    }
    years
}

impl RequestsScreen {
    pub(super) fn bar_shown(&self) -> bool {
        self.results.is_none()
    }

    pub(super) fn bar_rect(&self, measure: &dyn plx_machine::machine::Measure, n: usize, section: Rect) -> Rect {
        self.prepare_bar(measure);
        let (x, w) = self.bar_layout.borrow().pills[n];
        Rect::new(x, section.y + layout::BAR_PILL_Y, w, HEIGHT)
    }

    pub(super) fn prepare_bar(&self, measure: &dyn plx_machine::machine::Measure) {
        let mut cache = self.bar_layout.borrow_mut();
        if cache.stamp == self.bar_stamp { return; }
        cache.stamp = self.bar_stamp;
        let gap = space::MD;
        let room = SUMMARY_W - gap * self.pills.len().saturating_sub(1) as f32;
        let slots = &mut cache.pills[..self.pills.len()];
        let mut total = 0.0;
        for (slot, pill) in slots.iter_mut().zip(&self.pills) {
            slot.1 = plx_ui::widgets::Button::pill_w_measured(&pill.label, size::BODY, pill.icon.is_some(), None, measure);
            total += slot.1;
        }
        let extra = (room - total).max(0.0) / slots.len() as f32;
        let fit = (room / total).min(1.0);
        let mut x = plx_ui::consts::MARGIN_X;
        for (left, width) in slots {
            *left = x;
            *width = *width * fit + extra;
            x += *width + gap;
        }
        cache.summary_h = self.summary_view(measure).map_or(0.0, |summary| space::SM + summary.measure_h(SUMMARY_W));
    }

    pub(super) fn summary_view<'a>(&'a self, measure: &'a dyn plx_machine::machine::Measure) -> Option<TextView<'a>> {
        (!self.filter_summary.is_empty()).then(|| {
            TextView::new(&self.filter_summary, size::BODY, plx_ui::theme::TEXT_SECONDARY)
                .with_measure(measure)
                .break_long_words()
        })
    }

    pub(super) fn summary_rect(&self, section: Rect) -> Rect {
        Rect::new(plx_ui::consts::MARGIN_X, section.y + layout::BAR_H, SUMMARY_W, self.bar_layout.borrow().summary_h)
    }

    pub(super) fn choice_rect(&self, focus: Option<u32>, n: usize, section: Rect) -> Option<Rect> {
        self.chooser.as_ref()?.rect(focus.filter(|e| is_chip(*e)).map(|e| (e - CHIP_BASE) as usize), n, section)
    }

    pub(super) fn relabel(&mut self) {
        let b = &self.browse;
        let genre = b.genre.and_then(|id| match &self.genres {
            Genres::Loaded(list) => list.iter().find(|g| g.id == id).map(|g| g.name.as_str()),
            _ => None,
        });
        let years = year_label((b.from, b.to));
        let mut theme_names = String::new();
        for theme in &self.selected_themes {
            if !theme_names.is_empty() { theme_names.push_str(", "); }
            theme_names.push_str(&theme.name);
        }
        let runtime = RUNTIMES.iter().find(|(lo, hi, _)| (*lo, *hi) == (b.runtime_min, b.runtime_max))
            .and_then(|(lo, hi, label)| (lo.is_some() || hi.is_some()).then(|| label()));
        let values = [genre, years.as_deref(), (!theme_names.is_empty()).then_some(theme_names.as_str()),
            b.language.and_then(language_name), runtime];
        self.filter_summary.clear();
        self.pills.clear();
        for (pick, active) in Pick::FILTERS.into_iter().zip(values) {
            let label = pick.name();
            if let Some(value) = active {
                if !self.filter_summary.is_empty() { self.filter_summary.push_str("   ·   "); }
                self.filter_summary.push_str(&msg::requests_filter_pill(label, value));
            }
            self.pills.push(Pill::new(pick.elem(), label, Some(if active.is_some() { Icon::Check } else { Icon::ChevronDown })));
        }
        if !b.is_empty() {
            self.pills.push(Pill::new(RESET, msg::requests_filter_reset(), None));
        }
        self.bar_stamp = self.bar_stamp.wrapping_add(1);
        self.rev = self.rev.wrapping_add(1);
        self.relabel_actions();
    }

    pub(super) fn want_genres<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        if matches!(self.genres, Genres::Asked) {
            return;
        }
        self.genres = Genres::Asked;
        self.ask(RequestsCmd::Genres { reply: self.tx.clone() }, fx);
    }

    pub(super) fn open_chooser<H: ContentLike>(&mut self, pick: Pick, fx: &mut Effects<'_, H>) {
        if pick == Pick::Genre && matches!(self.genres, Genres::Unknown) {
            self.want_genres(fx);
        }
        self.dismiss_chooser();
        self.chooser = Some(Chooser { pick, stage: Stage::Main, choices: Vec::new(), start: 0, years: (self.browse.from, self.browse.to) });
        self.build_choices(None);
        self.seat_choice(fx);
    }

    pub(super) fn dismiss_chooser(&mut self) {
        if self.chooser.take().is_some() {
            self.keyword_search = None;
            self.theme_input.clear();
            self.rev = self.rev.wrapping_add(1);
        }
    }

    pub(super) fn close_chooser<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) -> bool {
        let Some(pick) = self.chooser.as_ref().map(|c| c.pick) else { return false };
        self.dismiss_chooser();
        self.reseat(FocusTarget::Elem(self.key(pick.elem())), fx);
        true
    }

    fn seat_choice<H: ContentLike>(&self, fx: &mut Effects<'_, H>) {
        if let Some(chooser) = &self.chooser {
            self.reseat(FocusTarget::Elem(self.key(CHIP_BASE + chooser.start as u32)), fx);
        }
    }

    fn chooser_stage<H: ContentLike>(&mut self, stage: Stage, prefer: Option<Value>, fx: &mut Effects<'_, H>) {
        if let Some(chooser) = &mut self.chooser {
            if stage == Stage::Range && chooser.stage == Stage::Main {
                chooser.years = (self.browse.from, self.browse.to);
            }
            chooser.stage = stage;
        }
        self.build_choices(prefer);
        self.seat_choice(fx);
    }

    pub(super) fn back_in_chooser<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) -> bool {
        let Some(chooser) = &self.chooser else { return false };
        match chooser.stage {
            Stage::Range => self.chooser_stage(Stage::Main, Some(Value::Range), fx),
            Stage::Decades(bound) => self.chooser_stage(Stage::Range, Some(Value::Bound(bound)), fx),
            Stage::Year(bound, first) => self.chooser_stage(Stage::Decades(bound), Some(Value::Decade(bound, first)), fx),
            Stage::Main if self.keyword_search.take().is_some() => {
                self.theme_input.clear();
                self.build_choices(Some(Value::FindTheme));
                self.seat_choice(fx);
            }
            Stage::Main => return self.close_chooser(fx),
        }
        true
    }

    pub(super) fn build_choices(&mut self, prefer: Option<Value>) {
        let Some(c) = &mut self.chooser else { return };
        let b = &self.browse;
        let year = this_year();
        c.choices.clear();
        let mut push = |name: &str, value, selected| {
            if let Ok(label) = CString::new(name) {
                c.choices.push(Choice { label, value, selected });
            }
        };
        let mut note = None;
        match (c.pick, c.stage) {
            (Pick::Genre, _) => {
                push(msg::requests_filter_any_genre(), Value::Genre(None), b.genre.is_none());
                match &self.genres {
                    Genres::Loaded(list) => for g in list { push(&g.name, Value::Genre(Some(g.id)), b.genre == Some(g.id)); },
                    Genres::Failed => push(msg::requests_filter_retry(), Value::RetryGenres, false),
                    // placeholder-exempt: counted at the draw of the chooser's Working note
                    Genres::Unknown | Genres::Asked => note = Some((msg::requests_loading(), Note::Working)),
                }
            }
            (Pick::Years, Stage::Main) => {
                let now = (b.from, b.to);
                for (label, years) in [
                    (msg::requests_filter_any_year(), (None, None)),
                    (msg::requests_filter_this_year(), (Some(year), Some(year))),
                    (msg::requests_filter_last_five(), (Some(year - 4), Some(year))),
                ] {
                    push(label, Value::Years(years), now == years);
                }
                push(msg::requests_filter_custom_years(), Value::Range, false);
                for start in (OLDEST..=decade(year)).rev().step_by(10) {
                    let range = (Some(start), Some(start + 9));
                    push(&msg::requests_filter_decade(&start.to_string()), Value::Years(range), now == range);
                }
            }
            (Pick::Years, Stage::Range) => {
                push(msg::requests_filter_apply_years(), Value::ApplyRange, false);
                for (bound, label) in [(Bound::Start, msg::requests_filter_start_year()), (Bound::End, msg::requests_filter_end_year())] {
                    let value = year_bound(c.years, bound).map_or_else(|| msg::requests_filter_open_bound().to_owned(), |y| y.to_string());
                    push(&msg::requests_filter_pill(label, &value), Value::Bound(bound), false);
                }
            }
            (Pick::Years, Stage::Decades(bound)) => {
                let current = year_bound(c.years, bound);
                push(msg::requests_filter_open_bound(), Value::Year(bound, None), current.is_none());
                for start in (OLDEST..=decade(year)).rev().step_by(10) {
                    push(&msg::requests_filter_decade(&start.to_string()), Value::Decade(bound, start), current.map(decade) == Some(start));
                }
            }
            (Pick::Years, Stage::Year(bound, first)) => {
                let current = year_bound(c.years, bound);
                for y in (first..=(first + 9).min(year)).rev() {
                    push(&y.to_string(), Value::Year(bound, Some(y)), current == Some(y));
                }
            }
            (Pick::Themes, _) => {
                let text = self.theme_input.text();
                push(if text.trim().is_empty() { msg::requests_filter_find_theme() } else { text }, Value::FindTheme, false);
                if let Some(search) = &self.keyword_search {
                    if search.waiting {
                        note = Some((msg::requests_searching(), Note::Working));
                    } else if search.failed {
                        push(msg::requests_filter_retry(), Value::ThemePage(search.page), false);
                    } else if let Some(page) = &search.found {
                        for word in &page.items {
                            push(&word.name, Value::Theme(word.id), b.keywords.binary_search(&word.id).is_ok());
                        }
                        if page.page > 1 {
                            push(msg::requests_matches_previous(), Value::ThemePage(page.page - 1), false);
                        }
                        if page.more() {
                            push(msg::requests_matches_next(), Value::ThemePage(page.page + 1), false);
                        }
                        if page.items.is_empty() {
                            note = Some((msg::requests_filter_no_themes(), Note::Empty));
                        }
                    }
                } else {
                    push(msg::requests_filter_clear_themes(), Value::ClearThemes, false);
                    for (i, (id, label)) in MOODS.into_iter().enumerate() {
                        push(label(), Value::Theme(id), b.keywords.binary_search(&id).is_ok());
                        if i == 1 {
                            // An added search hit stays removable without finding it again.
                            for word in &self.selected_themes {
                                if !MOODS.iter().any(|m| m.0 == word.id) {
                                    push(&word.name, Value::Theme(word.id), true);
                                }
                            }
                        }
                    }
                }
            }
            (Pick::Language, _) => {
                push(msg::requests_filter_default_language(), Value::Language(None), b.language.is_none());
                push(msg::requests_filter_any_language(), Value::Language(Some("all")), b.language == Some("all"));
                let languages = plx_plex::plex::languages::LANGUAGES;
                let common = ["en", "fr", "es", "hi", "ja", "ko", "de", "it", "zh"];
                let ahead = common.iter().filter_map(|code| languages.iter().find(|l| &l.code == code));
                let rest = languages.iter().filter(|l| l.code.len() == 2 && !common.contains(&l.code));
                for language in ahead.chain(rest) {
                    push(language.name, Value::Language(Some(language.code)), b.language == Some(language.code));
                }
            }
            (Pick::Runtime, _) => for (lo, hi, label) in RUNTIMES {
                push(label(), Value::Runtime(lo, hi), (b.runtime_min, b.runtime_max) == (lo, hi));
            },
            (Pick::Sort, _) => for sort in Sort::ALL {
                push(matches::sort_label(sort), Value::Sort(sort), self.matches.sort == sort);
            },
        }
        if let Some((label, kind)) = note {
            push(label, Value::Note(kind), false);
        }
        c.start = prefer.and_then(|v| c.choices.iter().position(|ch| ch.value == v))
            .or_else(|| (c.pick != Pick::Themes).then(|| c.choices.iter().position(|ch| ch.selected)).flatten())
            .unwrap_or(0);
        self.rev = self.rev.wrapping_add(1);
    }

    pub(super) fn theme_editing(&self) -> bool {
        self.chooser.as_ref().is_some_and(|c| c.pick == Pick::Themes)
    }
    pub(super) fn update_theme_field(&mut self) {
        let Some(field) = self.chooser.as_mut().and_then(|c| c.choices.first_mut()).filter(|c| c.value == Value::FindTheme) else { return };
        let text = self.theme_input.text();
        let label = if text.trim().is_empty() { msg::requests_filter_find_theme() } else { text };
        field.label = CString::new(label).unwrap_or_default();
    }


    pub(super) fn commit_theme<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        let query = self.theme_input.text().trim();
        if query.is_empty() {
            self.keyword_search = None;
            self.build_choices(Some(Value::FindTheme));
        } else {
            let query = query.to_owned();
            self.keyword_search = Some(ThemeSearch { query, page: 1, waiting: false, failed: false, found: None });
            self.want_keywords(1, fx);
        }
    }

    fn want_keywords<H: ContentLike>(&mut self, page: u32, fx: &mut Effects<'_, H>) {
        let Some(search) = &mut self.keyword_search else { return };
        search.page = page;
        search.waiting = true;
        self.keyword_gen = self.keyword_gen.wrapping_add(1);
        let job = RequestsCmd::Keywords { query: search.query.clone(), page, gen: self.keyword_gen, reply: self.tx.clone() };
        self.ask(job, fx);
        self.build_choices(Some(Value::FindTheme));
        self.seat_choice(fx);
    }

    pub(super) fn land_keywords(&mut self, gen: u32, page: u32, result: Result<Page<Keyword>, SeerrError>) -> bool {
        let Some(search) = &mut self.keyword_search else { return false };
        if gen != self.keyword_gen || page != search.page || !search.waiting {
            return false;
        }
        search.waiting = false;
        search.failed = result.as_ref().map_or(true, |found| found.page != page);
        search.found = result.ok().filter(|found| found.page == page);
        self.build_choices(Some(Value::FindTheme));
        true
    }

    pub(super) fn choose<H: ContentLike>(&mut self, n: usize, fx: &mut Effects<'_, H>) -> Handled {
        let Some(choice) = self.chooser.as_ref().and_then(|c| c.choices.get(n)) else { return Handled::No };
        let value = choice.value;
        let mut browse = self.browse.clone();
        match value {
            Value::Genre(id) => browse.genre = id,
            Value::Years(years) => (browse.from, browse.to) = years,
            Value::Language(code) => browse.language = code,
            Value::Runtime(lo, hi) => (browse.runtime_min, browse.runtime_max) = (lo, hi),
            Value::ApplyRange => (browse.from, browse.to) = self.chooser.as_ref().unwrap().years,
            Value::Sort(sort) => {
                self.close_chooser(fx);
                self.change_sort(sort, fx);
                return Handled::Yes;
            }
            _ => {
                match value {
                    Value::Range => self.chooser_stage(Stage::Range, None, fx),
                    Value::Bound(bound) => self.chooser_stage(Stage::Decades(bound), None, fx),
                    Value::Decade(bound, first) => self.chooser_stage(Stage::Year(bound, first), None, fx),
                    Value::Year(bound, y) => {
                        let c = self.chooser.as_mut().unwrap();
                        c.years = with_year(c.years, bound, y);
                        self.chooser_stage(Stage::Range, Some(Value::ApplyRange), fx);
                    }
                    Value::FindTheme => self.keyboard(!self.editing, true, fx),
                    Value::ThemePage(page) => self.want_keywords(page, fx),
                    Value::RetryGenres => {
                        self.want_genres(fx);
                        self.build_choices(None);
                        self.seat_choice(fx);
                    }
                    Value::Theme(id) => {
                        match browse.keywords.binary_search(&id) {
                            Ok(at) => {
                                browse.keywords.remove(at);
                                self.selected_themes.remove(at);
                            }
                            Err(at) => {
                                browse.keywords.insert(at, id);
                                self.selected_themes.insert(at, Keyword { id, name: choice.label.to_string_lossy().into_owned() });
                            }
                        }
                        self.set_filter(browse, fx);
                        self.build_choices(Some(value));
                        self.seat_choice(fx);
                    }
                    Value::ClearThemes => {
                        browse.keywords.clear();
                        self.selected_themes.clear();
                        self.set_filter(browse, fx);
                        self.build_choices(Some(value));
                    }
                    _ => {}
                }
                return Handled::Yes;
            }
        }
        self.close_chooser(fx);
        self.set_filter(browse, fx);
        Handled::Yes
    }

    pub(super) fn reset<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.dismiss_chooser();
        self.selected_themes.clear();
        self.set_filter(Browse::default(), fx);
        self.reseat(FocusTarget::Elem(self.key(GENRE)), fx);
    }

    pub(super) fn below_field(&self) -> Option<GroupId> {
        if self.bar_shown() { Some(GROUP) } else { self.first_content() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_years_keep_both_endpoints_and_correct_crossing_edits() {
        let range = with_year((None, Some(1980)), Bound::Start, Some(1990));
        assert_eq!(range, (Some(1990), Some(1990)));
        assert_eq!(with_year(range, Bound::End, Some(1960)), (Some(1960), Some(1960)));
        assert_eq!(with_year(range, Bound::Start, None), (None, Some(1990)));
    }

    #[test]
    fn applied_values_wrap_below_the_controls_without_changing_their_focus_order() {
        struct Measure;
        impl plx_machine::machine::Measure for Measure {
            fn width(&self, text: &std::ffi::CStr, _: i32, _: bool) -> f32 {
                text.to_string_lossy().chars().count() as f32 * 16.0
            }
            fn cap_h(&self, size: i32) -> f32 { size as f32 }
            fn line_h(&self, size: i32) -> f32 { size as f32 }
        }

        let mut page = RequestsScreen::new(EntryId(1), InstanceId(1));
        page.prepare_bar(&Measure);
        assert_eq!(page.bar_height(), layout::BAR_H);
        page.browse.from = Some(1920);
        page.browse.to = Some(1929);
        let themes = ["Dark comedy", "Heist", "Time travel with a carefully chosen long title",
            "A true story with another very long name to make the filter summary wrap on this screen"];
        for (id, name) in (1..).zip(themes) {
            page.browse.keywords.push(id);
            page.selected_themes.push(Keyword { id, name: name.to_owned() });
        }
        page.relabel();
        page.prepare_bar(&Measure);
        assert!(page.filter_summary.contains(&year_label((Some(1920), Some(1929))).unwrap()));
        assert!(themes.iter().all(|name| page.filter_summary.contains(name)));
        assert!(page.summary_view(&Measure).unwrap().line_count(SUMMARY_W) > 1);
        assert_eq!(page.pills.iter().map(|p| p.elem).collect::<Vec<_>>(), [GENRE, YEARS, THEMES, LANGUAGE, RUNTIME, RESET]);
        for (n, pill) in page.pills.iter().enumerate() {
            let rect = page.bar_rect(&Measure, n, Rect::new(0.0, 0.0, SCR_W, page.bar_height()));
            assert!(rect.w >= plx_ui::widgets::Button::pill_w_measured(&pill.label, size::BODY, pill.icon.is_some(), None, &Measure));
        }
    }
}
