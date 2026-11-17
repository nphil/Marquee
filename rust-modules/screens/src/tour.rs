//! **The tour: six short pages on how to use the remote**, shown once per profile after the
//! Favorite-libraries question and again whenever Settings → *Show the tour again* asks.
//!
//! It is a page of the Settings surface ([`SettingsPage::Tour`]) so it takes the surface's ground,
//! crumb and BACK handling for free. First run (`Tour(true)`) has no crumb and says how to skip;
//! from Settings (`Tour(false)`) the crumb names Settings and BACK returns there.
//!
//! Each lesson is words in the narrative column and a picture in the content column built from
//! the app's own parts: the key caps the alerts use, the back crumb, and the shelves' own poster
//! row. One band pill (*Next*, then *Done*) steps through; LEFT/RIGHT step too. BACK leaves at
//! any time. Finishing or skipping asks the loop to remember it for the seated profile
//! ([`LoopReq::TourSeen`]): a screen cannot write the session itself.

use std::borrow::Cow;
use std::ffi::CStr;

use plx_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, Fx, Handled, InputEvent, InputKind, Key, LogicalState, Machine, Measure, NavOp,
};
use plx_ui::cards::{RowStyle, SampleFrame, SampleRow, TileLabel};
use plx_ui::frame::Budget;
use plx_ui::icons::Icon;
use plx_ui::route_screen::{draw_back_crumb, RouteLayout, CRUMB_BAND};
use plx_ui::screen::{DrawFrame, FocusSource, HitSource, Part, RenderStrategy, Screen, ScreenEvent};
use plx_ui::table_screen::{pill_w, BandPart, Header};
use plx_ui::widgets::{key_cap_w_with, key_cap_with, Art, CapFace, CapLook, CapMetrics, CtlPop, KeyHint, KEYCAP_H};
use plx_ui::{theme, Painter, Rect};

use super::family::{palette, InnerHost, TABLE_GROUP};
use super::registry::{band_index, AppFx, LoopReq};

/// What one page teaches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lesson {
    Move,
    Back,
    Menu,
    Controls,
    Seek,
    Profile,
}

const LESSONS: [Lesson; 6] =
    [Lesson::Move, Lesson::Back, Lesson::Menu, Lesson::Controls, Lesson::Seek, Lesson::Profile];

impl Lesson {
    fn title(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Lesson::Move => msg::tour_move_title(),
            Lesson::Back => msg::tour_back_title(),
            Lesson::Menu => msg::tour_menu_title(),
            Lesson::Controls => msg::tour_controls_title(),
            Lesson::Seek => msg::tour_seek_title(),
            Lesson::Profile => msg::tour_profile_title(),
        }
    }
    fn copy(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Lesson::Move => msg::tour_move_copy(),
            Lesson::Back => msg::tour_back_copy(),
            Lesson::Menu => msg::tour_menu_copy(),
            Lesson::Controls => msg::tour_controls_copy(),
            Lesson::Seek => msg::tour_seek_copy(),
            Lesson::Profile => msg::tour_profile_copy(),
        }
    }
    /// The pictures that show a key being held fill a cap over and over; the rest are still.
    fn holds(self) -> bool {
        matches!(self, Lesson::Menu | Lesson::Seek)
    }
}

/// The band's one pill: *Next*, then *Done* on the last page.
fn pill_label(page: usize) -> &'static CStr {
    if page + 1 >= LESSONS.len() {
        plx_platform::i18n::msg::settings_done_c()
    } else {
        plx_platform::i18n::msg::tour_next_c()
    }
}

/// How full a held cap is `ms` into the loop: it fills, then rests full before it starts over.
fn hold_fill(ms: f32) -> f32 {
    const FILL_MS: f32 = 1400.0;
    const REST_MS: f32 = 700.0;
    (ms.rem_euclid(FILL_MS + REST_MS) / FILL_MS).min(1.0)
}

struct TourState {
    page: u8,
    first_run: bool,
}

impl LogicalState for TourState {
    fn write(&self, w: &mut Canon) {
        w.u8(self.page);
        w.u8(u8::from(self.first_run));
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("tour {}{}", self.page, if self.first_run { " first" } else { "" }));
    }
}

