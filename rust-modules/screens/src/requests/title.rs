//! One title's artwork, credits, synopsis and actions. Requests and progress never wait on the frame.
mod facts;
mod paint;
mod shelves;

use std::borrow::Cow;
use std::sync::mpsc::{channel, Receiver, Sender};

use super::progress_text;
use crate::registry::{AppFx, ContentArg, ContentLike, ContentReq, PageMemory, RequestsCmd, RequestsReply, TitlePage};
use plx_machine::machine::{Canon, Cx, Delivery, Edge, Effects, EntryId, Fx, GroupId, Handled, InputKind, Key, LogicalState, Machine, Tick};
use plx_machine::present::Provenance;
use plx_platform::i18n::msg;
use plx_plex::plex::progress::{self, Snapshot, Tracker};
use plx_plex::plex::seerr::{tmdb_image, Ask, Kind, SeerrError};
use plx_ui::cards::{CardEvent, Kind as Section, RowStyle, SectionSpec, Stack, StackEvent, StackPage};
use plx_ui::consts::{MARGIN_X, SCR_W, UNDER_LABEL_AIR};
use plx_ui::screen::{AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Enter, FocusSource, FocusTarget, GroupKind, GroupSpec, HitSource, Link, RenderStrategy, Screen, ScreenEvent, Seat};
use plx_ui::theme;
use plx_ui::widgets::{StatusKind, StatusOverlay};
use plx_ui::{Env, Rect, View};

const PLAY: u32 = 1;
const REQUEST: u32 = 2;
const TRAILER: u32 = 3;
const ABOUT: u32 = 4;
const HERO_READ: u32 = 5;
const SEASON: u32 = 100;
const CAST: u32 = 1000;
const VIDEO: u32 = 2000;
const STILL: u32 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec { Hero, About, Seasons, Cast, Videos, Episodes }

impl Sec {
    const ALL: [Self; 6] = [Self::Hero, Self::About, Self::Seasons, Self::Cast, Self::Videos, Self::Episodes];
    fn group(self) -> GroupId { GroupId(0x5254_0000 + self as u32) }
    fn style(self) -> &'static RowStyle {
        match self {
            Self::Seasons => &RowStyle::HOME,
            Self::Cast => &RowStyle::CAST,
            _ => &RowStyle::EPISODE,
        }
    }
}

