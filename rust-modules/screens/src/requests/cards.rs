//! One row's posters, for `plx_ui::cards::Shelf`.
use super::*;
use plx_machine::machine::Measure;
use plx_ui::cards::{CardSource, Tile, TileLabel};
use plx_ui::tile::{TileFacts, TileKind};
use plx_ui::widgets::Art;
use plx_ui::Painter;

/// The words a request's state is badged with on its poster.
pub(super) fn badge_word(badge: Badge) -> &'static str {
    use plx_platform::i18n::msg;
    match badge {
        Badge::Requested => msg::requests_badge_requested(),
        Badge::Approved => msg::requests_badge_approved(),
        Badge::Downloading => msg::requests_badge_downloading(),
        Badge::PartlyAvailable => msg::requests_badge_partly(),
        Badge::Available => msg::requests_badge_available(),
        Badge::Blocked => msg::requests_badge_blocked(),
        Badge::Declined => msg::requests_badge_declined(),
        Badge::Failed => msg::requests_badge_failed(),
    }
}

/// A chip's side padding (2 x 12) in `widgets::badge_w`, which the label's room leaves out.
const BADGE_CHROME: f32 = 24.0;

pub struct RowCards<'a> {
    pub(super) row: &'a Row,
    /// The server whose photo proxy fetches the posters; none while no server is reachable.
    pub(super) art: Option<plx_plex::plex::ServerId>,
    pub(super) progress: &'a progress::Poll,
}

impl RowCards<'_> {
    fn chip_word(&self, card: &Card) -> Option<&str> {
        let badge = card.badge?;
        if self.row.lane == Lane::Mine && progress::follows(badge) {
            if let Some(chip) = self.progress.chip(card.kind, card.tmdb) {
                return Some(chip.word.as_ref());
            }
        }
        Some(badge_word(badge))
    }
}

impl<H: ContentLike> CardSource<H> for RowCards<'_> {
    fn len(&self) -> usize {
        self.row.cards.len()
    }
    fn elem(&self, i: usize) -> u32 {
        self.row.cards[i].elem
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        self.row.cards.iter().position(|c| c.elem == *e)
    }
    fn art(&self, i: usize) -> Art<'_> {
        let (Some(card), Some(server)) = (self.row.cards.get(i), self.art) else { return Art::Poster(None) };
        if card.thumb.is_empty() {
            return Art::Poster(None);
        }
        Art::Poster(Some(TileFacts {
            owner: card.elem as usize,
            src: server.raw(),
            kind: match card.kind {
                Kind::Movie => TileKind::Movie,
                Kind::Tv => TileKind::Show,
            },
            title: &card.name,
            thumb: &card.thumb,
            year: card.year.parse().unwrap_or(0),
            ..Default::default()
        }))
    }
    fn label(&self, i: usize) -> TileLabel {
        match self.row.cards.get(i) {
            Some(card) => TileLabel::titled(&card.name, &card.year),
            None => TileLabel::default(),
        }
    }
    fn holdable(&self, _i: usize) -> bool {
        false
    }
    fn more(&self) -> bool {
        self.row.more
    }
    fn page_epoch(&self) -> u32 {
        self.row.epoch
    }
    fn overlay(&self, p: Painter, i: usize, tile: &Tile, measure: &dyn Measure) {
        if let Some(word) = self.row.cards.get(i).and_then(|c| self.chip_word(c)) {
            let inset = plx_ui::theme::space::SM;
            let room = (tile.rect.w - 2.0 * inset - BADGE_CHROME).max(0.0);
            let fit = measure.fit_line(word, room, plx_ui::theme::size::CAPTION, true);
            let word = fit.to_str().unwrap_or_default();
            let cy = tile.rect.y + inset + plx_ui::widgets::BADGE_H * 0.5;
            plx_ui::widgets::badge(p, tile.rect.x + inset, cy, word, None, plx_ui::widgets::BadgeStyle::OverArt, measure);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_ui::fixture::FixtureMeasure;

    #[test]
    fn the_mine_chip_uses_live_bytes_but_never_overrides_a_ready_title() {
        use plx_plex::plex::progress::{Live, Stage, Sum};
        let mut progress = progress::Poll::default();
        progress.show();
        let (kind, gen) = progress.advance(|k| k == Kind::Movie).unwrap();
        progress.land(gen, kind, Ok(vec![Live {
            tmdb: 7,
            sum: Sum { stage: Stage::Downloading, percent: Some(42), eta_s: None },
        }]));
        let row = Row::new(Lane::Mine);
        let cards = RowCards { row: &row, art: None, progress: &progress };
        let mut card = Card {
            elem: 10, kind, tmdb: 7, name: String::new(), year: String::new(), thumb: String::new(),
            badge: Some(Badge::Approved),
        };
        assert!(cards.chip_word(&card).unwrap().contains("42"));
        card.tmdb = 8;
        assert_eq!(cards.chip_word(&card), Some(badge_word(Badge::Approved)));
        card.tmdb = 7;
        card.badge = Some(Badge::Available);
        assert_eq!(cards.chip_word(&card), Some(badge_word(Badge::Available)));
    }

    #[test]
    fn every_status_chip_fits_a_poster_corner() {
        for badge in [Badge::Requested, Badge::Approved, Badge::Downloading, Badge::PartlyAvailable,
            Badge::Available, Badge::Blocked, Badge::Declined, Badge::Failed] {
            let w = plx_ui::widgets::badge_w(badge_word(badge), None, &FixtureMeasure);
            assert!(w <= plx_ui::consts::CARD_W - 2.0 * plx_ui::theme::space::SM, "{badge:?} is {w} wide");
        }
    }
}