pub struct TourPage {
    entry: EntryId,
    state: TourState,
    pop: CtlPop<1>,
    clock: plx_machine::motion::Phase,
    held_ms: f32,
    progress: std::ffi::CString,
    /// The band's one pill, kept so the band view can borrow it.
    labels: [&'static CStr; 1],
}

impl TourPage {
    pub fn new(entry: EntryId, first_run: bool) -> Self {
        let mut page = Self {
            entry,
            state: TourState { page: 0, first_run },
            pop: CtlPop::new(),
            clock: Default::default(),
            held_ms: 0.0,
            progress: Default::default(),
            labels: [pill_label(0)],
        };
        page.show(0);
        page
    }

    fn lesson(&self) -> Lesson {
        LESSONS[self.state.page as usize]
    }

    fn show(&mut self, page: usize) {
        self.state.page = page.min(LESSONS.len() - 1) as u8;
        self.clock = Default::default();
        self.held_ms = 0.0;
        let line = plx_platform::i18n::msg::tour_step(self.state.page as i64 + 1, LESSONS.len() as i64);
        self.progress = std::ffi::CString::new(line).unwrap_or_default();
        self.labels = [pill_label(self.state.page as usize)];
    }

    /// Remember the tour as seen and leave.
    fn finish(&self, fx: &mut Effects<'_, InnerHost>) {
        fx.push(Fx::App(AppFx::Loop(LoopReq::TourSeen)));
        fx.push(Fx::Nav(NavOp::Pop));
    }

    fn next(&mut self, fx: &mut Effects<'_, InnerHost>) {
        let page = self.state.page as usize;
        if page + 1 >= LESSONS.len() {
            self.finish(fx);
        } else {
            self.show(page + 1);
            fx.invalidate(plx_machine::present::Provenance::Input);
        }
    }

    fn previous(&mut self, fx: &mut Effects<'_, InnerHost>) {
        if let Some(page) = (self.state.page as usize).checked_sub(1) {
            self.show(page);
            fx.invalidate(plx_machine::present::Provenance::Input);
        }
    }

    fn view(&self) -> BandPart<'_> {
        BandPart {
            layout: RouteLayout::screen(),
            labels: &self.labels,
            group: TABLE_GROUP,
            entry: self.entry,
            // LEFT is the tour's own "previous page", never a way out of it.
            uncommitted: true,
            scales: [self.pop.scale(0), 1.0],
            palette: palette(),
            danger: None,
        }
    }
}

impl Machine<InnerHost> for TourPage {
    type Ev = ScreenEvent<InnerHost>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, InnerHost>, fx: &mut Effects<'_, InnerHost>) -> Handled {
        match ev {
            ScreenEvent::Tick(t) => {
                self.pop.step(cx.focus.current.and_then(|k| band_index(k.elem)), t.dt());
                if self.lesson().holds() {
                    self.held_ms = self.clock.advance(*t, &mut fx.present());
                }
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                self.next(fx);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Key { key, edge: Edge::Down, .. }, ..
            }) => match key {
                Key::Right => {
                    // forward only to the last page; leaving is OK on Done, or BACK
                    if (self.state.page as usize) + 1 < LESSONS.len() {
                        self.next(fx);
                    }
                    Handled::Yes
                }
                Key::Left => {
                    self.previous(fx);
                    Handled::Yes
                }
                Key::Back => {
                    self.finish(fx);
                    Handled::Yes
                }
                _ => Handled::No,
            },
            ScreenEvent::Enter(_) => Handled::Yes,
            _ => Handled::No,
        }
    }
}
plx_ui::focusable_via_view!(TourPage, InnerHost, view);

