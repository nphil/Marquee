//! The Requests page's paint: the search pill, the poster rows and the status read-out.
use super::*;
use plx_ui::consts::MARGIN_X;
use plx_ui::icons::Icon;
use plx_ui::label::{HAlign, Label};
use plx_ui::widgets::{focus_pill, Button, ControlStyle, StatusKind, StatusOverlay, BTN_PILL_AIR, CTRL_FOCUS_SCALE};
use plx_ui::{theme, Env, Painter, View};
use std::ffi::CString;
use std::rc::Rc;

const CARET_W: f32 = 4.0;
const CARET_GAP: f32 = 6.0;

/// The pill's words as drawn, measured once per edit.
#[derive(Default)]
pub(super) struct Resources {
    query: String,
    caret: usize,
    run: Option<Rc<std::ffi::CStr>>,
    /// Where the caret stands, from the pill's centre: the button centres its icon and words as one
    /// run, so the words start at `run / 2 - words` from the centre.
    caret_dx: f32,
}

impl Resources {
    pub(super) fn prepare<H: ContentLike>(&mut self, query: &str, caret: usize, cx: &Cx<'_, H>) {
        if self.run.is_some() && self.query == query && self.caret == caret {
            return;
        }
        self.query.clear();
        self.query.push_str(query);
        self.caret = caret;
        let size = theme::size::BODY;
        // What the pill carries around its words: the icon, its gap and the air.
        let chrome = Button::pill_w_measured(c"", size, true, None, cx.measure);
        let width = layout::FIELD.w - chrome - CARET_W - CARET_GAP;
        let blank = query.trim().is_empty();
        let words = if blank { plx_platform::i18n::msg::requests_placeholder() } else { query };
        let (run, words_w) = cx.measure.fit_line_measured(words, width, size, true);
        let head_w = if blank { 0.0 } else { cx.measure.width_str(query.get(..caret).unwrap_or(query), size, true).min(words_w) };
        let run_w = Button::pill_w_measured(&run, size, true, None, cx.measure) - BTN_PILL_AIR;
        self.caret_dx = run_w / 2.0 - words_w + head_w;
        self.run = Some(run);
    }
}

pub(super) fn draw<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>) {
    plx_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
    let p = f.painter.alpha(f.page_alpha);
    field(screen, f, p);
    if screen.bar_shown() {
        bar(screen, f, p);
        chooser(screen, f, p);
        actions(screen, f, p);
    }
    match screen.note() {
        Some(note) => note_read(screen, note, p),
        None if screen.shown().next().is_none() => status(screen, p),
        None => {}
    }
    if screen.matches.open && screen.bar_shown() {
        let key = Sec::Grid(screen.grid_gen);
        if let (Some(grid), Some(src)) = (screen.stack.grid(key), screen.cards_of(key)) {
            grid.draw(f, p, &src);
        }
        return;
    }
    for row in screen.shown() {
        let Some((frame, shelf)) = screen.row_frame(f.cx, row.lane) else { continue };
        let top = frame.y - layout::HEAD_TO_ROW;
        if !plx_ui::on_axis(top, layout::block_h(shelf.under_band()), SCR_H, 0.0) {
            continue;
        }
        plx_ui::cards::draw_heading_c(p, row.lane.heading(), c"", c"", MARGIN_X, top - shelf.heading_lift(), 1.0, f.cx.measure);
        if let Some(src) = screen.cards_of(screen.sec(row.lane)) {
            shelf.draw(f, p, &src, frame);
        }
    }
}

fn field<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    let rect = Rect::new(layout::FIELD.x, layout::FIELD.y - screen.stack.scroll(), layout::FIELD.w, layout::FIELD.h);
    let editing = screen.editing && !screen.theme_editing();
    let hot = editing || f.cx.focus.current == Some(screen.key(FIELD));
    if let Some(run) = &screen.render.run {
        let face = if hot { focus_pill(rect) } else { rect };
        Button::new(run.as_ptr(), theme::size::BODY, face)
            .style(ControlStyle::Glass { selected: false })
            .icon(Icon::Search)
            .focused(hot)
            .draw(&Env::inert(), p);
        if editing && screen.blink_us < BLINK_US {
            caret(p, rect, screen.render.caret_dx);
        }
    }
    stop(screen, f, p, FIELD);
}
fn caret(p: Painter, rect: Rect, dx: f32) {
    let (cap_top, cap_base) = plx_gfx::text::text_cap_band(theme::size::BODY, 1);
    let y = plx_gfx::text::text_vcenter_y(theme::size::BODY, 1, rect.cy());
    p.rect(Rect::new(rect.cx() + dx, y + cap_top, CARET_W, cap_base - cap_top),
        0.0, theme::TEXT_PRIMARY, theme::TEXT_PRIMARY, 0.0);
}


