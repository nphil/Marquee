//! The Tier 2 card-conformance harness for the Requests page (`cards_conformance_tests.rs`): the
//! Trending row, fed the way the driver feeds it. A child module of `requests` so it reads the
//! private rows and `row_frame` the way the page's own tests do.
use super::*;
use plx_machine::machine::{FocusRead, Host, PressRead, Tick};
use plx_machine::present::Present;
use plx_plex::plex::seerr::{MediaState, Page};
use plx_ui::cards::conformance::{CardHarness, Landing, Nb};
use plx_ui::fixture::FixtureMeasure;
use plx_ui::screen::{At, By, Focusable, Step};

/// A host with no stores: the page reads its rows from itself.
pub(crate) struct ReqHost;
impl Host for ReqHost {
    type Arg = crate::registry::SettingsPage;
    type Fx = AppFx;
    type Msg = crate::registry::AppMsg;
    type Elem = u32;
    type Views<'a> = ();
    type Init = crate::registry::NoInit;
    type Memory = PageMemory;
}

pub(crate) const ENTRY: EntryId = EntryId(66);
pub(crate) const INSTANCE: InstanceId = InstanceId(67);

/// A movie Seerr lists, `tmdb` its id.
pub(crate) fn title(tmdb: u32) -> Title {
    Title {
        kind: Kind::Movie,
        tmdb,
        name: format!("Movie {tmdb}"),
        year: "2000".into(),
        poster: String::new(),
        backdrop: String::new(),
        overview: String::new(),
        state: MediaState::default(),
    }
}

/// One page of `Trending` as the driver answers it.
pub(crate) fn trending(ids: &[u32]) -> RequestsReply {
    RequestsReply::Shelf {
        shelf: Discover::Trending,
        page: 1,
        result: Ok(Page { items: ids.iter().map(|id| title(*id)).collect(), page: 1, pages: 1 }),
    }
}

pub(crate) struct Harness {
    pub(crate) screen: RequestsScreen,
    ids: Vec<u32>,
    pub(crate) focus: Option<FocusKey<u32>>,
    press: f32,
    ms: u32,
}

pub(crate) fn mount(n: usize) -> Box<dyn CardHarness> {
    Box::new(Harness::new((0..n as u32).map(|i| 100 + i).collect()))
}

impl Harness {
    pub(crate) fn new(ids: Vec<u32>) -> Self {
        let mut screen = RequestsScreen::new(ENTRY, INSTANCE);
        screen.receive(trending(&ids));
        let mut h = Self { screen, ids, focus: None, press: 1.0, ms: 0 };
        h.step(ScreenEvent::Mount);
        h
    }

    pub(crate) fn cx(&self) -> Cx<'_, ReqHost> {
        Cx {
            views: (),
            tick: Tick { ms: self.ms, dt_us: 16_667 },
            measure: &FixtureMeasure,
            press: PressRead { scale: self.press, ..Default::default() },
            focus: FocusRead { current: self.focus, ..Default::default() },
            owner: InputOwner::Entry(ENTRY),
        }
    }

    /// Step one event; the effects it queued and whether the step reported page motion.
    pub(crate) fn step_fx(&mut self, ev: ScreenEvent<ReqHost>) -> (Vec<plx_machine::machine::Stamped<ReqHost>>, bool) {
        let mut present = Present::new();
        let mut out = Vec::new();
        let (_, moving) = plx_machine::idle::scoped_motion(|| {
            let cx = Cx {
                views: (),
                tick: Tick { ms: self.ms, dt_us: 16_667 },
                measure: &FixtureMeasure,
                press: PressRead { scale: self.press, ..Default::default() },
                focus: FocusRead { current: self.focus, ..Default::default() },
                owner: InputOwner::Entry(ENTRY),
            };
            let mut fx = Effects::new(&mut out, MachineId::Instance(INSTANCE), &mut present);
            Machine::<ReqHost>::step(&mut self.screen, &ev, &cx, &mut fx);
        });
        (out, moving || present.page_moving())
    }

    pub(crate) fn step(&mut self, ev: ScreenEvent<ReqHost>) -> bool {
        self.step_fx(ev).1
    }

    pub(crate) fn key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: ENTRY, elem }
    }

    pub(crate) fn focus_elem(&mut self, elem: u32, by: By) {
        let from = self.focus;
        self.focus = Some(self.key(elem));
        self.step(ScreenEvent::FocusMoved { from, to: self.key(elem), by });
    }

    fn lane_cards(&self) -> Vec<&Card> {
        self.screen.shown().flat_map(|r| r.cards.iter()).collect()
    }

    fn republish(&mut self) {
        self.screen.receive(trending(&self.ids));
        self.step(ScreenEvent::Tick(Tick { ms: self.ms, dt_us: 16_667 }));
    }
}

