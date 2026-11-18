//! The Requests page: what Seerr suggests, the viewer's own requests and a title search, as
//! poster rows under the strip. Every call to Seerr is the app driver's (`AppFx::Requests`); the
//! answers come back down this page's channel and are read on its tick, so nothing here waits.
//! A title opens [`title::TitleScreen`], where it is asked for.
mod cards;
mod layout;
mod render;
pub mod title;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod cards_harness; // Tier 2 card conformance (cards_conformance_tests.rs)

use std::borrow::Cow;
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::registry::{
    seerr_error_text, AppFx, ContentArg, ContentLike, ContentReq, HomeTab, PageMemory, RequestsCmd, RequestsReply,
    SearchReq,
};
use cards::RowCards;
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, InputOwner, InstanceId,
    Key, LogicalState, Machine, MachineId, TextEdit,
};
use plx_machine::present::Provenance;
use plx_plex::plex::seerr::{Badge, Kind, Shelf as Discover, Title};
use plx_ui::cards::{CardEvent, SectionFrame, SectionSpec, Shelf, Stack, StackEvent, StackPage};
use plx_ui::consts::{SCR_H, SCR_W};
use plx_ui::frame::Budget;
use plx_ui::screen::{
    AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusTarget, GroupKind, GroupSpec, Link, Placed,
    RenderStrategy, Screen, ScreenEvent, Seat,
};
use plx_ui::text_buffer::TextBuffer;
use plx_ui::Rect;

const FIELD: u32 = 1;
const FIELD_GROUP: GroupId = GroupId(0x5251_0000);
const STRIP: GroupId = plx_ui::containers::tabs::STRIP;
const BLINK_US: u32 = 530_000;
/// The first element number a card is given; the field and the strip keep the ones below.
const FIRST_ELEM: u32 = 10;

/// The rows of the page. `Results` stands alone while a search is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Mine,
    Trending,
    Movies,
    Shows,
    Upcoming,
    Results,
}

impl Lane {
    /// The browse rows, top to bottom.
    const BROWSE: [Lane; 5] = [Lane::Mine, Lane::Trending, Lane::Movies, Lane::Shows, Lane::Upcoming];
    fn ordinal(self) -> u32 {
        self as u32
    }
    fn group(self) -> GroupId {
        GroupId(0x5251_0100 + self.ordinal())
    }
    fn heading(self) -> &'static std::ffi::CStr {
        use plx_platform::i18n::msg;
        match self {
            Lane::Mine => msg::requests_mine_c(),
            Lane::Trending => msg::requests_trending_c(),
            Lane::Movies => msg::requests_movies_c(),
            Lane::Shows => msg::requests_shows_c(),
            Lane::Upcoming => msg::requests_upcoming_c(),
            Lane::Results => msg::requests_results_c(),
        }
    }
    fn discover(self) -> Option<Discover> {
        match self {
            Lane::Trending => Some(Discover::Trending),
            Lane::Movies => Some(Discover::Movies),
            Lane::Shows => Some(Discover::Shows),
            Lane::Upcoming => Some(Discover::Upcoming),
            Lane::Mine | Lane::Results => None,
        }
    }
    fn of(shelf: Discover) -> Lane {
        match shelf {
            Discover::Trending => Lane::Trending,
            Discover::Movies => Lane::Movies,
            Discover::Shows => Lane::Shows,
            Discover::Upcoming => Lane::Upcoming,
        }
    }
}

/// One poster: a title as Seerr names it, and how it stands.
#[derive(PartialEq)]
pub(crate) struct Card {
    elem: u32,
    kind: Kind,
    tmdb: u32,
    name: String,
    year: String,
    /// The poster's whole address on TMDB; empty when the title has none.
    thumb: String,
    badge: Option<Badge>,
}

/// One row of posters, and whether Seerr has more after them.
pub(crate) struct Row {
    lane: Lane,
    cards: Vec<Card>,
    /// A reply (good or not) has come for this row's first page.
    loaded: bool,
    more: bool,
    /// The next page is asked for and has not come.
    asked: bool,
    /// Pages read, for a discover row or a search.
    pages: u32,
    /// Requests read, for the viewer's own.
    skip: u32,
    /// Moves whenever a page lands, so the shelf reads a landing as the answer to its ask.
    epoch: u32,
}

