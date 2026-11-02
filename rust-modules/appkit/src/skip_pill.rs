//! Watch and Skip controls replace the transport discs during a marked segment, keeping the
//! control row's height and right edge.
#![allow(dead_code)]
use plx_data::metadata::{self, MarkerKind};
use plx_ui::theme;
use plx_ui::widgets::{Button, ControlGround};
use plx_ui::{Env, Painter, Rect, View};

pub const BTN_WATCH: i32 = 0;
pub const BTN_SKIP: i32 = 1;
pub const PRIMARY_BTN: i32 = BTN_SKIP;

/// What pressing the button does. The distinction is the `final` flag on a credits marker: an
/// ordinary segment is a seek and playback continues past it, but a `final` one runs to the end of
/// the item, so "skip" means **the episode is over** — seeking to its end would race the decoder
/// against its own last frames for no benefit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkipAction {
    /// jump to this position (ns) and keep playing
    Seek(i64),
    /// the item is finished — hand off to the end-of-playback path (next episode, or leave)
    Finish,
}

/// The offer the button is currently making. Carries the marker's TYPED kind, not its label:
/// asking "is the playhead in a credits segment?" by string-comparing user-facing copy made a pure
/// copy-edit — the sort of thing a design pass does — silently disable Up Next and auto-advance,
/// with no compile error and nothing to fail.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    /// the segment itself — carried so activating the button can retire it (`metadata::mark_skipped`)
    pub marker: metadata::Marker,
    pub kind: MarkerKind,
    pub action: SkipAction,
}

impl Prompt {
    /// The button's copy — presentation, derived from the kind at the point of drawing.
    pub fn label(&self) -> &'static str {
        match self.kind {
            MarkerKind::Intro => plx_platform::i18n::msg::widgets_skip_intro(),
            MarkerKind::Credits => plx_platform::i18n::msg::widgets_skip_credits(),
        }
    }
}

impl Prompt {
    fn watch_label_c(&self) -> &'static std::ffi::CStr {
        match self.kind {
            MarkerKind::Intro => plx_platform::i18n::msg::widgets_skip_watch_intro_c(),
            MarkerKind::Credits => plx_platform::i18n::msg::widgets_skip_watch_credits_c(),
        }
    }
    fn label_c(&self) -> &'static std::ffi::CStr {
        match self.kind {
            MarkerKind::Intro => plx_platform::i18n::msg::widgets_skip_intro_c(),
            MarkerKind::Credits => plx_platform::i18n::msg::widgets_skip_credits_c(),
        }
    }
}

/// PURE: the offer a given segment makes. Takes the marker rather than reading the playhead, so
/// the precedence it feeds ([`crate::player_hud::slot_for`]) is host-testable and the whole frame
/// decides from ONE playhead sample — `playpos_ns` is written by LG's media thread, and re-reading
/// it per call site let the input path and the draw path disagree within a single frame.
///
/// Markers belong to the PLAYING leaf (`metadata::playing()`), never to `metadata::current()`:
/// during a show-page episode play `current()` is the SHOW, and offering episode 1's intro timing
/// during episode 5 is the same identity bug the track store exists to prevent.
pub fn prompt_for(m: metadata::Marker) -> Prompt {
    Prompt {
        marker: m,
        kind: m.kind,
        action: match m.kind {
            // a `final` credits segment runs to the end of the item, so "skip" means the episode is
            // over — seeking to its end would race the decoder against its own last frames
            MarkerKind::Credits if m.final_seg => SkipAction::Finish,
            _ => SkipAction::Seek(m.end_ms * 1_000_000),
        },
    }
}

// The primary retains the stand-in slot's floor; Watch only needs its intrinsic width,
// matching Up Next's unequal pair and keeping Skip at the original right edge.
fn pair(primary: Rect, watch_w: f32) -> (Rect, Rect) {
    let watch = Rect::new(primary.x - theme::space::SM - watch_w, primary.y, watch_w, primary.h);
    (watch, primary)
}

