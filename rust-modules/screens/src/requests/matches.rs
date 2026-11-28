//! One popular carousel previews filtered movies; the all-matches grid reads any page in the
//! selected order without changing the filter. Each outstanding request has one owner and token.
use super::*;
use filters::{Note, Pill};
use plx_plex::plex::seerr::{BrowsePage, SeerrError};
use plx_platform::i18n::msg;
use plx_ui::icons::Icon;
use std::ffi::CString;

pub(super) const ALL: u32 = 8;
pub(super) const SORT: u32 = filters::CHIP_BASE - 4;
const PREVIOUS: u32 = SORT + 1;
const NEXT: u32 = SORT + 2;
const RETRY: u32 = SORT + 3;
pub(super) const GROUP: GroupId = GroupId(0x5251_0004);

#[derive(Clone, Copy)]
struct Pages {
    total: u32,
    pages: u32,
    capped: bool,
}

pub(super) struct Matches {
    pub(super) open: bool,
    pub(super) sort: Sort,
    pub(super) page: u32,
    pub(super) row: Row,
    pending: Option<u32>,
    preview_pending: Option<u32>,
    pages: Option<Pages>,
    preview_pages: Option<Pages>,
    pub(super) info: CString,
    pub(super) actions: Vec<Pill>,
}

impl Matches {
    pub(super) fn new(sort: Sort) -> Self {
        Self {
            open: false, sort, page: 1, row: Row::new(Lane::Matches), pending: None, preview_pending: None,
            pages: None, preview_pages: None, info: CString::default(), actions: Vec::new(),
        }
    }

    fn showing_preview(&self, filtered: bool) -> bool {
        filtered && self.sort == Sort::Popular && self.page == 1
    }

    fn grid_pages(&self, filtered: bool) -> Option<Pages> {
        if self.showing_preview(filtered) { self.preview_pages } else { self.pages }
    }

    pub(super) fn shown_page(&self, filtered: bool) -> u32 {
        if self.open && self.grid_pages(filtered).is_some() { self.page } else { 0 }
    }

    pub(super) fn total(&self, filtered: bool) -> Option<u32> {
        if self.open { self.grid_pages(filtered).or(self.preview_pages) } else { self.preview_pages }
            .map(|p| p.total)
    }

    pub(super) fn write(&self, c: &mut Canon) {
        c.bool(self.open).u32(self.sort as u32).u32(self.page).u32(self.pending.unwrap_or(0)).u32(self.preview_pending.unwrap_or(0));
        for pages in [self.pages, self.preview_pages] {
            c.bool(pages.is_some());
            if let Some(p) = pages { c.u32(p.total).u32(p.pages).bool(p.capped); }
        }
    }
}

pub(super) fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Popular => msg::requests_sort_popular(),
        Sort::TopRated => msg::requests_sort_rated(),
        Sort::Newest => msg::requests_sort_released(),
    }
}

impl RequestsScreen {
    pub(super) fn match_row(&self) -> &Row {
        if self.matches.showing_preview(!self.browse.is_empty()) {
            &self.rows[Lane::BROWSE.len()]
        } else {
            &self.matches.row
        }
    }

    pub(super) fn set_filter<H: ContentLike>(&mut self, browse: Browse, fx: &mut Effects<'_, H>) {
        if browse == self.browse { return; }
        self.browse = browse;
        let was_open = self.matches.open;
        self.matches = Matches::new(self.matches.sort);
        self.matches.open = was_open;
        self.keys.retain(|key| !key.0.is_movies_filter());
        self.rows[Lane::BROWSE.len()] = Row::new(Lane::Popular);
        self.filter_gen = self.filter_gen.wrapping_add(1);
        self.grid_gen = self.grid_gen.wrapping_add(1);
        if !self.browse.is_empty() {
            self.ask_movies(Lane::Popular, fx);
        }
        if was_open && !self.matches.showing_preview(!self.browse.is_empty()) {
            self.ask_movies(Lane::Matches, fx);
        }
        self.relabel();
    }

    fn ask_movies<H: ContentLike>(&mut self, lane: Lane, fx: &mut Effects<'_, H>) {
        let (pending, sort, page) = if lane == Lane::Popular {
            (&mut self.matches.preview_pending, Sort::Popular, 1)
        } else {
            (&mut self.matches.pending, self.matches.sort, self.matches.page)
        };
        if pending.is_some() { return; }
        self.browse_gen = self.browse_gen.wrapping_add(1);
        *pending = Some(self.browse_gen);
        self.ask(RequestsCmd::Browse {
            filter: self.browse.clone(), sort, page, gen: self.browse_gen, reply: self.tx.clone(),
        }, fx);
    }

    pub(super) fn land_movies(&mut self, gen: u32, sort: Sort, page: u32, result: Result<BrowsePage, SeerrError>) -> bool {
        let lane = if self.matches.preview_pending == Some(gen) && sort == Sort::Popular && page == 1 {
            self.matches.preview_pending = None;
            Lane::Popular
        } else if self.matches.pending == Some(gen) && (sort, page) == (self.matches.sort, self.matches.page) {
            self.matches.pending = None;
            Lane::Matches
        } else {
            return false;
        };
        match result {
            Ok(p) if p.titles.page == page => {
                let pages = Pages { total: p.total, pages: p.titles.pages, capped: p.capped };
                if lane == Lane::Popular { self.matches.preview_pages = Some(pages); }
                else { self.matches.pages = Some(pages); }
                self.land(lane, true, p.titles.items.iter().map(|title| (title, None)).collect(), false, page, 0);
            }
            _ => { self.fail(lane); }
        }
        self.relabel_actions();
        true
    }

