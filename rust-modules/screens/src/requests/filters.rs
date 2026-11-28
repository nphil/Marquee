//! The Requests page's filter bar: genre, first and last release year and a mood, set from a row of
//! controls under the search pill. Picking a value swaps the bar for a strip of choices in place (no
//! menu over the page); the choice is a filter Seerr hands to TMDB, so the rows under the bar are the
//! filtered listing, page after page, not a filtered copy of the first page.
use super::*;
use plx_plex::plex::seerr::{Browse, Genre};
use std::cell::RefCell;
use std::ffi::{CStr, CString};

pub(super) const GENRE: u32 = 2;
pub(super) const FROM: u32 = 3;
pub(super) const TO: u32 = 4;
pub(super) const MOOD: u32 = 5;
pub(super) const RESET: u32 = 6;
pub(super) const GROUP: GroupId = GroupId(0x5251_0001);
pub(super) const NOTE_GROUP: GroupId = GroupId(0x5251_0002);
/// The first element of the choice strip; below the top bar's tabs and above the page's own numbers.
pub(super) const CHIP_BASE: u32 = 0x2000_0000;
/// The oldest year the strip offers.
const OLDEST: u16 = 1920;
/// The gap between two controls.
const GAP: f32 = plx_ui::theme::space::SM;

/// Whether `elem` is a control of the bar or a choice of its strip.
pub(super) fn is_bar(elem: u32) -> bool {
    (GENRE..=RESET).contains(&elem) || is_chip(elem)
}

pub(super) fn is_chip(elem: u32) -> bool {
    (CHIP_BASE..plx_ui::dispatch::STRIP_BASE).contains(&elem)
}

/// What a control sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    Genre,
    From,
    To,
    Mood,
}

impl Pick {
    const ALL: [Pick; 4] = [Pick::Genre, Pick::From, Pick::To, Pick::Mood];

    pub(super) fn elem(self) -> u32 {
        match self {
            Pick::Genre => GENRE,
            Pick::From => FROM,
            Pick::To => TO,
            Pick::Mood => MOOD,
        }
    }

    pub(super) fn of(elem: u32) -> Option<Pick> {
        Pick::ALL.into_iter().find(|p| p.elem() == elem)
    }

    fn name(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Pick::Genre => msg::requests_filter_genre(),
            Pick::From => msg::requests_filter_from(),
            Pick::To => msg::requests_filter_to(),
            Pick::Mood => msg::requests_filter_mood(),
        }
    }

    fn any_label(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Pick::Genre => msg::requests_filter_any_genre(),
            Pick::From => msg::requests_filter_any_from(),
            Pick::To => msg::requests_filter_any_to(),
            Pick::Mood => msg::requests_filter_any_mood(),
        }
    }

    /// The filter with this control set to `value`. A first year after the last, or a last before the
    /// first, drags the other end along so the range is never empty by construction; a decade sets both.
    fn set(self, mut browse: Browse, value: Value) -> Browse {
        match (self, value) {
            (Pick::Genre, Value::Any) => browse.genre = None,
            (Pick::Genre, Value::Id(id)) => browse.genre = Some(id),
            (Pick::Mood, Value::Any) => browse.keyword = None,
            (Pick::Mood, Value::Id(id)) => browse.keyword = Some(id),
            (Pick::From, Value::Any) => browse.from = None,
            (Pick::From, Value::Year(year)) => {
                browse.from = Some(year);
                browse.to = browse.to.map(|to| to.max(year));
            }
            (Pick::From, Value::Decade(start)) => (browse.from, browse.to) = (Some(start), Some(start + 9)),
            (Pick::To, Value::Any) => browse.to = None,
            (Pick::To, Value::Year(year)) => {
                browse.to = Some(year);
                browse.from = browse.from.map(|from| from.min(year));
            }
            _ => {}
        }
        browse
    }

    /// The choice that stands for this control's current value, if the strip has one.
    fn is_current(self, browse: &Browse, value: Value) -> bool {
        match (self, value) {
            (Pick::Genre, Value::Any) => browse.genre.is_none(),
            (Pick::Genre, Value::Id(id)) => browse.genre == Some(id),
            (Pick::Mood, Value::Any) => browse.keyword.is_none(),
            (Pick::Mood, Value::Id(id)) => browse.keyword == Some(id),
            (Pick::From, Value::Any) => browse.from.is_none(),
            (Pick::From, Value::Year(year)) => browse.from == Some(year),
            (Pick::To, Value::Any) => browse.to.is_none(),
            (Pick::To, Value::Year(year)) => browse.to == Some(year),
            _ => false,
        }
    }
}