impl Screen<InnerHost> for TourPage {
    fn name(&self) -> &'static str {
        "tour"
    }
    fn state(&self) -> &dyn LogicalState {
        &self.state
    }
    fn crumb(&self, _cx: &Cx<'_, InnerHost>) -> Option<Cow<'_, str>> {
        (!self.state.first_run).then(|| Cow::Borrowed(plx_platform::i18n::msg::settings_title()))
    }
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, InnerHost>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, InnerHost>) {
        let p = f.painter;
        let layout = RouteLayout::screen();
        let lesson = self.lesson();
        let crumb = (!self.state.first_run).then(plx_platform::i18n::msg::settings_title);
        let mut header = Header::new(layout, crumb, lesson.title(), lesson.copy());
        if self.state.first_run {
            let skip = plx_platform::i18n::msg::tour_skip_hint("\u{fffc}");
            header = header.with_hint(KeyHint::translated(skip, c"BACK"));
        }
        header.paint(p, f.measure);

        let mut band = self.view();
        Part::<InnerHost>::draw(&mut band, f, layout.action);
        // "2 of 6", level with the pill and just past it.
        let m = f.measure;
        let size = theme::size::CAPTION;
        let x = layout.action.x + pill_w(m, self.labels[0], theme::size::BODY) + theme::space::MD;
        let y = plx_gfx::text::text_vcenter_y(size, 0, layout.action.y + layout.action.h * 0.5);
        p.text(self.progress.as_ptr(), x, y, size, theme::TEXT_TERTIARY, 0, 0);

        draw_picture(p, lesson, layout.content, self.held_ms, m);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
}

// ---------------------------------------------------------------------------------------------
// the pictures
// ---------------------------------------------------------------------------------------------

/// A touch roomier than the alerts' cap so the remote reads from the sofa.
const CAP: CapMetrics = CapMetrics { min_w: 104.0, pad_x: 16.0 };
/// The air between caps, and between a picture's parts.
const GAP: f32 = theme::space::SM;
const BLOCK_GAP: f32 = theme::space::LG;

fn cap(p: Painter, cx: f32, cy: f32, face: CapFace<'_>, held: Option<f32>, m: &dyn Measure) {
    let w = key_cap_w_with(face, CAP, m);
    let look = CapLook {
        ring: theme::TEXT_SECONDARY,
        label: theme::TEXT_PRIMARY,
        fill: held.map(|fraction| (fraction, theme::OVERLAY_FOCUS_PILL)),
        metrics: CAP,
    };
    key_cap_with(p, cx - w * 0.5, cy, face, look, m);
}

fn word(p: Painter, cx: f32, cy: f32, text: &CStr, ink: [f32; 4], m: &dyn Measure) {
    let size = theme::size::LABEL;
    let w = m.width(text, size, false);
    p.text(text.as_ptr(), cx - w * 0.5, plx_gfx::text::text_vcenter_y(size, 0, cy), size, ink, 0, 0);
}

fn arrow(icon: Icon) -> CapFace<'static> {
    CapFace::Glyph(icon)
}

