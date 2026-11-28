//! The Requests page's document geometry; the engine's placement and the render read the same
//! expressions.
use plx_ui::cards::RowStyle;
use plx_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_H, SCR_W};
use plx_ui::widgets::StatusOverlay;
use plx_ui::Rect;

/// The search pill in the page's header: a button-height capsule at the left margin.
pub(super) const FIELD: Rect = Rect { x: MARGIN_X, y: 160.0, w: 560.0, h: StatusOverlay::CTRL_H };
/// The head: the search pill's band. The filter bar sits under it.
pub(super) const HEAD_H: f32 = 228.0;
/// The filter bar's section; its controls are `StatusOverlay::CTRL_H` tall, `BAR_PILL_Y` below its top.
pub(super) const BAR_H: f32 = 84.0;
pub(super) const BAR_PILL_Y: f32 = 8.0;
/// Where the first row's heading sits: under the head and the bar.
pub(super) const CONTENT_TOP: f32 = HEAD_H + BAR_H;
/// The read-out under the bar while the filtered rows have nothing to show.
pub(super) const NOTE_H: f32 = 220.0;
pub(super) const HEAD_TO_ROW: f32 = 60.0;
/// The raised keyboard's height, which the status text stays above.
const KEYBOARD_H: f32 = 324.0;
pub(super) static POSTER: RowStyle = RowStyle { w: CARD_W, h: CARD_H, ..RowStyle::HOME };

/// A row block's height under a caption band of `band` px.
pub(super) fn block_h(band: f32) -> f32 {
    HEAD_TO_ROW + POSTER.h + plx_ui::consts::UNDER_LABEL_AIR + band
}

/// Where the page's status reads (loading, nothing found), clear of the keyboard when it is up.
pub(super) fn status_band(editing: bool) -> Rect {
    let bottom = if editing { SCR_H - KEYBOARD_H } else { SCR_H };
    Rect::new(0.0, CONTENT_TOP, SCR_W, bottom - CONTENT_TOP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_pill_stays_inside_the_margins() {
        assert!(FIELD.x >= MARGIN_X);
        assert!(FIELD.x + FIELD.w <= SCR_W - MARGIN_X);
    }
}