impl facts::Act {
    fn elem(self) -> Option<u32> {
        match self {
            Self::Play => Some(PLAY),
            Self::Request => Some(REQUEST),
            Self::Trailer => Some(TRAILER),
            Self::Status => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent { Idle, Sending, Done, Failed(SeerrError) }

enum Load { Loading, Ready(Box<Ready>), Failed(SeerrError) }

struct Ready {
    page: TitlePage,
    ask: Ask,
    actions: Vec<facts::Action>,
    poster: String,
    backdrop: String,
    length: String,
    stage: &'static str,
    facts: Vec<facts::Fact>,
    overflow_overview: bool,
    seasons: Vec<shelves::Card>,
    cast: Vec<shelves::Card>,
    videos: Vec<shelves::Card>,
}

impl Ready {
    fn new(page: TitlePage, ask: Ask, picked: &[u32], sending: bool, measure: &dyn plx_machine::machine::Measure) -> Self {
        let poster = tmdb_image(&page.detail.title.poster, "w342");
        let backdrop = tmdb_image(&page.detail.title.backdrop, "w1280");
        let seasons = facts::announced(&page).take((CAST - SEASON) as usize).enumerate()
            .map(|(i, s)| shelves::season(s, &page, &poster, i)).collect();
        Self {
            actions: facts::actions(&page, picked, sending),
            length: facts::length(&page),
            stage: page.detail.info.stage.map_or("", facts::stage_word),
            facts: facts::about_facts(&page),
            overflow_overview: paint::preview(&page.detail.title.overview, measure).truncates(theme::hero::TEXT_W),
            cast: shelves::cast(&page), videos: shelves::videos(&page),
            page, ask, poster, backdrop, seasons,
        }
    }

    fn about(&self) -> bool { !self.facts.is_empty() || self.overflow_overview }

    fn about_overview(&self) -> Option<&str> {
        self.overflow_overview.then(|| self.page.detail.title.overview.trim())
    }
}

pub struct TitleScreen {
    entry: EntryId,
    kind: Kind,
    tmdb: u32,
    stack: Stack<Sec>,
    revision: u64,
    tx: Sender<RequestsReply>,
    inbox: Receiver<RequestsReply>,
    load: Load,
    sent: Sent,
    picked: Vec<u32>,
    stills: Vec<shelves::Card>,
    trailer_pending: bool,
    trailer_failed: bool,
    tracker: Tracker,
    progress_at: u32,
    snapshot: Option<Snapshot>,
    progress: Option<progress_text::Lines>,
    season_progress: Vec<(u32, shelves::SeasonProgress)>,
    visible: bool,
}

impl TitleScreen {
    pub fn new(entry: EntryId, kind: Kind, tmdb: u32) -> Self {
        let (tx, inbox) = channel();
        Self {
            entry, kind, tmdb, stack: Stack::new(entry).reveal_on_move(true), revision: 0, tx, inbox,
            load: Load::Loading, sent: Sent::Idle, picked: Vec::new(), stills: Vec::new(),
            trailer_pending: false, trailer_failed: false,
            tracker: Tracker::new(progress::Key { kind, tmdb }, entry.0), progress_at: 0,
            snapshot: None, progress: None, season_progress: Vec::new(),
            visible: true,
        }
    }

    fn ready(&self) -> Option<&Ready> {
        match &self.load { Load::Ready(r) => Some(r), _ => None }
    }
    fn picker(&self) -> bool {
        self.ready().is_some_and(|r| r.page.rules.partial && matches!(r.ask, Ask::Seasons(_)))
    }
    fn actions(&self) -> &[facts::Action] { self.ready().map_or(&[], |r| &r.actions) }
    fn cards_of(&self, k: Sec) -> Option<shelves::Cards<'_>> {
        let ready = self.ready()?;
        let art = ready.page.art;
        let cards = match k {
            Sec::Seasons if self.picker() || !self.season_progress.is_empty() => &ready.seasons,
            Sec::Cast if art.is_some() => &ready.cast,
            Sec::Videos if art.is_some() => &ready.videos,
            Sec::Episodes if art.is_some() => &self.stills,
            _ => return None,
        };
        (!cards.is_empty()).then_some(shelves::Cards { screen: self, sid: art.map(|s| s.raw()), cards, which: k })
    }
    fn focus_sections(&self) -> impl Iterator<Item = Sec> + '_ {
        Sec::ALL.into_iter().filter(|k| match k {
            Sec::Hero => self.ready().is_some(),
            Sec::About => self.ready().is_some_and(Ready::about),
            _ => self.cards_of(*k).is_some(),
        })
    }
    fn ask<H: ContentLike>(&self, cmd: RequestsCmd, fx: &mut Effects<'_, H>) { fx.push(Fx::App(AppFx::Requests(cmd))); }
    fn read<H: ContentLike>(&self, fx: &mut Effects<'_, H>) {
        self.ask(RequestsCmd::Detail { kind: self.kind, tmdb: self.tmdb, reply: self.tx.clone() }, fx);
    }

    fn relabel_actions(&mut self) {
        if let Load::Ready(ready) = &mut self.load {
            if let Some(action) = ready.actions.iter_mut().find(|a| a.act == facts::Act::Request) {
                let empty_picker = ready.page.rules.partial && matches!(ready.ask, Ask::Seasons(_)) && self.picked.is_empty();
                action.enabled = !matches!(self.sent, Sent::Sending | Sent::Done) && !empty_picker;
            }
        }
    }
    fn read_stills<H: ContentLike>(&self, page: &TitlePage, fx: &mut Effects<'_, H>) {
        if self.kind != Kind::Tv || page.art.is_none() { return; }
        let aired = page.detail.seasons.iter().filter(|s| s.number > 0 && s.episodes > 0).map(|s| s.number);
        if let Some(first) = aired.clone().min() {
            let mut seasons = vec![first];
            if let Some(last) = aired.max().filter(|n| *n != first) { seasons.push(last); }
            self.ask(RequestsCmd::Stills { tmdb: self.tmdb, seasons, reply: self.tx.clone() }, fx);
        }
    }
    fn progress_readout(&mut self) -> bool {
        let snapshot = self.tracker.snapshot();
        if self.snapshot.as_ref() == Some(&snapshot) { return false; }
        let was_empty = self.season_progress.is_empty();
        self.progress = progress_text::lines(&snapshot, self.kind);
        self.season_progress = snapshot.seasons.iter().map(|row| (row.number, shelves::SeasonProgress::new(row))).collect();
        if was_empty != self.season_progress.is_empty() { self.revision += 1; }
        self.snapshot = Some(snapshot);
        true
    }
    fn receive<H: ContentLike>(&mut self, reply: RequestsReply, now: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> bool {
        match reply {
            RequestsReply::Detail { kind, tmdb, result } if kind == self.kind && tmdb == self.tmdb => {
                match result {
                    Ok(page) => {
                        let first = self.ready().is_none();
                        let ask = page.detail.ask(page.rules);
                        match &ask {
                            Ask::Seasons(free) if first => self.picked.clone_from(free),
                            Ask::Seasons(free) => self.picked.retain(|n| free.contains(n)),
                            _ => self.picked.clear(),
                        }
                        self.picked.sort_unstable();
                        if first { self.read_stills(&page, fx); }
                        let disabled = matches!(self.sent, Sent::Sending | Sent::Done);
                        self.load = Load::Ready(Box::new(Ready::new(page, ask, &self.picked, disabled, cx.measure)));
                        self.revision += 1;
                    }
                    Err(error) if self.ready().is_none() => self.load = Load::Failed(error),
                    Err(_) => return false,
                }
                true
            }
            RequestsReply::Requested { kind, tmdb, result } if kind == self.kind && tmdb == self.tmdb => {
                self.sent = match result { Ok(()) => Sent::Done, Err(e) => Sent::Failed(e) };
                self.relabel_actions();
                if result.is_ok() && self.visible {
                    self.tracker.hide();
                    self.tracker.show(now);
                }
                if matches!(result, Ok(()) | Err(SeerrError::AlreadyRequested)) { self.read(fx); }
                true
            }
            RequestsReply::Stills { tmdb, result: Ok(stills) } if tmdb == self.tmdb => {
                self.stills = shelves::stills(stills);
                self.revision += 1;
                true
            }
            RequestsReply::Trailer { opened } => {
                self.trailer_pending = false;
                self.trailer_failed = !opened;
                true
            }
            RequestsReply::Progress { gen, step, outcome } => {
                self.tracker.land(gen, step, outcome) && self.progress_readout()
            }
            _ => false,
        }
    }
    fn tick<H: ContentLike>(&mut self, t: Tick, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> bool {
        let first = self.ready().is_none();
        let mut changed = false;
        while let Ok(reply) = self.inbox.try_recv() { changed |= self.receive(reply, t.ms, cx, fx); }
        if let Some(order) = self.tracker.advance(t.ms) {
            self.ask(RequestsCmd::Progress { order, reply: self.tx.clone() }, fx);
        }
        if self.visible && t.ms.wrapping_sub(self.progress_at) >= 1_000 {
            self.progress_at = t.ms;
            changed |= self.progress_readout();
        }
        if changed { fx.invalidate(Provenance::Landing(fx.from())); }
        first && self.ready().is_some()
    }
    fn launch<H: ContentLike>(&mut self, key: String, fx: &mut Effects<'_, H>) {
        if !self.trailer_pending {
            self.trailer_pending = true;
            self.trailer_failed = false;
            self.ask(RequestsCmd::Trailer { video: key, reply: self.tx.clone() }, fx);
            fx.invalidate(Provenance::Input);
        }
    }
    fn activate<H: ContentLike>(&mut self, elem: u32, fx: &mut Effects<'_, H>) -> Handled {
        let Some(ready) = self.ready() else { return Handled::Yes };
        match elem {
            PLAY => {
                if let Some((sid, rk)) = &ready.page.plex {
                    fx.push(Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { sid: *sid, rk: rk.clone() }))));
                }
            }
            REQUEST if !matches!(self.sent, Sent::Sending | Sent::Done) => {
                let seasons = match &ready.ask {
                    Ask::Movie => Vec::new(),
                    Ask::Seasons(_) if self.picker() => self.picked.clone(),
                    Ask::Seasons(free) => free.clone(),
                    _ => return Handled::Yes,
                };
                if self.kind == Kind::Tv && seasons.is_empty() { return Handled::Yes; }
                self.sent = Sent::Sending;
                self.relabel_actions();
                self.ask(RequestsCmd::Request { kind: self.kind, tmdb: self.tmdb, seasons, reply: self.tx.clone() }, fx);
                fx.invalidate(Provenance::Input);
            }
            TRAILER => {
                if let Some(video) = ready.page.detail.info.trailer() { self.launch(video.key.clone(), fx); }
            }
            SEASON..CAST if self.picker() && !matches!(self.sent, Sent::Sending | Sent::Done) => {
                let Some(season) = facts::announced(&ready.page).nth((elem - SEASON) as usize) else { return Handled::Yes };
                let n = season.number;
                if !matches!(&ready.ask, Ask::Seasons(free) if free.contains(&n)) { return Handled::Yes; }
                match self.picked.binary_search(&n) {
                    Ok(i) => { self.picked.remove(i); }
                    Err(i) => self.picked.insert(i, n),
                }
                self.relabel_actions();
                fx.invalidate(Provenance::Input);
            }
            VIDEO..STILL if ready.page.art.is_some() => {
                if let Some(video) = ready.page.detail.info.videos.get((elem - VIDEO) as usize) { self.launch(video.key.clone(), fx); }
            }
            _ => {}
        }
        Handled::Yes
    }

    fn stack_on<H: ContentLike>(&mut self, ev: &ScreenEvent<H>, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Option<StackEvent<Sec, u32>> {
        let mut stack = std::mem::replace(&mut self.stack, Stack::new(self.entry));
        let result = stack.on(self, ev, cx, fx);
        self.stack = stack;
        result
    }
    fn view<H: ContentLike>(&self) -> impl plx_ui::screen::Focusable<H> + '_ { self.stack.view(self) }
}

impl<H: ContentLike> Machine<H> for TitleScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let first = if let ScreenEvent::Tick(t) = ev { self.tick(*t, cx, fx) } else { false };
        let stack = self.stack_on(ev, cx, fx);
        if first {
            if let Some(k) = self.focus_sections().next() {
                fx.push(Fx::Deliver(fx.from(), Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: FocusTarget::FirstInGroup(k.group()) }))));
            }
        }
        match ev {
            ScreenEvent::Mount => self.read(fx),
            ScreenEvent::Cover | ScreenEvent::Suspend | ScreenEvent::WillLeave(_) | ScreenEvent::Unmount => {
                self.visible = false;
                self.tracker.hide();
            }
            ScreenEvent::Uncover | ScreenEvent::Resume => {
                self.visible = true;
                self.tracker.show(cx.tick.ms);
            }
            ScreenEvent::Tick(_) | ScreenEvent::Enter(_) | ScreenEvent::FocusMoved { .. } => {}
            ScreenEvent::Activate(elem) => return self.activate(*elem, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = stack { return self.activate(elem, fx); }
                if let Some(key) = cx.focus.current { return self.activate(key.elem, fx); }
            }
            ScreenEvent::Input(input) if matches!(input.kind, InputKind::Key { key: Key::Back, edge: Edge::Down, .. }) => {
                self.visible = false;
                self.tracker.hide();
                fx.push(Fx::App(AppFx::Content(ContentReq::Back)));
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl<H: ContentLike> StackPage<H> for TitleScreen {
    type Key = Sec;
    type Cards<'a> = shelves::Cards<'a>;
    fn revision(&self, _: &Cx<'_, H>) -> u64 { self.revision }
    fn sections(&self, cx: &Cx<'_, H>, out: &mut Vec<SectionSpec<Sec>>) {
        let Some(ready) = self.ready() else { return };
        let hero = Section::Custom { height: paint::HERO_H, focusable: true };
        out.push(SectionSpec::new(Sec::Hero, hero, Sec::Hero.group()));
        if ready.about() {
            let about = Section::Custom { height: paint::about_h(ready, cx.measure), focusable: true };
            out.push(SectionSpec::new(Sec::About, about, Sec::About.group()));
        }
        for k in [Sec::Seasons, Sec::Cast, Sec::Videos, Sec::Episodes] {
            if self.cards_of(k).is_none() { continue; }
            out.push(SectionSpec::new(k, Section::Shelf { style: k.style(), heading: super::layout::HEAD_TO_ROW }, k.group())
                .seated(Seat::Projected).edges([EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop]).of_kind(ElemKind::Card));
        }
    }
    fn fallback(&self, _: &Cx<'_, H>, out: &mut Vec<Sec>) { out.extend(self.focus_sections()); }
    fn cards<'a>(&'a self, _: &'a Cx<'_, H>, k: Sec) -> Option<Self::Cards<'a>> { self.cards_of(k) }
    fn card_has_menu(&self, _: &Cx<'_, H>, _: Sec, _: &u32) -> bool { false }
    fn plain_len(&self, k: Sec) -> usize {
        match k { Sec::Hero => self.actions().iter().filter(|a| a.focusable()).count().max(1), Sec::About => 1, _ => 0 }
    }
    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Hero if n == 0 && !self.actions().iter().any(facts::Action::focusable) => Some(HERO_READ),
            Sec::Hero => self.actions().iter().filter_map(|a| a.act.elem()).nth(n),
            Sec::About if n == 0 => Some(ABOUT),
            _ => None,
        }
    }
    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        if k != Sec::Hero { return None; }
        match dir {
            Dir::Left => at.checked_sub(1),
            Dir::Right if at + 1 < <Self as StackPage<H>>::plain_len(self, k) => Some(at + 1),
            _ => None,
        }
    }
    fn focus_rect(&self, _: &Cx<'_, H>, k: Sec, section: Rect) -> Rect {
        if k == Sec::About { paint::about_rect(section) } else { section }
    }
    fn element_rect(&self, cx: &Cx<'_, H>, k: Sec, n: usize, section: Rect) -> Rect {
        if k == Sec::Hero {
            return paint::pills(self.actions(), section.y, cx.measure).filter(|(a, _)| a.focusable()).nth(n)
                .or_else(|| paint::pills(self.actions(), section.y, cx.measure).next()).map_or(section, |(_, r)| r);
        }
        self.focus_rect(cx, k, section)
    }
    fn plain_group(&self, _: &Cx<'_, H>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        GroupSpec { id, kind: GroupKind::Row { wrap: false }, seat: Seat::First, reachable: AxisMask::BOTH,
            edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop], extent,
            len: <Self as StackPage<H>>::plain_len(self, k),
            elem: if k == Sec::Hero && self.actions().iter().any(facts::Action::focusable) { ElemKind::Control } else { ElemKind::Bare } }
    }
    fn shelf_foot(&self, _: &Cx<'_, H>, _: Sec) -> f32 { UNDER_LABEL_AIR }
    fn wide_extent(&self, _: Sec) -> bool { false }
    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, lift: f32) {
        let word = match k {
            Sec::Seasons => msg::requests_seasons_c(),
            Sec::Cast => msg::browse_detail_cast_c(),
            Sec::Videos => msg::requests_videos_c(),
            Sec::Episodes => msg::requests_episodes_c(),
            _ => return,
        };
        let p = f.painter.alpha(f.page_alpha).translate(0.0, r.y);
        plx_ui::cards::draw_heading_c(p, word, c"", c"", MARGIN_X, -lift, 1.0, f.cx.measure);
    }
    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, _: f32) {
        let Some(ready) = self.ready() else { return };
        let p = f.painter.alpha(f.page_alpha);
        let hot = f.cx.focus.current.filter(|key| key.entry == self.entry).map(|key| key.elem);
        match k {
            Sec::Hero => paint::hero(p, r.y, ready, hot, self.sent, self.trailer_failed, self.progress.as_ref(), f.cx.measure),
            Sec::About => paint::about(p, r, ready, hot == Some(ABOUT), f.cx.measure),
            _ => {}
        }
    }
}