/// What the page says where its rows would be: it is reaching Seerr, it could not, or nothing is there.
fn status(screen: &RequestsScreen, p: Painter) {
    use plx_platform::i18n::msg;
    let mut rect = layout::status_band(screen.editing, screen.bar_shown());
    if screen.bar_shown() { rect.y += screen.movie_content_top() - layout::CONTENT_TOP; }
    rect.y -= screen.stack.scroll();
    let env = Env::inert();
    if screen.results.is_some() {
        if screen.searching {
            StatusOverlay::new(rect, msg::requests_searching_c(), StatusKind::Working).draw(&env, p);
        } else {
            let line = CString::new(msg::requests_no_results(&screen.searched)).unwrap_or_default();
            StatusOverlay::new(rect, &line, StatusKind::Empty).draw(&env, p);
        }
        return;
    }
    match (screen.phase, &screen.failure) {
        (Phase::Failed, Some(reason)) => {
            StatusOverlay::new(rect, msg::requests_failed_c(), StatusKind::Failed).reason(reason).draw(&env, p)
        }
        (Phase::Failed, None) => StatusOverlay::new(rect, msg::requests_failed_c(), StatusKind::Failed).draw(&env, p),
        // placeholder-exempt: counted where drawn (StatusOverlay::draw_geometry, Working)
        _ if !screen.settled() => StatusOverlay::new(rect, msg::requests_loading_c(), StatusKind::Working).draw(&env, p),
        _ => StatusOverlay::new(rect, msg::requests_empty_c(), StatusKind::Empty).draw(&env, p),
    }
}

/// What a filter with nothing on its rows says, in the band the page keeps for it under the bar.
fn note_read(screen: &RequestsScreen, note: filters::Note, p: Painter) {
    use plx_platform::i18n::msg;
    let rect = Rect::new(0.0, screen.movie_content_top() - screen.stack.scroll(), SCR_W, layout::NOTE_H);
    let env = Env::inert();
    match note {
        // placeholder-exempt: counted where drawn (StatusOverlay::draw_geometry, Working)
        filters::Note::Working => StatusOverlay::new(rect, msg::requests_loading_c(), StatusKind::Working).draw(&env, p),
        filters::Note::Failed => StatusOverlay::new(rect, msg::requests_failed_c(), StatusKind::Failed).draw(&env, p),
        filters::Note::Empty => StatusOverlay::new(rect, msg::requests_filter_none_c(), StatusKind::Empty)
            .reason(msg::requests_filter_none_reason_c())
            .draw(&env, p),
    }
}

/// Paint the same glass language in the browse bar, the chooser and the grid's actions.
fn control(p: Painter, measure: &dyn plx_machine::machine::Measure, rect: Rect, label: &std::ffi::CStr,
    icon: Option<Icon>, selected: bool, focused: bool)
{
    let size = theme::size::BODY;
    let chrome = Button::pill_w_measured(c"", size, icon.is_some(), None, measure);
    let budget = (rect.w - chrome).max(0.0);
    let fit;
    let label = if measure.width(label, size, true) <= budget {
        label
    } else {
        fit = measure.fit_line(label.to_str().unwrap_or_default(), budget, size, true);
        &fit
    };
    // Leave half the gap clear on each side even for the wide four-across choices.
    let scale = if focused { CTRL_FOCUS_SCALE.min(1.0 + theme::space::SM / rect.w) } else { 1.0 };
    let mut button = Button::new(label.as_ptr(), size, rect)
        .style(ControlStyle::Glass { selected })
        .scale(scale)
        .focused(focused);
    if let Some(icon) = icon { button = button.icon(icon); }
    button.draw(&Env::inert(), p);
}

fn bar<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    let section = Rect::new(0.0, layout::HEAD_H - screen.stack.scroll(), SCR_W, layout::BAR_H);
    if !plx_ui::on_axis(section.y, section.h, SCR_H, 0.0) { return; }
    for (n, pill) in screen.pills.iter().enumerate() {
        let rect = screen.bar_rect(f.cx.measure, n, section);
        let selected = matches!(pill.icon, Some(Icon::Check));
        control(p, f.cx.measure, rect, &pill.label, pill.icon, selected, f.cx.focus.current == Some(screen.key(pill.elem)));
        stop(screen, f, p, pill.elem);
    }
}

