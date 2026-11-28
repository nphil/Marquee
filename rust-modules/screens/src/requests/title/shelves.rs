use super::{facts, Sec, TitleScreen, CAST, STILL, VIDEO};
use crate::registry::{captions, ContentLike, TitlePage};
use plx_machine::machine::Measure;
use plx_plex::plex::seerr::{Season, Still};
use plx_ui::cards::{CardSource, RowStyle, Tile, TileLabel};
use plx_ui::icons::Icon;
use plx_ui::theme::{size, space};
use plx_ui::widgets::{self, Art, BadgeStyle};
use plx_ui::Painter;

pub(super) struct Card {
    pub elem: u32,
    key: String,
    label: TileLabel,
    season: Option<u32>,
    badge: Option<&'static str>,
}

pub(super) struct SeasonProgress {
    word: String,
    label: TileLabel,
}

impl SeasonProgress {
    pub(super) fn new(row: &plx_plex::plex::progress::SeasonRow) -> Self {
        let word = super::progress_text::season_word(row);
        let detail = super::progress_text::season_detail(row);
        let label = TileLabel::titled(&facts::season_name(row.number), detail.as_deref().unwrap_or(&word));
        Self { word, label }
    }
}

pub(super) fn season(s: &Season, page: &TitlePage, poster: &str, ordinal: usize) -> Card {
    let name = facts::season_name(s.number);
    let badge = facts::season_word(s);
    Card {
        elem: super::SEASON + ordinal as u32,
        key: page.detail.info.season_poster(s.number).unwrap_or(poster).to_owned(),
        label: TileLabel::titled(&name, badge.unwrap_or_default()),
        season: Some(s.number), badge,
    }
}

pub(super) fn cast(page: &TitlePage) -> Vec<Card> {
    page.detail.info.people.iter().enumerate().map(|(i, person)| Card {
        elem: CAST + i as u32,
        key: person.photo.clone(),
        label: TileLabel::titled(&person.name, facts::role_word(&person.role)),
        season: None, badge: None,
    }).collect()
}

pub(super) fn videos(page: &TitlePage) -> Vec<Card> {
    page.detail.info.videos.iter().enumerate().map(|(i, video)| Card {
        elem: VIDEO + i as u32,
        key: video.thumb(),
        label: captions::still_block(&video.name, facts::video_word(video.kind)),
        season: None, badge: None,
    }).collect()
}

pub(super) fn stills(stills: Vec<Still>) -> Vec<Card> {
    stills.into_iter().take(plx_plex::plex::seerr::SHELF_CAP).enumerate().map(|(i, still)| Card {
        elem: STILL + i as u32,
        key: still.image,
        label: captions::still_block(&still.name, &facts::season_name(still.season)),
        season: None, badge: None,
    }).collect()
}

pub struct Cards<'a> {
    pub(super) screen: &'a TitleScreen,
    pub(super) sid: Option<u16>,
    pub(super) cards: &'a [Card],
    pub(super) which: Sec,
}

impl Cards<'_> {
    fn progress(&self, i: usize) -> Option<&SeasonProgress> {
        let season = self.cards[i].season?;
        self.screen.season_progress.iter().find(|(n, _)| *n == season).map(|(_, p)| p)
    }

    fn caption(&self, i: usize) -> &TileLabel {
        self.progress(i).map_or(&self.cards[i].label, |p| &p.label)
    }
}

impl<H: ContentLike> CardSource<H> for Cards<'_> {
    fn len(&self) -> usize { self.cards.len() }
    fn elem(&self, i: usize) -> u32 { self.cards[i].elem }
    fn index_of(&self, elem: &u32) -> Option<usize> { self.cards.iter().position(|c| c.elem == *elem) }
    fn art(&self, i: usize) -> Art<'_> {
        let Some(sid) = self.sid else { return Art::Poster(None) };
        let key = &self.cards[i].key;
        match self.which {
            Sec::Cast => Art::Person { sid, key, res: (300, 300) },
            Sec::Seasons => Art::Thumb { sid, key, res: widgets::POSTER_RES },
            _ => {
                let style = RowStyle::EPISODE;
                Art::Thumb { sid, key, res: (style.w as i32, style.h as i32) }
            }
        }
    }
    fn label(&self, i: usize) -> TileLabel { self.caption(i).clone() }
    fn with_label<R>(&self, i: usize, draw: impl FnOnce(&TileLabel) -> R) -> R { draw(self.caption(i)) }
    fn holdable(&self, _: usize) -> bool { false }
    fn overlay(&self, p: Painter, i: usize, tile: &Tile, measure: &dyn Measure) {
        let card = &self.cards[i];
        let Some(season) = card.season else { return };
        if self.screen.picked.contains(&season) && self.screen.picker() {
            widgets::corner_mark(p, tile.rect, tile.radius, Icon::Check);
            return;
        }
        let live = self.progress(i).map(|p| p.word.as_str());
        if let Some(word) = live.or(card.badge) {
            let inset = space::SM;
            let chrome = widgets::badge_w("", None, measure);
            let fit = measure.fit_line(word, (tile.rect.w - 2.0 * inset - chrome).max(0.0), size::CAPTION, true);
            let cy = tile.rect.y + inset + widgets::BADGE_H * 0.5;
            widgets::badge(p, tile.rect.x + inset, cy, fit.to_str().unwrap_or_default(), None, BadgeStyle::OverArt, measure);
        }
    }
}