fn watch_width(pr: Prompt, measure: &dyn plx_machine::machine::Measure) -> f32 {
    Button::pill_w_measured(pr.watch_label_c(), theme::size::BODY, false, None, measure)
}

fn bounds(watch: Rect, skip: Rect) -> Rect {
    Rect::new(watch.x, skip.y, skip.x + skip.w - watch.x, skip.h)
}

fn choose(watch: Rect, skip: Rect, idx: i32) -> Rect {
    if idx == BTN_WATCH { watch } else { skip }
}

pub fn rect(row: &mut crate::player_hud::motion::HudMotion, pr: Prompt, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let primary = crate::player_hud::classic::ctrl_slot(row, pr.label(), measure);
    let (watch, skip) = pair(primary, watch_width(pr, measure));
    bounds(watch, skip)
}

pub fn item_rect(row: &mut crate::player_hud::motion::HudMotion, pr: Prompt, idx: i32, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let primary = crate::player_hud::classic::ctrl_slot(row, pr.label(), measure);
    let (watch, skip) = pair(primary, watch_width(pr, measure));
    choose(watch, skip, idx)
}

fn pair_peek(row: &crate::player_hud::motion::HudMotion, pr: Prompt, measure: &dyn plx_machine::machine::Measure) -> (Rect, Rect) {
    let anchor = crate::player_hud::classic::frame_geometry().row_anchor;
    let width = crate::player_hud::classic::ctrl_slot_w(row, pr.label(), measure);
    pair(Rect::new(anchor.right - width, anchor.top, width, anchor.h), watch_width(pr, measure))
}

pub fn rect_peek(row: &crate::player_hud::motion::HudMotion, pr: Prompt, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let (watch, skip) = pair_peek(row, pr, measure);
    bounds(watch, skip)
}

pub fn item_rect_peek(row: &crate::player_hud::motion::HudMotion, pr: Prompt, idx: i32, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let (watch, skip) = pair_peek(row, pr, measure);
    choose(watch, skip, idx)
}

pub fn draw(row: &mut crate::player_hud::motion::HudMotion, p: Painter, pr: Prompt, focused: Option<i32>, progress: Option<f32>, measure: &dyn plx_machine::machine::Measure) {
    let (watch_label, skip_label) = (pr.watch_label_c(), pr.label_c());
    let primary = crate::player_hud::classic::ctrl_slot(row, pr.label(), measure);
    let watch_w = Button::pill_w_measured(watch_label, theme::size::BODY, false, None, measure);
    let (watch, skip) = pair(primary, watch_w);
    let e = Env::inert();
    Button::new(watch_label.as_ptr(), theme::size::BODY, watch)
        .scale(row.scale(BTN_WATCH))
        .focused(focused == Some(BTN_WATCH))
        .ground(ControlGround::Unkeyed)
        .draw(&e, p);
    let mut button = Button::new(skip_label.as_ptr(), theme::size::BODY, skip)
        .scale(row.scale(BTN_SKIP))
        .focused(focused == Some(BTN_SKIP))
        .ground(ControlGround::Unkeyed);
    if let Some(progress) = progress {
        if !plx_machine::motion::phase_clocks_held() { plx_machine::idle::invalidate(); }
        button = button.progress(progress);
    }
    button.draw(&e, p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_preserves_primary_and_right_edge() {
        let primary = Rect::new(800.0, 900.0, 240.0, 60.0);
        let (watch, skip) = pair(primary, 160.0);
        assert_eq!(skip, primary);
        assert_eq!(watch.x + watch.w + theme::space::SM, skip.x);
        assert_eq!((watch.y, watch.h), (skip.y, skip.h));
        let union = bounds(watch, skip);
        assert_eq!(union.x, watch.x);
        assert_eq!(union.x + union.w, primary.x + primary.w);
        assert_eq!(choose(watch, skip, BTN_WATCH), watch);
        assert_eq!(choose(watch, skip, BTN_SKIP), skip);
        assert_eq!(PRIMARY_BTN, BTN_SKIP);
    }
}
