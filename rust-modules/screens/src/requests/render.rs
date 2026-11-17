//! The Requests page's paint: the search field, the poster rows and the status read-out.
use super::*;
use plx_ui::consts::MARGIN_X;
use plx_ui::label::Label;
use plx_ui::widgets::{StatusKind, StatusOverlay};
use plx_ui::{theme, Env, Painter, View};
use std::ffi::CString;
use std::rc::Rc;

const CARET_W: f32 = 5.0;
const CARET_GAP: f32 = 8.0;

/// The field's words as drawn, measured once per edit.
#[derive(Default)]
pub(super) struct Resources {
    query: String,
    caret: usize,
    run: Option<Rc<std::ffi::CStr>>,
    /// The width of the words up to the caret.
    head_w: f32,
}

impl Resources {
    pub(super) fn prepare<H: ContentLike>(&mut self, query: &str, caret: usize, cx: &Cx<'_, H>) {
        if self.run.is_some() && self.query == query && self.caret == caret {
            return;
        }
        self.query.clear();
        self.query.push_str(query);
        self.caret = caret;
        let width = layout::FIELD.w - CARET_W - CARET_GAP;
        let blank = query.trim().is_empty();
        let words = if blank { plx_platform::i18n::msg::requests_placeholder() } else { query };
        self.run = Some(cx.measure.fit_line(words, width, theme::size::HERO, true));
        let head = query.get(..caret).unwrap_or(query);
        self.head_w = if blank { 0.0 } else { cx.measure.width_str(head, theme::size::HERO, true).min(width) };
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
    let blank = screen.query.text().trim().is_empty();
    let hot = screen.editing || f.cx.focus.current == Some(screen.key(FIELD));
    let ink = match (blank, hot) {
        (true, false) => theme::TEXT_TERTIARY,
        (true, true) => theme::TEXT_SECONDARY,
        (false, false) => theme::TEXT_SECONDARY,
        (false, true) => theme::TEXT_PRIMARY,
    };
    if let Some(run) = &screen.render.run {
        let (cap_top, cap_base) = plx_gfx::text::text_cap_band(theme::size::HERO, 1);
        let pad = plx_gfx::text::text_height(theme::size::HERO, 1) - rect.h;
        let _clip = f.clip(p, Rect::new(rect.x, rect.y, rect.w, rect.h + pad.max(0.0)));
        Label::new(run.as_ptr(), theme::size::HERO, ink).bold().draw(p, rect);
        if screen.editing && screen.blink_us < BLINK_US {
            let text_y = plx_gfx::text::text_vcenter_y(theme::size::HERO, 1, rect.cy());
            p.rect(
                Rect::new(rect.x + screen.render.head_w, text_y + cap_top, CARET_W, cap_base - cap_top),
                0.0,
                theme::TEXT_PRIMARY,
                theme::TEXT_PRIMARY,
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