fn chooser<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    let Some(chooser) = &screen.chooser else { return };
    let measure = f.cx.measure;
    let focus = f.cx.focus.current.filter(|k| k.entry == screen.entry).map(|k| k.elem);
    let section = Rect::new(0.0, screen.chooser_top() - screen.stack.scroll(), SCR_W, chooser.height());
    if !plx_ui::on_axis(section.y, section.h, SCR_H, 0.0) { return; }
    let room = SCR_W - 2.0 * MARGIN_X;
    let title = chooser.title();
    let label = Rect::new(MARGIN_X, section.y, room, theme::space::XL);
    Label::new(title.as_ptr(), theme::size::HEADLINE, theme::TEXT_PRIMARY).bold().draw(p, label);
    if chooser.pick == filters::Pick::Themes {
        use plx_platform::i18n::msg;
        let text = msg::requests_filter_theme_rule_c();
        Label::new(text.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY).h(HAlign::Right).draw(p, label);
    }
    let slot = focus.filter(|e| filters::is_chip(*e)).map(|e| (e - filters::CHIP_BASE) as usize);
    for n in chooser.visible(slot) {
        let choice = &chooser.choices[n];
        let Some(rect) = screen.choice_rect(focus, n, section) else { continue };
        let elem = filters::CHIP_BASE + n as u32;
        if let filters::Value::Note(note) = choice.value {
            if note == filters::Note::Working {
                plx_ui::placeholder::note(p, plx_ui::placeholder::Reason::WorkingReadout, "requests-filter-options");
            }
            Label::new(choice.label.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                .h(HAlign::Center).draw(p, rect);
            continue;
        }
        let is_input = choice.value == filters::Value::FindTheme;
        let icon = if choice.selected { Some(Icon::Check) } else if is_input { Some(Icon::Search) } else { None };
        control(p, measure, rect, &choice.label, icon, choice.selected, focus == Some(elem));
        if is_input && screen.editing && focus == Some(elem) && screen.blink_us < BLINK_US {
            let search = screen.theme_input.text();
            let chrome = Button::pill_w_measured(c"", theme::size::BODY, true, None, measure);
            let cap = (rect.w - chrome).max(0.0);
            let advance = measure.width_str(search, theme::size::BODY, true).min(cap);
            let lead = measure.width_str(search.get(..screen.theme_input.caret()).unwrap_or(search), theme::size::BODY, true).min(advance);
            caret(p, rect, (chrome - BTN_PILL_AIR - advance) * 0.5 + lead);
        }
        stop(screen, f, p, elem);
    }
}

fn actions<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    let section = Rect::new(0.0, screen.actions_top() - screen.stack.scroll(), SCR_W, layout::ACTION_H);
    if !plx_ui::on_axis(section.y, section.h, SCR_H, 0.0) { return; }
    let mut end = MARGIN_X;
    for (n, pill) in screen.matches.actions.iter().enumerate() {
        let rect = screen.action_rect(f.cx.measure, section, n);
        control(p, f.cx.measure, rect, &pill.label, pill.icon, false, f.cx.focus.current == Some(screen.key(pill.elem)));
        stop(screen, f, p, pill.elem);
        end = rect.x + rect.w + theme::space::LG;
    }
    if !screen.matches.info.as_bytes().is_empty() {
        let text = &screen.matches.info;
        let room = (SCR_W - MARGIN_X - end).max(0.0);
        let fit;
        let text = if f.cx.measure.width(text, theme::size::BODY, false) <= room {
            text.as_c_str()
        } else {
            fit = f.cx.measure.fit_line(text.to_str().unwrap_or_default(), room, theme::size::BODY, false);
            &fit
        };
        Label::new(text.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY).h(HAlign::Right)
            .draw(p, Rect::new(end, section.y + layout::BAR_PILL_Y, room, filters::HEIGHT));
    }
}

/// An element's hit target, recorded where it is drawn.
fn stop<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter, elem: u32) {
    if !f.records_stops() {
        return;
    }
    use plx_ui::screen::{Activate, Hover, Stop};
    let Some(placed) = <RequestsScreen as plx_ui::screen::Focusable<H>>::place(screen, &elem, f.cx, plx_ui::screen::At::Drawn)
    else {
        return;
    };
    f.stop(
        p,
        Stop {
            key: screen.key(elem),
            rect: placed.rect,
            rest_rect: placed.rest_rect,
            clip: placed.clip,
            hover: Hover::Focus,
            activate: Activate::Immediate,
        },
    );
}