impl Row {
    fn new(lane: Lane) -> Self {
        Self { lane, cards: Vec::new(), loaded: false, more: false, asked: false, pages: 0, skip: 0, epoch: 0 }
    }
}

/// Where the page is in reaching Seerr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Opening,
    Ready,
    Failed,
}

/// The sections of the page, top to bottom. A search's row carries the page's `rows_gen`: each
/// search is a new section with a new shelf.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sec {
    Head,
    Row(Lane, u32),
}

pub struct RequestsScreen {
    entry: EntryId,
    instance: InstanceId,
    /// Every reply of the driver comes down this one channel.
    tx: Sender<RequestsReply>,
    inbox: Receiver<RequestsReply>,
    phase: Phase,
    /// Why the page could not open, in words.
    failure: Option<std::ffi::CString>,
    art: Option<plx_plex::plex::ServerId>,
    query: TextBuffer,
    editing: bool,
    blink_us: u32,
    /// The words the shown search was made for.
    searched: String,
    /// A search is out and has not answered.
    searching: bool,
    rows: [Row; 5],
    results: Option<Row>,
    keys: Vec<(Lane, Kind, u32, u32)>,
    next_elem: u32,
    rows_gen: u32,
    /// Moves whenever the rows change, for [`StackPage::revision`].
    rev: u64,
    stack: Stack<Sec>,
    render: render::Resources,
}

impl RequestsScreen {
    pub fn new(entry: EntryId, instance: InstanceId) -> Self {
        let (tx, inbox) = channel();
        Self {
            entry,
            instance,
            tx,
            inbox,
            phase: Phase::Opening,
            failure: None,
            art: None,
            query: TextBuffer::new(String::new(), 0),
            editing: false,
            blink_us: 0,
            searched: String::new(),
            searching: false,
            rows: Lane::BROWSE.map(Row::new),
            results: None,
            keys: Vec::new(),
            next_elem: FIRST_ELEM,
            rows_gen: 0,
            rev: 0,
            stack: new_stack(entry),
            render: Default::default(),
        }
    }

    fn key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    fn intern(&mut self, lane: Lane, kind: Kind, tmdb: u32) -> u32 {
        if let Some(&(.., elem)) = self.keys.iter().find(|k| (k.0, k.1, k.2) == (lane, kind, tmdb)) {
            return elem;
        }
        let elem = self.next_elem;
        self.next_elem = self.next_elem.checked_add(1).expect("Requests element space exhausted");
        self.keys.push((lane, kind, tmdb, elem));
        elem
    }

    fn row_mut(&mut self, lane: Lane) -> Option<&mut Row> {
        match lane {
            Lane::Results => self.results.as_mut(),
            _ => self.rows.iter_mut().find(|r| r.lane == lane),
        }
    }