/// What a choice of the strip sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Value {
    Any,
    Id(u32),
    Year(u16),
    /// A whole decade, named by its first year: sets the first and the last year together.
    Decade(u16),
    /// Not a choice: asks for the genres again.
    Retry,
    /// Not a choice: the genres are on their way.
    Waiting,
}

pub(super) struct Choice {
    pub(super) label: CString,
    pub(super) value: Value,
}

/// The strip that replaces the bar while a value is being picked.
pub(super) struct Chooser {
    pub(super) pick: Pick,
    pub(super) choices: Vec<Choice>,
    /// The choice that stands for the value now, where the strip opens.
    pub(super) start: usize,
}

/// Where the genres Seerr offers stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Genres {
    Unknown,
    Asked,
    Loaded(Vec<Genre>),
    Failed,
}

/// The moods: a keyword TMDB tags films with, and its name. Ids are TMDB's own and stay put; each has
/// over 250 films, so a mood never filters to a handful.
pub(super) const MOODS: [(u32, fn() -> &'static str); 27] = {
    use plx_platform::i18n::msg;
    [
        (207268, msg::requests_mood_neo_noir),
        (9807, msg::requests_mood_film_noir),
        (10123, msg::requests_mood_dark_comedy),
        (4565, msg::requests_mood_dystopia),
        (12190, msg::requests_mood_cyberpunk),
        (4379, msg::requests_mood_time_travel),
        (10051, msg::requests_mood_heist),
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

fn label(text: &str) -> Option<CString> {
    CString::new(text).ok()
}

/// A bar control: what it says and whether it holds a filter.
pub(super) struct Pill {
    pub(super) elem: u32,
    pub(super) label: CString,
    pub(super) active: bool,
}

/// The controls for `browse`: each with its value, then Reset while any is set.
pub(super) fn pills(browse: &Browse, genres: &Genres) -> Vec<Pill> {
    use plx_platform::i18n::msg;
    let any = msg::requests_filter_any();
    let genre = browse.genre.and_then(|id| match genres {
        Genres::Loaded(list) => list.iter().find(|g| g.id == id).map(|g| g.name.as_str()),
        _ => None,
    });
    let mood = browse.keyword.and_then(|id| MOODS.iter().find(|m| m.0 == id)).map(|m| (m.1)());
    let from = browse.from.map(|y| y.to_string());
    let to = browse.to.map(|y| y.to_string());
    let mut out = Vec::with_capacity(5);
    for pick in Pick::ALL {
        let value = match pick {
            Pick::Genre => genre,
            Pick::From => from.as_deref(),
            Pick::To => to.as_deref(),
            Pick::Mood => mood,
        };
        let active = value.is_some();
        let text = msg::requests_filter_pill(pick.name(), value.unwrap_or(any));
        out.extend(label(&text).map(|label| Pill { elem: pick.elem(), label, active }));
    }
    if !browse.is_empty() {
        out.push(Pill { elem: RESET, label: msg::requests_filter_reset_c().to_owned(), active: false });
    }
    out
}

/// This year, for the top of the year strip.
fn this_year() -> u16 {
    let stamp = i64::try_from(plx_plex::plex::seerr_cache::now()).unwrap_or(0);
    plx_platform::i18n::local_calendar_date(stamp).and_then(|d| u16::try_from(d[0]).ok()).unwrap_or(2026)
}

/// The strip for `pick`: Any first, then the values, newest year first. The first-year strip puts the
/// decades before the years, so a range such as the 1920s is one choice.
pub(super) fn chooser(pick: Pick, browse: &Browse, genres: &Genres) -> Chooser {
    use plx_platform::i18n::msg;
    let mut choices = vec![Choice { label: label(pick.any_label()).unwrap_or_default(), value: Value::Any }];
    let mut push = |text: &str, value: Value| choices.extend(label(text).map(|label| Choice { label, value }));
    match pick {
        Pick::Genre => match genres {
            Genres::Loaded(list) => list.iter().for_each(|g| push(&g.name, Value::Id(g.id))),
            Genres::Failed => push(msg::requests_filter_retry(), Value::Retry),
            // placeholder-exempt: the visible waiting chip is counted in render::bar.
            Genres::Unknown | Genres::Asked => push(msg::requests_loading(), Value::Waiting),
        },
        Pick::Mood => MOODS.iter().for_each(|(id, name)| push(name(), Value::Id(*id))),
        Pick::From | Pick::To => {
            let year = this_year();
            if pick == Pick::From {
                for start in (OLDEST..=year - year % 10).rev().step_by(10) {
                    push(&msg::requests_filter_decade(&start.to_string()), Value::Decade(start));
                }
            }
            for y in (OLDEST..=year).rev() {
                push(&y.to_string(), Value::Year(y));
            }
        }
    }
    let start = choices.iter().position(|c| pick.is_current(browse, c.value)).unwrap_or(0);
    Chooser { pick, choices, start }
}

impl Chooser {
    /// The filter after choice `n`, and whether it is one (the others are notes).
    pub(super) fn apply(&self, n: usize, browse: Browse) -> Option<Browse> {
        let choice = self.choices.get(n)?;
        match choice.value {
            Value::Retry | Value::Waiting => None,
            value => Some(self.pick.set(browse, value)),
        }
    }
}

/// Where the bar's controls (or the strip's choices) sit, measured once per change.
#[derive(Default)]
pub(super) struct Layout {
    stamp: u64,
    xs: Vec<f32>,
    ws: Vec<f32>,
}

/// The pills' and chips' placement, cached on `RequestsScreen::bar_stamp`.
pub(super) type LayoutCache = RefCell<Layout>;

impl RequestsScreen {
    /// The filter bar is part of the page unless a search stands in for the browse rows.
    pub(super) fn bar_shown(&self) -> bool {
        self.results.is_none()
    }

    pub(super) fn bar_len(&self) -> usize {
        if !self.bar_shown() {
            return 0;
        }
        self.chooser.as_ref().map_or(self.pills.len(), |c| c.choices.len())
    }

    pub(super) fn bar_elem(&self, n: usize) -> Option<u32> {
        if n >= self.bar_len() {
            return None;
        }
        match &self.chooser {
            Some(_) => Some(CHIP_BASE + n as u32),
            None => self.pills.get(n).map(|p| p.elem),
        }
    }

    pub(super) fn bar_label(&self, n: usize) -> Option<&CStr> {
        match &self.chooser {
            Some(c) => c.choices.get(n).map(|c| c.label.as_c_str()),
            None => self.pills.get(n).map(|p| p.label.as_c_str()),
        }
    }

    pub(super) fn bar_waiting(&self, n: usize) -> bool {
        self.chooser.as_ref().and_then(|c| c.choices.get(n)).is_some_and(|c| c.value == Value::Waiting)
    }

    /// Whether control or choice `n` holds a value: it is drawn lit.
    pub(super) fn bar_lit(&self, n: usize) -> bool {
        match &self.chooser {
            Some(c) => c.choices.get(n).is_some_and(|ch| c.pick.is_current(&self.browse, ch.value)),
            None => self.pills.get(n).is_some_and(|p| p.active),
        }
    }

    /// Where control or choice `n` sits in a bar section drawn at `section`. The strip is wider than
    /// the screen: it slides so the focused choice stays at the middle, and stands still at its ends.
    pub(super) fn bar_rect(&self, measure: &dyn plx_machine::machine::Measure, focus: Option<u32>, n: usize, section: Rect) -> Rect {
        let mut cache = self.bar_layout.borrow_mut();
        if cache.stamp != self.bar_stamp || cache.xs.len() != self.bar_len() {
            cache.stamp = self.bar_stamp;
            cache.ws.clear();
            cache.xs.clear();
            let mut x = plx_ui::consts::MARGIN_X;
            for label in (0..self.bar_len()).filter_map(|n| self.bar_label(n)) {
                let w = plx_ui::widgets::Button::pill_w_measured(label, plx_ui::theme::size::BODY, false, None, measure);
                cache.xs.push(x);
                cache.ws.push(w);
                x += w + GAP;
            }
        }
        let Some((&x, &w)) = cache.xs.get(n).zip(cache.ws.get(n)) else { return Rect::new(0.0, section.y, 0.0, 0.0) };
        let shift = match &self.chooser {
            Some(chooser) => {
                let at = focus.filter(|e| is_chip(*e)).map_or(chooser.start, |e| (e - CHIP_BASE) as usize);
                let end = cache.xs.last().zip(cache.ws.last()).map_or(0.0, |(x, w)| x + w) + plx_ui::consts::MARGIN_X;
                let centre = cache.xs.get(at).zip(cache.ws.get(at)).map_or(0.0, |(x, w)| x + w / 2.0);
                (centre - SCR_W / 2.0).clamp(0.0, (end - SCR_W).max(0.0))
            }
            None => 0.0,
        };
        Rect::new(x - shift, section.y + layout::BAR_PILL_Y, w, plx_ui::widgets::StatusOverlay::CTRL_H)
    }

    /// Left or right of control `at`, as an index; `None` at the ends.
    pub(super) fn bar_step(&self, at: usize, dir: Dir) -> Option<usize> {
        match dir {
            Dir::Left => at.checked_sub(1),
            Dir::Right => (at + 1 < self.bar_len()).then_some(at + 1),
            _ => None,
        }
    }

    /// Rebuild what the bar says after the filter, the genres or the strip changed.
    pub(super) fn relabel(&mut self) {
        self.pills = pills(&self.browse, &self.genres);
        self.bar_stamp = self.bar_stamp.wrapping_add(1);
        self.rev = self.rev.wrapping_add(1);
    }

    /// Ask Seerr for the genres once.
    pub(super) fn want_genres<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        if matches!(self.genres, Genres::Asked) {
            return;
        }
        self.genres = Genres::Asked;
        self.ask(RequestsCmd::Genres { reply: self.tx.clone() }, fx);
    }

    /// The control `pick` was pressed: its choices replace the bar and focus lands on the current one.
    pub(super) fn open_chooser<H: ContentLike>(&mut self, pick: Pick, fx: &mut Effects<'_, H>) {
        if pick == Pick::Genre && matches!(self.genres, Genres::Unknown) {
            self.want_genres(fx);
        }
        let chooser = chooser(pick, &self.browse, &self.genres);
        let at = chooser.start as u32;
        self.chooser = Some(chooser);
        self.relabel();
        self.reseat(FocusTarget::Elem(self.key(CHIP_BASE + at)), fx);
    }

    /// Close the strip and put focus back on the control it came from.
    pub(super) fn close_chooser<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) -> bool {
        let Some(chooser) = self.chooser.take() else { return false };
        self.relabel();
        self.reseat(FocusTarget::Elem(self.key(chooser.pick.elem())), fx);
        true
    }

    /// Choice `n` of the strip was pressed.
    pub(super) fn choose<H: ContentLike>(&mut self, n: usize, fx: &mut Effects<'_, H>) -> Handled {
        let Some(chooser) = &self.chooser else { return Handled::No };
        match chooser.apply(n, self.browse) {
            Some(browse) => {
                self.close_chooser(fx);
                self.set_filter(browse, fx);
            }
            None if chooser.choices.get(n).is_some_and(|c| c.value == Value::Retry) => {
                self.genres = Genres::Unknown;
                self.want_genres(fx);
                self.chooser = Some(super::filters::chooser(Pick::Genre, &self.browse, &self.genres));
                self.relabel();
            }
            None => {}
        }
        Handled::Yes
    }

    /// Clear every filter in one press.
    pub(super) fn reset<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.chooser = None;
        self.set_filter(Browse::default(), fx);
        self.reseat(FocusTarget::Elem(self.key(GENRE)), fx);
    }

    /// Make `browse` the page's filter: the filtered rows start over and Seerr is asked for their
    /// first pages, in the three orders; with no filter they are gone.
    pub(super) fn set_filter<H: ContentLike>(&mut self, browse: Browse, fx: &mut Effects<'_, H>) {
        if browse == self.browse {
            return;
        }
        self.browse = browse;
        self.filter_gen = self.filter_gen.wrapping_add(1);
        self.keys.retain(|key| !Lane::FILTERED.contains(&key.0));
        for lane in Lane::FILTERED {
            if let Some(row) = self.row_mut(lane) {
                *row = Row::new(lane);
            }
        }
        if !browse.is_empty() {
            for sort in plx_plex::plex::seerr::Sort::ALL {
                self.ask(RequestsCmd::Browse { filter: browse, sort, page: 1, gen: self.filter_gen, reply: self.tx.clone() }, fx);
            }
        }
        self.relabel();
    }
}

/// What the area under the bar says while no filtered row has a poster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Note {
    Working,
    Empty,
    Failed,
}

impl RequestsScreen {
    /// The group Down from the search pill goes to: the bar, or the first row when a search hides it.
    pub(super) fn below_field(&self) -> Option<GroupId> {
        if self.bar_shown() { Some(GROUP) } else { self.first_content() }
    }

    /// The read-out for a filter that has nothing to show yet (or at all).
    pub(super) fn note(&self) -> Option<Note> {
        if self.browse.is_empty() || !self.bar_shown() {
            return None;
        }
        let rows = &self.rows[Lane::BROWSE.len()..];
        if rows.iter().any(|r| !r.cards.is_empty()) {
            None
        } else if rows.iter().any(|r| r.failed) {
            Some(Note::Failed)
        } else if rows.iter().all(|r| r.loaded) {
            Some(Note::Empty)
        } else {
            Some(Note::Working)
        }
    }
}
