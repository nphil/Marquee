//! The Requests page's document geometry; the engine's placement and the render read the same
//! expressions. The field and the head match Search's, so the two pages' heads sit alike.
use plx_ui::cards::RowStyle;
use plx_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_H, SCR_W};
use plx_ui::Rect;

pub(super) const FIELD: Rect = Rect { x: MARGIN_X, y: 138.0, w: SCR_W - 2.0 * MARGIN_X, h: 80.0 };
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