    /// The rows on show, top to bottom: the search's alone while one is shown, else the browse
    /// rows that hold something.
    fn shown(&self) -> impl Iterator<Item = &Row> + '_ {
        let browse = self.results.is_none();
        self.results.iter().chain(self.rows.iter().filter(move |_| browse)).filter(|r| !r.cards.is_empty())
    }

    fn first_content(&self) -> Option<GroupId> {
        self.shown().next().map(|r| r.lane.group())
    }

    fn cards_of(&self, k: Sec) -> Option<RowCards<'_>> {
        let Sec::Row(lane, _) = k else { return None };
        let row = self.shown().find(|r| r.lane == lane)?;
        Some(RowCards { row, art: self.art })
    }

    fn row_frame<H: ContentLike>(&self, cx: &Cx<'_, H>, lane: Lane) -> Option<(SectionFrame, &Shelf)> {
        let k = Sec::Row(lane, self.rows_gen);
        Some((self.stack.shelf_frame(self, cx, k)?, self.stack.shelf(k)?))
    }

    /// Whether every row has had its first answer, so that an empty page is the truth.
    fn settled(&self) -> bool {
        self.rows.iter().all(|r| r.loaded)
    }

    fn ask<H: ContentLike>(&self, cmd: RequestsCmd, fx: &mut Effects<'_, H>) {
        fx.push(Fx::App(AppFx::Requests(cmd)));
    }

    fn open<H: ContentLike>(&self, fx: &mut Effects<'_, H>) {
        self.ask(RequestsCmd::Open(self.tx.clone()), fx);
    }

    /// Back from a title: read again only if what the page shows may have gone stale.
    fn refresh<H: ContentLike>(&self, fx: &mut Effects<'_, H>) {
        self.ask(RequestsCmd::Refresh, fx);
    }

    /// Take one answer of the driver into the rows. `true` when the page changed: an answer that
    /// repeats what the page shows (a refresh behind a held copy) changes nothing, so nothing
    /// redraws and focus stays where it is.
    fn receive(&mut self, reply: RequestsReply) -> bool {
        let changed = match reply {
            RequestsReply::Ready { art, .. } => {
                let changed = self.art != art || self.phase != Phase::Ready || self.failure.is_some();
                self.art = art;
                self.phase = Phase::Ready;
                self.failure = None;
                changed
            }
            RequestsReply::Failed(error) => {
                self.phase = Phase::Failed;
                self.failure = std::ffi::CString::new(seerr_error_text(error)).ok();
                for row in &mut self.rows {
                    row.loaded = true;
                }
                true
            }
            RequestsReply::Shelf { shelf, page, result } => {
                let lane = Lane::of(shelf);
                match result {
                    Ok(p) => {
                        let more = p.more();
                        let cards = p.items.iter().map(|t| (t, None)).collect();
                        self.land(lane, page <= 1, cards, more, page, 0)
                    }
                    Err(_) => self.fail(lane),
                }
            }
            RequestsReply::Mine { skip, result } => match result {
                Ok((items, more)) => {
                    let read = items.len() as u32;
                    let cards = items.iter().map(|a| (&a.title, a.badge)).collect();
                    self.land(Lane::Mine, skip == 0, cards, more, 0, skip + read)
                }
                Err(_) => self.fail(Lane::Mine),
            },
            RequestsReply::Search { query, page, result } => {
                if query != self.searched {
                    return false;
                }
                self.searching = false;
                match result {
                    Ok(p) => {
                        let more = p.more();
                        let cards = p.items.iter().map(|t| (t, None)).collect();
                        self.land(Lane::Results, page <= 1, cards, more, page, 0);
                    }
                    Err(_) => {
                        self.fail(Lane::Results);
                    }
                }
                true
            }
            RequestsReply::Detail { .. } | RequestsReply::Requested { .. } => false,
        };
        if changed {
            self.rev = self.rev.wrapping_add(1);
        }
        changed
    }

    fn fail(&mut self, lane: Lane) -> bool {
        let Some(row) = self.row_mut(lane) else { return false };
        let was = (row.loaded, row.more, row.asked);
        (row.loaded, row.more, row.asked) = (true, false, false);
        was != (row.loaded, row.more, row.asked)
    }

    /// A page of `lane` landed: `first` replaces what the row held, a later one follows it.
    /// `badge` is the stated one of a title the viewer asked for; a title read off a shelf shows
    /// the one its own state gives. Returns whether the row changed.
    ///
    /// A first page that arrives over a row already read further (the page was held and has since
    /// been refreshed) only patches the cards it names: a title keeps its place and its element, so
    /// focus never moves, and the pages the viewer scrolled to stay.
    fn land(&mut self, lane: Lane, first: bool, titles: Vec<(&Title, Option<Badge>)>, more: bool, pages: u32, skip: u32) -> bool {
        let mut cards = Vec::with_capacity(titles.len());
        for (title, badge) in titles {
            let elem = self.intern(lane, title.kind, title.tmdb);
            cards.push(Card {
                elem,
                kind: title.kind,
                tmdb: title.tmdb,
                name: title.name.clone(),
                year: title.year.clone(),
                thumb: plx_plex::plex::seerr::tmdb_image(&title.poster, "w342"),
                badge: badge.or_else(|| title.state.badge()),
            });
        }
        let Some(row) = self.row_mut(lane) else { return false };
        if first && row.loaded && (row.pages > pages || row.skip > skip) {
            let mut changed = false;
            for (at, card) in cards.into_iter().enumerate() {
                match row.cards.iter_mut().find(|c| c.elem == card.elem) {
                    Some(held) if *held == card => {}
                    Some(held) => {
                        *held = card;
                        changed = true;
                    }
                    None => {
                        row.cards.insert(at.min(row.cards.len()), card);
                        changed = true;
                    }
                }
            }
            if changed {
                row.epoch = row.epoch.wrapping_add(1);
            }
            return changed;
        }
        let mut changed = !row.loaded;
        if first {
            if row.cards != cards || row.more != more {
                row.cards = cards;
                changed = true;
            }
        } else {
            for card in cards {
                if !row.cards.iter().any(|c| c.elem == card.elem) {
                    row.cards.push(card);
                    changed = true;
                }
            }
        }
        // A first page that repeats the row leaves a next page in flight alone.
        if changed || !first {
            row.asked = false;
            row.more = more;
            row.pages = pages;
            row.skip = skip;
        }
        row.loaded = true;
        if changed {
            row.epoch = row.epoch.wrapping_add(1);
        }
        changed
    }

    /// The row's next page, asked for once.
    fn want<H: ContentLike>(&mut self, lane: Lane, fx: &mut Effects<'_, H>) {
        let reply = self.tx.clone();
        let query = self.searched.clone();
        let Some(row) = self.row_mut(lane) else { return };
        if !row.more || row.asked {
            return;
        }
        row.asked = true;
        let cmd = match lane.discover() {
            Some(shelf) => RequestsCmd::Shelf { shelf, page: row.pages + 1, reply },
            None if lane == Lane::Mine => RequestsCmd::Mine { skip: row.skip, reply },
            None => RequestsCmd::Search { query, page: row.pages + 1, reply },
        };
        self.ask(cmd, fx);
    }

    /// Make the typed words the search: an empty field ends it, anything else asks Seerr.
    fn commit<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        let words = self.query.text().trim().to_owned();
        if words == self.searched {
            return;
        }
        self.searched = words;
        self.rows_gen = self.rows_gen.wrapping_add(1);
        self.rev = self.rev.wrapping_add(1);
        self.stack.scroll_to(0.0);
        if self.searched.is_empty() {
            self.results = None;
            self.searching = false;
            return;
        }
        self.results = Some(Row::new(Lane::Results));
        self.searching = true;
        self.ask(RequestsCmd::Search { query: self.searched.clone(), page: 1, reply: self.tx.clone() }, fx);
    }

    /// Leave the search the page shows, and the words behind it.
    fn clear<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.query.clear();
        self.commit(fx);
        self.reseat(FocusTarget::Elem(self.key(FIELD)), fx);
    }

    fn reseat<H: ContentLike>(&self, target: FocusTarget<u32>, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: target }))));
    }

    fn keyboard<H: ContentLike>(&mut self, up: bool, commit: bool, fx: &mut Effects<'_, H>) {
        if self.editing == up {
            return;
        }
        if !up && commit {
            self.commit(fx);
        }
        self.editing = up;
        self.blink_us = 0;
        if up {
            let text = self.query.text().to_string();
            let end = text.len();
            self.query = TextBuffer::new(text, end);
            self.stack.scroll_to(0.0);
        }
        fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Keyboard { up }));
        fx.invalidate(Provenance::Input);
    }

    fn edit<H: ContentLike>(&mut self, edit: &TextEdit, fx: &mut Effects<'_, H>) {
        self.query.edit(edit);
        self.blink_us = 0;
        fx.invalidate(Provenance::Input);
    }

    fn activate<H: ContentLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) -> Handled {
        use plx_ui::widgets::{
            TAB_KEY_CHIP, TAB_KEY_HOME, TAB_KEY_MOVIES, TAB_KEY_REQUESTS, TAB_KEY_SEARCH, TAB_KEY_SHOWS, TAB_KEY_WATCHLIST,
        };
        if elem >= plx_ui::dispatch::STRIP_BASE {
            let request = match elem {
                TAB_KEY_HOME => SearchReq::Tab(HomeTab::Home),
                TAB_KEY_MOVIES => SearchReq::Tab(HomeTab::Movies),
                TAB_KEY_SHOWS => SearchReq::Tab(HomeTab::Shows),
                TAB_KEY_WATCHLIST => SearchReq::Tab(HomeTab::Watchlist),
                TAB_KEY_SEARCH => SearchReq::Tab(HomeTab::Search),
                // The Requests mark is the page the viewer is already on.
                TAB_KEY_REQUESTS => return Handled::Yes,
                TAB_KEY_CHIP => SearchReq::Account,
                _ => return Handled::No,
            };
            self.keyboard(false, true, fx);
            fx.push(Fx::App(AppFx::Search(request)));
            return Handled::Yes;
        }
        if elem == FIELD {
            self.keyboard(!self.editing, true, fx);
            return Handled::Yes;
        }
        let found = self.shown().flat_map(|r| r.cards.iter()).find(|c| c.elem == elem).map(|c| (c.kind, c.tmdb));
        match found {
            Some((kind, tmdb)) => {
                fx.push(Fx::App(AppFx::Content(ContentReq::Push(ContentArg::RequestTitle { kind, tmdb }))));
                Handled::Yes
            }
            None => Handled::No,
        }
    }

    fn tick<H: ContentLike>(&mut self, tick: plx_machine::machine::Tick, fx: &mut Effects<'_, H>) {
        let mut changed = false;
        while let Ok(reply) = self.inbox.try_recv() {
            changed |= self.receive(reply);
        }
        if changed {
            fx.invalidate(Provenance::Landing(fx.from()));
        }
        if self.editing {
            let was = self.blink_us < BLINK_US;
            self.blink_us = ((u64::from(self.blink_us) + u64::from(tick.dt_us)) % (2 * u64::from(BLINK_US))) as u32;
            if was != (self.blink_us < BLINK_US) {
                fx.invalidate(Provenance::Input);
            }
        } else {
            self.blink_us = 0;
        }
    }

    /// Run `f` on the stack and the page it lays out (the stack is out of the page meanwhile).
    fn with_stack<R>(&mut self, f: impl FnOnce(&mut Stack<Sec>, &Self) -> R) -> R {
        let mut stack = std::mem::replace(&mut self.stack, Stack::new(self.entry));
        let got = f(&mut stack, &*self);
        self.stack = stack;
        got
    }

    fn stack_on<H: ContentLike>(
        &mut self,
        ev: &ScreenEvent<H>,
        cx: &Cx<'_, H>,
        fx: &mut Effects<'_, H>,
    ) -> Option<CardEvent<u32>> {
        match self.with_stack(|stack, page| stack.on(page, ev, cx, fx)) {
            Some(StackEvent::Card(Sec::Row(lane, _), CardEvent::Want(range))) => {
                self.want(lane, fx);
                Some(CardEvent::Want(range))
            }
            Some(StackEvent::Card(_, card)) => Some(card),
            _ => None,
        }
    }

    /// The probe line the app's focus read-out prints.
    pub fn probe_line(&self) -> String {
        format!(
            "requests entry={} editing={} phase={:?} rows={} searching={}",
            self.entry.0,
            self.editing,
            self.phase,
            self.shown().count(),
            self.searching
        )
    }
}

