//! **Watch and Skip** replace the transport's Subtitles/Audio discs while the playhead sits inside
//! one of the playing leaf's server markers (`metadata::playing_markers`, fetched with
//! `?includeMarkers=1`): same row, same right edge, same height, and the discs return the moment the
//! segment ends. Skip keeps the row's floor width; Watch takes only its own label's.
//!
//! The cost is that CC/Audio are covered for the length of the segment, so BACK over the pair
//! DISMISSES the offer for that segment and puts the discs back (`PlayerScreen::dismiss_offer` asks
//! `PlayerReq::DismissOffer`, which marks the segment skipped in the metadata store — the same set a
//! Skip press uses). The pair only exists while the HUD is up, which is why `app.rs` RAISES the HUD
//! once when a segment begins and parks focus on Skip, so a bare OK still skips in one press.
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

/// **Draw the pair** at the rects the caller already has: the compiled package's `[watch, skip]`
/// (`Standin::skip`, one pair per marker kind), placed at compile time in the control row's place.
/// `focused` is the row's focused item while the row holds focus; `progress` is the automatic
/// skip's elapsed fraction, swept across Skip.
pub fn draw_at(row: &crate::player_hud::motion::HudMotion, p: Painter, pr: Prompt, [watch, skip]: [Rect; 2], focused: Option<i32>, progress: Option<f32>) {
    let (watch_label, skip_label) = (pr.watch_label_c(), pr.label_c());
    let e = Env::inert();
    // Both stand in the transport discs' own slot, over the video plane and on the HUD's ramp, so
    // they wear the discs' ground and the control ROW's pop (`HudMotion::scale`) — see `ControlGround`.
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
