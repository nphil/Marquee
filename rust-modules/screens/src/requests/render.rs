//! The Requests page's paint: the search pill, the poster rows and the status read-out.
use super::*;
use plx_ui::consts::MARGIN_X;
use plx_ui::icons::Icon;
use plx_ui::widgets::{Button, StatusKind, StatusOverlay, BTN_PILL_AIR};
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
    if screen.shown().next().is_none() {
        status(screen, p);
    }
    for row in screen.shown() {
        let Some((frame, shelf)) = screen.row_frame(f.cx, row.lane) else { continue };
        let top = frame.y - layout::HEAD_TO_ROW;
        if !plx_ui::on_axis(top, layout::block_h(shelf.under_band()), SCR_H, 0.0) {
            continue;
        }
        plx_ui::cards::draw_heading_c(p, row.lane.heading(), c"", c"", MARGIN_X, top - shelf.heading_lift(), 0.0, f.cx.measure);
        if let Some(src) = screen.cards_of(Sec::Row(row.lane, screen.rows_gen)) {
            shelf.draw(f, p, &src, frame);
        }
    }
}

fn field<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    let rect = Rect::new(layout::FIELD.x, layout::FIELD.y - screen.stack.scroll(), layout::FIELD.w, layout::FIELD.h);
    let hot = screen.editing || f.cx.focus.current == Some(screen.key(FIELD));
    if let Some(run) = &screen.render.run {
        Button::new(run.as_ptr(), theme::size::BODY, rect).icon(Icon::Search).focused(hot).draw(&Env::inert(), p);
        if screen.editing && screen.blink_us < BLINK_US {
            let (cap_top, cap_base) = plx_gfx::text::text_cap_band(theme::size::BODY, 1);
            let text_y = plx_gfx::text::text_vcenter_y(theme::size::BODY, 1, rect.cy());
            // The pill is focused while it is edited, so it wears the focused face's dark ink.
            p.rect(
                Rect::new(rect.cx() + screen.render.caret_dx, text_y + cap_top, CARET_W, cap_base - cap_top),
                0.0,
                theme::ACCENT_INK,
                theme::ACCENT_INK,
                0.0,
            );
        }
    }
    stop(screen, f, p);
}

/// What the page says where its rows would be: it is reaching Seerr, it could not, or nothing is there.
fn status(screen: &RequestsScreen, p: Painter) {
    use plx_platform::i18n::msg;
    let mut rect = layout::status_band(screen.editing);
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

/// The field's hit target, recorded where the field is drawn.
fn stop<H: ContentLike>(screen: &RequestsScreen, f: &mut DrawFrame<'_, '_, H>, p: Painter) {
    if !f.records_stops() {
        return;
    }
    use plx_ui::screen::{Activate, Hover, Stop};
    let Some(placed) = <RequestsScreen as plx_ui::screen::Focusable<H>>::place(screen, &FIELD, f.cx, plx_ui::screen::At::Drawn)
    else {
        return;
    };
    f.stop(
        p,
        Stop {
            key: screen.key(FIELD),
            rect: placed.rect,
            rest_rect: placed.rest_rect,
            clip: placed.clip,
            hover: Hover::Focus,
            activate: Activate::Immediate,
        },
    );
}