impl<H: ContentLike> Machine<H> for RequestsScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let card = self.stack_on(ev, cx, fx);
        match ev {
            ScreenEvent::Mount => {
                self.open(fx);
                self.reseat(FocusTarget::Elem(self.key(FIELD)), fx);
            }
            // Back from a title: what was asked for shows on its poster.
            ScreenEvent::Uncover => self.refresh(fx),
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                self.keyboard(false, false, fx)
            }
            ScreenEvent::Activate(elem) => return self.activate(*elem, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(CardEvent::Activate(elem)) = card {
                    return self.activate(elem, fx);
                }
                if let Some(key) = cx.focus.current {
                    return self.activate(key.elem, fx);
                }
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    if *up {
                        self.stack.scroll_to(0.0);
                    }
                    self.editing = *up;
                    self.blink_us = 0;
                }
                InputKind::Text(edit) if self.editing || matches!(cx.owner, InputOwner::System(_)) => self.edit(edit, fx),
                InputKind::Wheel { dy } => {
                    if !self.editing && dy.is_finite() {
                        let max = self.stack.max_scroll(&*self, cx);
                        let unit = if dy.fract() == 0.0 { plx_ui::table::ROW_H } else { plx_ui::consts::ROW_PITCH };
                        self.stack.scroll_to((self.stack.target() - dy * unit).clamp(0.0, max));
                        fx.invalidate(Provenance::Input);
                    }
                }
                InputKind::Key { key, sym, edge, .. } if *edge != Edge::Up => {
                    if self.editing {
                        let edit = match (*key, *sym) {
                            (Key::Left, _) => Some(TextEdit::Left),
                            (Key::Right, _) => Some(TextEdit::Right),
                            (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                            (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                            _ => None,
                        };
                        if let Some(edit) = edit {
                            self.edit(&edit, fx);
                            return Handled::Yes;
                        }
                        match *key {
                            Key::Back => self.keyboard(false, false, fx),
                            Key::Ok => self.keyboard(false, true, fx),
                            Key::Down => {
                                self.keyboard(false, true, fx);
                                if let Some(group) = self.first_content() {
                                    self.reseat(FocusTarget::ContainerGroup(group), fx);
                                }
                            }
                            _ => {}
                        }
                        return Handled::Yes;
                    }
                    if *key == Key::Up && cx.focus.current == Some(self.key(FIELD)) {
                        self.reseat(FocusTarget::Elem(self.key(plx_ui::widgets::TAB_KEY_REQUESTS)), fx);
                        return Handled::Yes;
                    }
                    if *key == Key::Back {
                        // Back closes the innermost thing: a shown search, then the page.
                        if self.results.is_some() || !self.query.text().is_empty() {
                            self.clear(fx);
                        } else {
                            fx.push(Fx::App(AppFx::Search(SearchReq::Back)));
                        }
                        return Handled::Yes;
                    }
                    return Handled::No;
                }
                _ => return Handled::No,
            },
            ScreenEvent::Tick(tick) => self.tick(*tick, fx),
            ScreenEvent::FocusMoved { to, .. } => {
                if to.elem != FIELD {
                    self.keyboard(false, true, fx);
                }
                if self.editing {
                    self.stack.scroll_to(0.0);
                }
                fx.invalidate(Provenance::Input);
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

/// The page's stack: the standing strip owns the hits under it and the scroll follows focus when
/// it moves.
fn new_stack(entry: EntryId) -> Stack<Sec> {
    let floor = plx_ui::widgets::TOP_BAR_BOTTOM;
    Stack::new(entry).reveal_on_move(true).clipped(Rect::new(0.0, floor, SCR_W, SCR_H - floor))
}

impl<H: ContentLike> StackPage<H> for RequestsScreen {
    type Key = Sec;
    type Cards<'a> = RowCards<'a>;

    fn revision(&self, _cx: &Cx<'_, H>) -> u64 {
        let mix = |h: u64, v: u64| (h ^ v).wrapping_mul(0x0100_0000_01b3);
        let mut h = mix(0xcbf2_9ce4_8422_2325, u64::from(self.rows_gen));
        h = mix(h, self.rev);
        for row in self.shown() {
            h = mix(mix(h, u64::from(row.lane.ordinal())), row.cards.len() as u64);
        }
        h
    }
    fn sections(&self, _cx: &Cx<'_, H>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Head, plx_ui::cards::Kind::Custom { height: layout::CONTENT_TOP, focusable: true }, FIELD_GROUP));
        for row in self.shown() {
            out.push(
                SectionSpec::new(
                    Sec::Row(row.lane, self.rows_gen),
                    plx_ui::cards::Kind::Shelf { style: &layout::POSTER, heading: layout::HEAD_TO_ROW },
                    row.lane.group(),
                )
                .seated(Seat::Projected)
                .edges([EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop])
                .of_kind(ElemKind::Card),
            );
        }
    }
    fn fallback(&self, _cx: &Cx<'_, H>, out: &mut Vec<Sec>) {
        out.push(Sec::Head);
    }
    fn cards<'a>(&'a self, _cx: &'a Cx<'_, H>, k: Sec) -> Option<RowCards<'a>> {
        self.cards_of(k)
    }
    fn card_has_menu(&self, _cx: &Cx<'_, H>, _k: Sec, _elem: &u32) -> bool {
        false
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        (k == Sec::Head).then_some(FIELD)
    }
    fn focus_rect(&self, _cx: &Cx<'_, H>, _k: Sec, section: Rect) -> Rect {
        Rect::new(layout::FIELD.x, layout::FIELD.y + section.y, layout::FIELD.w, layout::FIELD.h)
    }
    fn plain_group(&self, _cx: &Cx<'_, H>, _k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        GroupSpec {
            id,
            kind: GroupKind::Row { wrap: false },
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
            extent,
            len: 1,
            elem: ElemKind::Bare,
        }
    }
    /// Entered from the field, a row takes the card focus last left on in it, else its first.
    fn seat_override(&self, cx: &Cx<'_, H>, k: Sec, _from: Placed) -> Option<u32> {
        if !cx.focus.current.is_some_and(|key| key.elem == FIELD) {
            return None;
        }
        let Sec::Row(lane, _) = k else { return None };
        let row = self.shown().find(|r| r.lane == lane)?;
        let valid = |elem: &u32| row.cards.iter().any(|c| c.elem == *elem);
        Some(cx.focus.remembered(lane.group()).filter(valid).or(row.cards.first().map(|c| c.elem)).unwrap_or(FIELD))
    }
    fn reveal_margin(&self, _k: Sec) -> f32 {
        layout::CONTENT_TOP
    }
    fn shelf_foot(&self, _cx: &Cx<'_, H>, _k: Sec) -> f32 {
        plx_ui::consts::UNDER_LABEL_AIR
    }
    fn wide_extent(&self, _k: Sec) -> bool {
        false
    }
    /// A card that went away (a page replaced its row) is replaced by the field.
    fn recover(&self, _cx: &Cx<'_, H>, _want: &u32) -> Option<u32> {
        Some(FIELD)
    }
}