/// The lesson's picture, centred in the content column.
fn draw_picture(p: Painter, lesson: Lesson, area: Rect, held_ms: f32, m: &dyn Measure) {
    use plx_platform::i18n::msg;
    let (cx, cy) = (area.x + area.w * 0.5, area.y + area.h * 0.5);
    let row = KEYCAP_H + GAP;
    let held = hold_fill(held_ms);
    match lesson {
        Lesson::Move => {
            let col = key_cap_w_with(arrow(Icon::Chevron), CAP, m) + GAP;
            cap(p, cx, cy - row, arrow(Icon::ChevronUp), None, m);
            cap(p, cx - col, cy, arrow(Icon::ChevronLeft), None, m);
            cap(p, cx, cy, CapFace::Label(c"OK"), None, m);
            cap(p, cx + col, cy, arrow(Icon::Chevron), None, m);
            cap(p, cx, cy + row, arrow(Icon::ChevronDown), None, m);
        }
        Lesson::Back => {
            cap(p, cx, cy - row, CapFace::Label(c"BACK"), None, m);
            let w = 320.0_f32.min(area.w);
            let band = Rect::new(cx - w * 0.5 + GAP, cy + GAP, w, CRUMB_BAND);
            draw_back_crumb(p, band, msg::settings_title(), m);
        }
        Lesson::Menu => {
            let style = RowStyle { gap: 35.0, ..RowStyle::HOME };
            let pad = 60.0;
            let frame_h = style.h + 2.0 * pad;
            let top = cy - (frame_h + BLOCK_GAP + KEYCAP_H) * 0.5;
            let rect = Rect::new(cx - area.w.min(820.0) * 0.5, top, area.w.min(820.0), frame_h);
            let frame = SampleFrame {
                rect,
                row: Rect::new(rect.x, rect.y + pad, rect.w, style.h),
                rest_alpha: 0.35,
                foot: None,
            };
            let tiles = (0..3).map(|_| Art::Poster(None));
            SampleRow::draw(p, &frame, &style, tiles, 1, &TileLabel::default(), m);
            cap(p, cx, rect.y + frame_h + BLOCK_GAP + KEYCAP_H * 0.5, CapFace::Label(c"OK"), Some(held), m);
        }
        Lesson::Controls => {
            cap(p, cx, cy - row, arrow(Icon::ChevronUp), None, m);
            cap(p, cx, cy, arrow(Icon::ChevronDown), None, m);
            let tabs = cy + row + GAP;
            word(p, cx - 90.0, tabs, msg::widgets_player_info_c(), theme::TEXT_SECONDARY, m);
            word(p, cx + 90.0, tabs, msg::widgets_player_chapters_c(), theme::TEXT_SECONDARY, m);
        }
        Lesson::Seek => {
            let col = key_cap_w_with(arrow(Icon::Chevron), CAP, m) * 0.5 + GAP;
            cap(p, cx - col, cy, arrow(Icon::ChevronLeft), None, m);
            cap(p, cx + col, cy, arrow(Icon::Chevron), Some(held), m);
        }
        Lesson::Profile => {
            cap(p, cx, cy - row, arrow(Icon::ChevronUp), None, m);
            cap(p, cx, cy, CapFace::Label(c"OK"), None, m);
            word(p, cx, cy + row + GAP, msg::settings_account_change_profile_c(), theme::TEXT_SECONDARY, m);
            word(p, cx, cy + 2.0 * row + GAP, msg::settings_title_c(), theme::TEXT_SECONDARY, m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_base::fontcov::advances::ShippedMeasure;
    use plx_machine::machine::{FocusRead, InputOwner, InstanceId, MachineId, PressRead, Source, Stamped, Tick};
    use plx_machine::present::Present;
    use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    fn press(key: Key) -> ScreenEvent<InnerHost> {
        ScreenEvent::Input(InputEvent {
            at: Tick::default(),
            source: Source::Script,
            kind: InputKind::Key { key, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
        })
    }

    /// One `step` of the real machine, returning its answer and what it emitted.
    fn step(page: &mut TourPage, ev: &ScreenEvent<InnerHost>) -> (Handled, Vec<Stamped<InnerHost>>) {
        let m = plx_ui::fixture::FixtureMeasure;
        let cx = Cx::<InnerHost> {
            views: plx_data::stores::browse::DirectoryView::empty_for_test(),
            tick: Tick::default(),
            measure: &m,
            press: PressRead::default(),
            focus: FocusRead::default(),
            owner: InputOwner::Entry(EntryId(0)),
        };
        let mut present = Present::new();
        let mut buf = Vec::new();
        let handled = {
            let mut fx = Effects::new(&mut buf, MachineId::Instance(InstanceId(0)), &mut present);
            Machine::<InnerHost>::step(page, ev, &cx, &mut fx)
        };
        (handled, buf)
    }

    fn saved_and_left(effects: &[Stamped<InnerHost>]) -> (bool, bool) {
        let saved = effects.iter().any(|e| matches!(e.fx, Fx::App(AppFx::Loop(LoopReq::TourSeen))));
        let left = effects.iter().any(|e| matches!(e.fx, Fx::Nav(NavOp::Pop)));
        (saved, left)
    }

    #[test]
    fn every_page_has_a_title_and_copy() {
        for lesson in LESSONS {
            assert!(!lesson.title().is_empty() && !lesson.copy().is_empty(), "{lesson:?}");
        }
    }

    #[test]
    fn back_on_any_page_remembers_the_tour_and_leaves() {
        for first_run in [true, false] {
            let mut page = TourPage::new(EntryId(0), first_run);
            step(&mut page, &press(Key::Right));
            let (handled, effects) = step(&mut page, &press(Key::Back));
            assert_eq!(handled, Handled::Yes);
            assert_eq!(saved_and_left(&effects), (true, true));
        }
    }

    #[test]
    fn right_and_left_walk_the_pages_without_leaving() {
        let mut page = TourPage::new(EntryId(0), true);
        for expected in 1..LESSONS.len() {
            let (_, effects) = step(&mut page, &press(Key::Right));
            assert_eq!(page.state.page as usize, expected);
            assert_eq!(saved_and_left(&effects), (false, false));
        }
        step(&mut page, &press(Key::Right));
        assert_eq!(page.state.page as usize, LESSONS.len() - 1, "RIGHT stops at the last page");
        step(&mut page, &press(Key::Left));
        assert_eq!(page.state.page as usize, LESSONS.len() - 2);
        let mut first = TourPage::new(EntryId(0), true);
        let (handled, effects) = step(&mut first, &press(Key::Left));
        assert_eq!((handled, first.state.page, saved_and_left(&effects)), (Handled::Yes, 0, (false, false)));
    }

    #[test]
    fn the_pill_steps_on_and_done_on_the_last_page_remembers_the_tour() {
        let mut page = TourPage::new(EntryId(0), false);
        let commit = ScreenEvent::PressCommit(plx_machine::machine::PressId(0));
        for _ in 1..LESSONS.len() {
            let (_, effects) = step(&mut page, &commit);
            assert_eq!(saved_and_left(&effects), (false, false));
        }
        assert_eq!(page.labels, [plx_platform::i18n::msg::settings_done_c()]);
        let (_, effects) = step(&mut page, &commit);
        assert_eq!(saved_and_left(&effects), (true, true));
    }

    #[test]
    fn a_held_cap_fills_then_rests_full_then_starts_over() {
        assert_eq!(hold_fill(0.0), 0.0);
        assert!((hold_fill(700.0) - 0.5).abs() < 1e-3);
        assert_eq!(hold_fill(1800.0), 1.0);
        assert!(hold_fill(2110.0) < 0.05);
    }

    #[test]
    fn only_the_hold_lessons_animate() {
        let moving: Vec<_> = LESSONS.iter().filter(|l| l.holds()).collect();
        assert_eq!(moving, [&Lesson::Menu, &Lesson::Seek]);
    }

    /// Every page's words fit the narrative column in every shipped language, measured the way the
    /// device does: the copy and the skip hint end above the action band, the hint and the
    /// "n of 6" line fit on one line.
    #[test]
    fn every_page_fits_its_column_in_every_language() {
        use plx_ui::text_view::TextView;
        let layout = RouteLayout::screen();
        let mut out = Vec::new();
        for language in SHIPPED {
            let _guard = language_on_this_thread_for_test(language);
            let tag = language.tag();
            let message = plx_platform::i18n::msg::tour_skip_hint("\u{fffc}");
            if message.matches('\u{fffc}').count() != 1 {
                out.push(format!("{tag}: the skip hint needs exactly one key placeholder: {message:?}"));
            }
            let hint_w = KeyHint::translated(message, c"BACK").width(&ShippedMeasure);
            if hint_w > layout.narrative.w {
                out.push(format!("{tag}: skip hint {hint_w:.0}px, past the {:.0}px column", layout.narrative.w));
            }
            for (page, lesson) in LESSONS.iter().enumerate() {
                let frame = layout.narrative_copy_frame(true, lesson.title(), layout.action.y, &ShippedMeasure);
                let copy_h = TextView::new(lesson.copy(), theme::size::LABEL, theme::TEXT_READING)
                    .leading(theme::size::LABEL as f32 + theme::space::XS)
                    .max_lines(12)
                    .with_measure(&ShippedMeasure)
                    .measure_h(layout.narrative.w);
                let bottom = frame.y + copy_h + theme::space::MD + KeyHint::height();
                if bottom > layout.action.y - theme::space::XL {
                    out.push(format!("{tag}: page {} ends at {bottom:.0}, past the action band", page + 1));
                }
                let label = pill_label(page);
                let step = std::ffi::CString::new(plx_platform::i18n::msg::tour_step(page as i64 + 1, 6)).unwrap();
                let line = pill_w(&ShippedMeasure, label, theme::size::BODY) + theme::space::MD
                    + ShippedMeasure.width(&step, theme::size::CAPTION, false);
                if line > layout.action.w {
                    out.push(format!("{tag}: page {} pill and progress are {line:.0}px wide", page + 1));
                }
            }
        }
        assert!(out.is_empty(), "{}", out.join("\n"));
    }
}