plx_ui::focusable_via_view!(TitleScreen, H: [ContentLike], view);

impl<H: ContentLike> Screen<H> for TitleScreen {
    fn glides(&self) -> bool { true }
    fn name(&self) -> &'static str { "request_title" }
    fn state(&self) -> &dyn LogicalState { self }
    fn crumb(&self, _: &Cx<'_, H>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed(msg::requests_title())) }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
    fn memory(&self) -> PageMemory { PageMemory::None }
    fn links(&self, out: &mut Vec<Link>) {
        let mut previous = None;
        for group in self.focus_sections().map(Sec::group) {
            if let Some(from) = previous {
                out.push(Link { from, dir: Dir::Down, to: group });
                out.push(Link { from: group, dir: Dir::Up, to: from });
            }
            previous = Some(group);
        }
    }
    fn prepare(&mut self, _: &mut plx_ui::frame::Budget, _: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        plx_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        let p = f.painter.alpha(f.page_alpha);
        let band = Rect::new(MARGIN_X, theme::space::XL, SCR_W - 2.0 * MARGIN_X, paint::HERO_H);
        match self.load {
            Load::Ready(_) => self.stack.view(self).paint(f),
            Load::Failed(error) => {
                let reason = std::ffi::CString::new(crate::registry::seerr_error_text(error)).unwrap_or_default();
                StatusOverlay::new(band, msg::requests_failed_c(), StatusKind::Failed).reason(&reason).draw(&Env::inert(), p);
            }
            // placeholder-exempt: counted where drawn (StatusOverlay::draw_geometry, Working)
            Load::Loading => StatusOverlay::new(band, msg::requests_loading_c(), StatusKind::Working).draw(&Env::inert(), p),
        }
    }
}

impl LogicalState for TitleScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.kind as u32).u32(self.tmdb).u64(self.revision);
        c.u32(match self.load { Load::Loading => 0, Load::Ready(_) => 1, Load::Failed(_) => 2 });
        c.u32(match self.sent { Sent::Idle => 0, Sent::Sending => 1, Sent::Done => 2, Sent::Failed(_) => 3 });
        c.seq(self.picked.len());
        for n in &self.picked { c.u32(*n); }
        self.stack.write_scroll(c);
        for k in Sec::ALL {
            if let Some(shelf) = self.stack.shelf(k) { shelf.write(c); }
        }
    }
    fn probe(&self, out: &mut String) {
        use std::fmt::Write;
        let percent = self.snapshot.as_ref().and_then(|snap| snap.percent).map_or(-1, i16::from);
        let _ = write!(out, "request-title entry={} tmdb={} sections={} ready={} percent={percent}",
            self.entry.0, self.tmdb, self.focus_sections().count(), u8::from(self.ready().is_some()));
    }
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