plx_ui::focusable_via_view!(RequestsScreen, H: [ContentLike], view);

impl RequestsScreen {
    fn view<H: ContentLike>(&self) -> impl plx_ui::screen::Focusable<H> + '_ {
        self.stack.view(self)
    }
}

impl<H: ContentLike> Screen<H> for RequestsScreen {
    fn glides(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "requests"
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _: &Cx<'_, H>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _: &mut Budget, cx: &Cx<'_, H>) {
        self.render.prepare(self.query.text(), self.query.caret(), cx);
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        render::draw(self, f);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn links(&self, out: &mut Vec<Link>) {
        out.push(Link { from: STRIP, dir: Dir::Down, to: FIELD_GROUP });
        out.push(Link { from: FIELD_GROUP, dir: Dir::Up, to: STRIP });
        if let Some(first) = self.first_content() {
            out.push(Link { from: FIELD_GROUP, dir: Dir::Down, to: first });
            out.push(Link { from: first, dir: Dir::Up, to: FIELD_GROUP });
        }
        let groups: Vec<GroupId> = self.shown().map(|r| r.lane.group()).collect();
        for pair in groups.windows(2) {
            out.push(Link { from: pair[0], dir: Dir::Down, to: pair[1] });
            out.push(Link { from: pair[1], dir: Dir::Up, to: pair[0] });
        }
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
    fn memory(&self) -> PageMemory {
        PageMemory::None
    }
}

impl LogicalState for RequestsScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.instance.0);
        c.u32(self.phase as u32).bool(self.editing).u32(self.blink_us).bool(self.searching);
        c.str(self.query.text()).u64(self.query.caret() as u64).str(&self.searched);
        self.stack.write_scroll(c);
        c.u32(self.next_elem).u32(self.rows_gen);
        let rows = self.rows.iter().chain(self.results.iter());
        c.seq(rows.clone().count());
        for row in rows {
            c.u32(row.lane.ordinal()).bool(row.loaded).bool(row.more).bool(row.asked).u32(row.pages).u32(row.skip);
            c.seq(row.cards.len());
            for card in &row.cards {
                c.u32(card.elem);
            }
            if let Some(shelf) = self.stack.shelf(Sec::Row(row.lane, self.rows_gen)) {
                shelf.write(c);
            }
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&self.probe_line());
    }
}
