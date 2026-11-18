//! The Requests page's document geometry; the engine's placement and the render read the same
//! expressions.
use plx_ui::cards::RowStyle;
use plx_ui::consts::{CARD_H, CARD_W, SCR_H, SCR_W};
use plx_ui::widgets::StatusOverlay;
use plx_ui::Rect;

/// The column LumaTV's mute mark drifts about under the profile picture: x 141..253, y 180..292
/// (measured on the TV, 64 px mark, 48 px drift). The search pill starts past it so the mark never
/// lands on the pill; the headings below start under it.
const MARK_COLUMN_RIGHT: f32 = 253.0;
/// The search pill in the page's header: a button-height capsule beside the mute mark's column.
pub(super) const FIELD: Rect = Rect { x: MARK_COLUMN_RIGHT + 35.0, y: 160.0, w: 560.0, h: StatusOverlay::CTRL_H };
/// The head's height: the first row's heading sits just under it.
pub(super) const CONTENT_TOP: f32 = 300.0;
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
    use plx_ui::consts::MARGIN_X;

    #[test]
    fn the_search_pill_clears_the_mute_marks_column_and_headings_start_under_it() {
        assert!(FIELD.x >= MARK_COLUMN_RIGHT + 32.0);
        assert!(CONTENT_TOP >= 292.0);
        assert!(FIELD.x + FIELD.w <= SCR_W - MARGIN_X);
    }
}