    fn open_matches<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.dismiss_chooser();
        self.matches.open = true;
        if !self.match_row().loaded {
            let lane = self.match_row().lane;
            self.ask_movies(lane, fx);
        }
        self.relabel_actions();
        self.stack.scroll_to(0.0);
        self.reseat(FocusTarget::Elem(self.key(SORT)), fx);
    }

    pub(super) fn close_matches<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) -> bool {
        if !self.matches.open { return false; }
        self.matches.open = false;
        self.relabel_actions();
        self.stack.scroll_to(0.0);
        self.reseat(FocusTarget::Elem(self.key(ALL)), fx);
        true
    }

    fn turn_page<H: ContentLike>(&mut self, page: u32, fx: &mut Effects<'_, H>) {
        self.matches.page = page;
        self.matches.row = Row::new(Lane::Matches);
        self.matches.pending = None;
        self.matches.pages = None;
        self.keys.retain(|k| k.0 != Lane::Matches);
        self.grid_gen = self.grid_gen.wrapping_add(1);
        if !self.match_row().loaded {
            let lane = self.match_row().lane;
            self.ask_movies(lane, fx);
        }
        self.relabel_actions();
        self.stack.scroll_to(0.0);
        self.reseat(FocusTarget::Elem(self.key(SORT)), fx);
    }

    pub(super) fn change_sort<H: ContentLike>(&mut self, sort: Sort, fx: &mut Effects<'_, H>) {
        if self.matches.sort == sort { return; }
        self.matches.sort = sort;
        self.turn_page(1, fx);
    }

    pub(super) fn match_action<H: ContentLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) -> bool {
        if !self.matches.actions.iter().any(|a| a.elem == elem) { return false; }
        match elem {
            ALL => self.open_matches(fx),
            PREVIOUS => self.turn_page(self.matches.page - 1, fx),
            NEXT => self.turn_page(self.matches.page + 1, fx),
            RETRY => {
                let lane = self.match_row().lane;
                if let Some(row) = self.row_mut(lane) { row.failed = false; row.loaded = false; }
                self.ask_movies(lane, fx);
                self.relabel_actions();
                self.reseat(FocusTarget::Elem(self.key(SORT)), fx);
            }
            _ => return false,
        }
        true
    }

    pub(super) fn note(&self) -> Option<Note> {
        if !self.bar_shown() || (!self.matches.open && self.browse.is_empty()) { return None; }
        let row = if self.matches.open { self.match_row() } else { &self.rows[Lane::BROWSE.len()] };
        if !row.cards.is_empty() { return None; }
        Some(if row.failed { Note::Failed } else if row.loaded { Note::Empty } else { Note::Working })
    }

    pub(super) fn relabel_actions(&mut self) {
        let failed = self.match_row().failed;
        let matches = &mut self.matches;
        let filtered = !self.browse.is_empty();
        let mut actions = std::mem::take(&mut matches.actions);
        matches.info = CString::default();
        actions.clear();
        if matches.open {
            let label = msg::requests_filter_pill(msg::requests_matches_sort(), sort_label(matches.sort));
            actions.push(Pill::new(SORT, &label, Some(Icon::ChevronDown)));
            if matches.page > 1 { actions.push(Pill::new(PREVIOUS, msg::requests_matches_previous(), Some(Icon::ChevronLeft))); }
            if let Some(p) = matches.grid_pages(filtered) {
                if matches.page < p.pages { actions.push(Pill::new(NEXT, msg::requests_matches_next(), Some(Icon::Chevron))); }
                let count = msg::requests_matches_count(&p.total.to_string());
                let page = match p.pages {
                    0 => String::new(),
                    _ if p.capped => msg::requests_matches_page_limit(&matches.page.to_string(), &p.pages.to_string()),
                    _ => msg::requests_matches_page(&matches.page.to_string(), &p.pages.to_string()),
                };
                let info = if page.is_empty() { count } else { format!("{count}  ·  {page}") };
                matches.info = CString::new(info).unwrap_or_default();
            }
            if failed { actions.push(Pill::new(RETRY, msg::requests_filter_retry(), None)); }
        } else {
            let label = match (filtered, matches.preview_pages) {
                (false, _) => msg::requests_all_movies().to_owned(),
                (true, Some(p)) => msg::requests_all_matches_count(&p.total.to_string()),
                (true, None) => msg::requests_all_matches().to_owned(),
            };
            actions.push(Pill::new(ALL, &label, Some(Icon::Chevron)));
        }
        self.matches.actions = actions;
        self.rev = self.rev.wrapping_add(1);
    }

    pub(super) fn action_rect(&self, measure: &dyn plx_machine::machine::Measure, section: Rect, n: usize) -> Rect {
        let gap = plx_ui::theme::space::MD;
        let width = |pill: &Pill| plx_ui::widgets::Button::pill_w_measured(&pill.label, plx_ui::theme::size::BODY, pill.icon.is_some(), None, measure);
        let x = plx_ui::consts::MARGIN_X + self.matches.actions.iter().take(n).map(|p| width(p) + gap).sum::<f32>();
        let w = self.matches.actions.get(n).map_or(0.0, width);
        Rect::new(x, section.y + layout::BAR_PILL_Y, w, filters::HEIGHT)
    }
}
