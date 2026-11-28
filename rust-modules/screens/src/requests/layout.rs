//! The Requests page's document geometry; the engine's placement and the render read the same
//! expressions.
use super::RequestsScreen;
use plx_ui::cards::RowStyle;
use plx_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_H, SCR_W};
use plx_ui::widgets::StatusOverlay;
use plx_ui::Rect;

/// The search pill in the page's header: a button-height capsule at the left margin.
pub(super) const FIELD: Rect = Rect { x: MARGIN_X, y: 160.0, w: 560.0, h: super::filters::HEIGHT };
/// The head: the search pill's band. The filter bar sits under it.
pub(super) const HEAD_H: f32 = 252.0;
/// Filter and result-control bands have the same rhythm and button height.
pub(super) const BAR_H: f32 = super::filters::HEIGHT + plx_ui::theme::space::MD + plx_ui::theme::space::SM;
pub(super) const ACTION_H: f32 = BAR_H;
pub(super) const BAR_PILL_Y: f32 = 8.0;
/// Where the first row's heading sits: under the head and the bar.
pub(super) const CONTENT_TOP: f32 = HEAD_H + BAR_H;
pub(super) const SEARCH_CONTENT_TOP: f32 = HEAD_H + StatusOverlay::CTRL_H + plx_ui::theme::space::MD;
pub(super) const GRID_EDGE: f32 = plx_ui::widgets::TOP_BAR_BOTTOM + plx_ui::theme::space::MD;
/// The read-out under the bar while the filtered rows have nothing to show.
pub(super) const NOTE_H: f32 = 220.0;
pub(super) const HEAD_TO_ROW: f32 = 60.0;
/// The raised keyboard's height, which the status text stays above.
const KEYBOARD_H: f32 = 324.0;
pub(super) static POSTER: RowStyle = RowStyle { w: CARD_W, h: CARD_H, ..RowStyle::HOME };

impl RequestsScreen {
    pub(super) fn actions_top(&self) -> f32 {
        CONTENT_TOP + if self.matches.open { 0.0 } else { self.chooser_height() }
    }

    pub(super) fn chooser_top(&self) -> f32 {
        CONTENT_TOP + if self.matches.open { ACTION_H } else { 0.0 }
    }

    pub(super) fn movie_content_top(&self) -> f32 {
        CONTENT_TOP + ACTION_H + self.chooser_height()
    }

    fn chooser_height(&self) -> f32 {
        self.chooser.as_ref().map_or(0.0, super::filters::Chooser::height)
    }
}

/// A row block's height under a caption band of `band` px.
pub(super) fn block_h(band: f32) -> f32 {
    HEAD_TO_ROW + POSTER.h + plx_ui::consts::UNDER_LABEL_AIR + band
}

/// Where the page's status reads (loading, nothing found), clear of the keyboard when it is up.
pub(super) fn status_band(editing: bool, bar_shown: bool) -> Rect {
    let bottom = if editing { SCR_H - KEYBOARD_H } else { SCR_H };
    let top = if bar_shown { CONTENT_TOP } else { SEARCH_CONTENT_TOP };
    Rect::new(0.0, top, SCR_W, bottom - top)
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