impl CardHarness for Harness {
    fn cards(&self) -> Vec<u32> {
        self.lane_cards().iter().map(|c| c.elem).collect()
    }
    fn focused(&self) -> Option<u32> {
        self.focus.map(|k| k.elem)
    }
    fn focus(&mut self, elem: u32, by: By) {
        self.focus_elem(elem, by);
    }
    fn tick(&mut self, frames: u32) -> bool {
        let mut moved = false;
        for _ in 0..frames {
            self.ms += 16;
            moved |= self.step(ScreenEvent::Tick(Tick { ms: self.ms, dt_us: 16_667 }));
        }
        moved
    }
    fn cover(&mut self) {
        self.step(ScreenEvent::Cover);
    }
    fn uncover(&mut self) {
        self.step(ScreenEvent::Uncover);
    }
    fn set_press(&mut self, scale: f32) {
        self.press = scale;
    }
    fn neighbour(&self, elem: u32, dir: plx_ui::screen::Dir) -> Nb {
        match Focusable::<ReqHost>::neighbour(&self.screen, self.key(elem), dir, &self.cx()) {
            Step::Move(k) => Nb::To(k.elem),
            Step::Edge => Nb::Edge,
        }
    }
    fn place(&self, elem: u32, at: At) -> Option<Placed> {
        Focusable::<ReqHost>::place(&self.screen, &elem, &self.cx(), at)
    }
    /// The rect the page registers as the stop of `elem`: the shelf's draw registers the rect it
    /// paints.
    fn drawn_rect(&self, elem: u32, _press: f32) -> Option<Rect> {
        let cx = self.cx();
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        let src = self.screen.cards_of(Sec::Row(Lane::Trending, self.screen.rows_gen))?;
        let root = frame.painter;
        if let Some((at, shelf)) = self.screen.row_frame(&cx, Lane::Trending) {
            shelf.record_stops(&mut frame, root, &src, at);
        }
        frame.stops().iter().find(|s| s.key.elem == elem).map(|s| s.rect)
    }
    fn scale(&self, elem: u32) -> Option<f32> {
        let cx = self.cx();
        let src = self.screen.cards_of(Sec::Row(Lane::Trending, self.screen.rows_gen))?;
        self.screen.row_frame(&cx, Lane::Trending)?.1.scale_of(&cx, &src, &elem)
    }
    fn focus_scale(&self) -> f32 {
        layout::POSTER.focus_scale
    }
    fn canon(&self) -> u64 {
        LogicalState::hash(&self.screen)
    }
    fn identity(&self, elem: u32) -> String {
        self.lane_cards().iter().find(|c| c.elem == elem).map(|c| c.tmdb.to_string()).unwrap_or_default()
    }
    fn scroll(&self) -> Option<f32> {
        self.screen.row_frame(&self.cx(), Lane::Trending).map(|(_, shelf)| shelf.scroll())
    }
    fn scroll_max(&self) -> Option<f32> {
        self.screen
            .row_frame(&self.cx(), Lane::Trending)
            .map(|(_, shelf)| shelf.style().max_scroll(self.ids.len()))
    }
    fn landing(&mut self, l: Landing) -> Result<(), &'static str> {
        let focused = self.focus.ok_or("nothing focused")?.elem;
        let col = self.lane_cards().iter().position(|c| c.elem == focused).ok_or("nothing focused")?;
        match l {
            Landing::Reorder => self.ids.swap(0, 2),
            Landing::InsertAbove => self.ids.insert(0, 9_000),
            Landing::RemoveFocused => {
                self.ids.remove(col);
            }
        }
        self.republish();
        let want = self.focus.unwrap();
        let now = Focusable::<ReqHost>::reconcile(&self.screen, want, &self.cx());
        if now != want {
            self.focus_elem(now.elem, By::Reconcile);
        }
        Ok(())
    }
    fn shift(&mut self, by: usize) -> Result<(), &'static str> {
        for k in 0..by as u32 {
            self.ids.insert(0, 9_100 + k);
        }
        self.republish();
        Ok(())
    }
    fn memory_roundtrip(&mut self) -> Result<Box<dyn CardHarness>, &'static str> {
        let mem = Screen::<ReqHost>::memory_at(&self.screen, self.focus);
        let mut fresh = Harness::new(self.ids.clone());
        fresh.step(ScreenEvent::RestoreMemory(mem));
        let want = self.focus.ok_or("nothing focused")?;
        let now = Focusable::<ReqHost>::reconcile(&fresh.screen, want, &fresh.cx());
        fresh.focus_elem(now.elem, By::Restore);
        Ok(Box::new(fresh))
    }
}
