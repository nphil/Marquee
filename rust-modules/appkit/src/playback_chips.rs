//! **The playback chips**: what is playing, in plain words ("4K", "Dolby Vision", "Dolby Atmos",
//! "English subtitles"), as small capsules in the picture's top-left corner once a playback's first
//! picture is up, and again whenever what they say changes. They fade in one after another, stay
//! [`HOLD_MS`] and fade out together.
//!
//! The top-left corner, because everything else that speaks over a playing picture is at the bottom
//! (captions, the transport, the system volume popup) or in the middle (the read-out). A caption the
//! file places at the top, such as a sign, wins: the chips are not drawn on a frame whose painted
//! captions they would cover.
//!
//! The words come from the facts the track menu reads (the route's carried tracks and conversions) and
//! the demuxer's raster, in the track menu's own plain words. The facts are read and compared every
//! frame; the words are built only when the facts change.

use plx_data::metadata::{self, PlayingItem, Stream};
use plx_machine::machine::Measure;
use plx_media::route::PlaybackSession;
use plx_ui::consts::{MARGIN_X, MARGIN_Y};
use plx_ui::theme;
use plx_ui::widgets::{badge, badge_w, BadgeStyle, BADGE_H};
use plx_ui::{Painter, Rect};
use std::borrow::Cow;

/// How long after one chip the next begins to fade in.
pub const STAGGER_MS: u32 = 90;
/// One chip's fade in.
pub const FADE_IN_MS: u32 = 240;
/// How long the chips stay whole once the last one is in.
pub const HOLD_MS: u32 = 4_000;
/// The chips' fade out, all together.
pub const FADE_OUT_MS: u32 = 400;
/// The widest a chip draws; a longer label (a long language name) ends in an ellipsis. Four chips this
/// wide fit between the screen's safe margins.
pub const CHIP_W_CAP: f32 = 420.0;
const _: () = assert!(4.0 * CHIP_W_CAP + 3.0 * theme::space::XS <= plx_ui::consts::SCR_W - 2.0 * MARGIN_X);

/// What the picture is, as the TV is shown it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Picture {
    #[default]
    Standard,
    Hdr,
    DolbyVision,
}

/// How the playing audio track reaches the TV.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// As the file has it.
    AsIs,
    /// Converted by the server, to a format the server picks: no chip names it.
    Converted,
    /// Decoded by the app and mixed down to stereo.
    Stereo,
}

/// Everything the chips say, as values cheap to read and compare every frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Facts {
    /// The picture's raster as decoded, else the source's when it is fed whole; (0, 0) when neither
    /// is known.
    pub raster: (i32, i32),
    pub picture: Picture,
    /// The playing audio track, an index into the item's list, and how it reaches the TV.
    pub audio: Option<(usize, Delivery)>,
    /// The subtitle track on screen, an index into the item's list: `None` with subtitles off, and
    /// under on-screen text only, which shows a subtitle only for a sign.
    pub subtitles: Option<usize>,
}

impl Facts {
    /// This frame's facts of the playing `item`. `decoded` is the raster the demuxer published
    /// (`plx_media::player::video_raster`), (0, 0) until it knows one.
    pub fn read(ps: &PlaybackSession, item: &PlayingItem, decoded: (i32, i32)) -> Self {
        use plx_media::route;
        let raster = if decoded.0 > 0 && decoded.1 > 0 {
            decoded
        } else {
            route::stream_video(ps).map_or((0, 0), |v| {
                (i32::try_from(v.width).unwrap_or(0), i32::try_from(v.height).unwrap_or(0))
            })
        };
        let picture = if matches!(route::stream_dv_presentation(ps), metadata::DvPresentation::Declare(_)) {
            Picture::DolbyVision
        } else if route::hdr_picture(item.hdr, route::is_transcoding(ps), route::is_remux(ps)) {
            Picture::Hdr
        } else {
            Picture::Standard
        };
        let audio = crate::track_menu::playing_audio(ps, &item.audio).map(|i| {
            let delivery = match crate::track_menu::audio_conversion(ps, &item.audio[i]) {
                (true, _) => Delivery::Converted,
                (false, true) => Delivery::Stereo,
                (false, false) => Delivery::AsIs,
            };
            (i, delivery)
        });
        let sid = route::cur_sub_sid(ps);
        let subtitles = (sid > 0 && route::cur_sub_mode(ps) != route::SubMode::ForcedOnly)
            .then(|| item.subs.iter().position(|s| s.id == sid))
            .flatten();
        Self { raster, picture, audio, subtitles }
    }
}

/// What `facts` say, in order: resolution, picture, audio, subtitles. A chip is left out when there
/// is nothing plain to say: no known raster, a standard picture, audio the server converts.
pub fn words(facts: &Facts, audio: &[Stream], subs: &[Stream]) -> Vec<String> {
    use plx_platform::i18n::msg;
    let (w, h) = facts.raster;
    let resolution = (w > 0 && h > 0).then(|| plx_ui::fmt::resolution("", w.into(), h.into())).flatten();
    let picture = match facts.picture {
        Picture::Standard => None,
        Picture::Hdr => Some(msg::widgets_chips_hdr().to_string()),
        Picture::DolbyVision => Some(msg::widgets_chips_dolby_vision().to_string()),
    };
    let sound = facts.audio.and_then(|(i, delivery)| sound(audio.get(i)?, delivery));
    let subtitles = facts.subtitles.and_then(|i| subs.get(i)).map(|s| match s.lang.trim() {
        "" => msg::widgets_badge_subtitles().to_string(),
        lang => msg::widgets_chips_subtitles(lang),
    });
    [resolution, picture, sound, subtitles].into_iter().flatten().collect()
}

/// The audio chip: Atmos by name, a surround layout as "5.1 surround", stereo and mono as the track
/// menu says them ([`audio_descriptor`](crate::track_menu::audio_descriptor)).
fn sound(s: &Stream, delivery: Delivery) -> Option<String> {
    use plx_platform::i18n::msg;
    match delivery {
        Delivery::Converted => None,
        Delivery::Stereo => Some(msg::widgets_tracks_stereo().to_string()),
        Delivery::AsIs if s.has_atmos() => Some(msg::widgets_chips_dolby_atmos().to_string()),
        Delivery::AsIs => {
            let layout = crate::track_menu::audio_descriptor(s);
            if layout.starts_with(|c: char| c.is_ascii_digit()) {
                Some(msg::widgets_chips_surround(&layout))
            } else {
                Some(layout).filter(|l| !l.is_empty())
            }
        }
    }
}

/// One chip: its words and when it begins to fade in.
struct Chip {
    word: String,
    at: u32,
}

/// **The chips' show**, owned by the player page: stepped on its Tick ([`Self::observe`]), drawn
/// after its captions ([`Self::draw`]) and reported to its frame gate ([`Self::fingerprint`]).
#[derive(Default)]
pub struct PlaybackChips {
    /// The facts the chips were built from; `None` until the playback's first picture.
    facts: Option<Facts>,
    chips: Vec<Chip>,
    /// When the chips begin to fade out.
    until: u32,
}

impl PlaybackChips {
    /// A new playback: its first picture shows the chips again, whatever they say.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Once a frame, with this frame's `facts` and whether the picture is `playing` or the pipeline
    /// is `busy` (resolving, connecting, seeking, buffering). Nothing is read while it is busy, when
    /// the route's facts are in flux, nor before the playback's first picture. After that, a change
    /// in what the chips say shows them again: a chip already whole stays, the new ones fade in one
    /// after another, and all of them stay another [`HOLD_MS`]. Facts that change into the same words
    /// (a reload publishing the same raster again) show nothing. `words` builds what `facts` say and
    /// runs only when they changed.
    pub fn observe(&mut self, facts: Facts, playing: bool, busy: bool, now: u32, words: impl FnOnce(&Facts) -> Vec<String>) {
        if busy || (self.facts.is_none() && !playing) || self.facts == Some(facts) {
            return;
        }
        self.facts = Some(facts);
        let next = words(&facts);
        if next.iter().eq(self.chips.iter().map(|c| &c.word)) {
            return;
        }
        let whole = !self.chips.is_empty() && before(now, self.until);
        let old = std::mem::take(&mut self.chips);
        let mut fresh = 0;
        self.chips = next
            .into_iter()
            .map(|word| {
                let kept = old.iter().find(|c| whole && c.word == word).map(|c| c.at);
                let at = kept.unwrap_or_else(|| {
                    fresh += 1;
                    now.wrapping_add((fresh - 1) * STAGGER_MS)
                });
                Chip { word, at }
            })
            .collect();
        self.until = now.wrapping_add(fresh.saturating_sub(1) * STAGGER_MS + FADE_IN_MS + HOLD_MS);
    }

    /// The frame gate's term (`PlayerScreen::clock_fingerprint`): the clock while a chip fades, and a
    /// still value while they stand whole or are gone, which owe no frame.
    pub fn fingerprint(&self, now: u32) -> u64 {
        if self.gone(now) {
            return 0;
        }
        let rising = self.chips.iter().any(|c| ramp(now, c.at, FADE_IN_MS) < 1.0);
        if rising || !before(now, self.until) { u64::from(now) + 2 } else { 1 }
    }

    /// Draw the chips from the top-left safe corner, unless a caption painted this frame (`captions`)
    /// stands where they would.
    pub fn draw(&self, p: Painter, now: u32, captions: Option<Rect>, measure: &dyn Measure) {
        if self.gone(now) {
            return;
        }
        let gap = theme::space::XS;
        let row_w: f32 = self.chips.iter().map(|c| badge_w(&fitted(&c.word, measure), None, measure) + gap).sum::<f32>() - gap;
        let row = Rect::new(MARGIN_X, MARGIN_Y, row_w, BADGE_H);
        if captions.is_some_and(|c| overlap(c, row)) {
            return;
        }
        let cy = MARGIN_Y + BADGE_H * 0.5;
        let mut x = MARGIN_X;
        for c in &self.chips {
            let label = fitted(&c.word, measure);
            let a = self.alpha(c, now);
            if a > 0.0 {
                badge(p.alpha(a), x, cy, &label, None, BadgeStyle::OverArt, measure);
            }
            x += badge_w(&label, None, measure) + gap;
        }
    }

    fn gone(&self, now: u32) -> bool {
        self.chips.is_empty() || ramp(now, self.until, FADE_OUT_MS) >= 1.0
    }

    fn alpha(&self, c: &Chip, now: u32) -> f32 {
        smooth(ramp(now, c.at, FADE_IN_MS)) * (1.0 - smooth(ramp(now, self.until, FADE_OUT_MS)))
    }
}

/// `word`, cut with an ellipsis to [`CHIP_W_CAP`] when it is wider.
fn fitted<'a>(word: &'a str, measure: &dyn Measure) -> Cow<'a, str> {
    if badge_w(word, None, measure) <= CHIP_W_CAP {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(plx_machine::machine::elide_by(word, CHIP_W_CAP, false, |s| badge_w(s, None, measure)))
    }
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Is `now` before `t` on the wrapping millisecond clock?
fn before(now: u32, t: u32) -> bool {
    (now.wrapping_sub(t) as i32) < 0
}

/// 0 before `from`, 1 from `from + ms` on, linear between.
fn ramp(now: u32, from: u32, ms: u32) -> f32 {
    ((now.wrapping_sub(from) as i32).max(0) as f32 / ms as f32).min(1.0)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u32 = 10_000;

    fn said<'a>(words: &'a [&'a str]) -> impl FnOnce(&Facts) -> Vec<String> + 'a {
        move |_| words.iter().map(|w| w.to_string()).collect()
    }

    fn at_width(w: i32) -> Facts {
        Facts { raster: (w, 1080), ..Facts::default() }
    }

    fn alphas(chips: &PlaybackChips, now: u32) -> Vec<f32> {
        chips.chips.iter().map(|c| chips.alpha(c, now)).collect()
    }

    fn atmos() -> Stream {
        Stream { id: 1, channels: 8, layout: "7.1".into(), profile: "dolby digital plus + dolby atmos".into(), ..Default::default() }
    }

    fn surround() -> Stream {
        Stream { id: 2, channels: 6, layout: "5.1(side)".into(), ..Default::default() }
    }

    #[test]
    fn the_words_say_what_reaches_the_tv() {
        let audio = [atmos(), surround()];
        let subs = [Stream { id: 3, lang: "English".into(), ..Default::default() }, Stream { id: 4, ..Default::default() }];
        let all = Facts { raster: (3840, 2160), picture: Picture::DolbyVision, audio: Some((0, Delivery::AsIs)), subtitles: Some(0) };
        assert_eq!(words(&all, &audio, &subs), ["4K", "Dolby Vision", "Dolby Atmos", "English subtitles"]);
        let converted = Facts { picture: Picture::Hdr, audio: Some((0, Delivery::Converted)), subtitles: Some(1), ..Facts::default() };
        assert_eq!(words(&converted, &audio, &subs), ["HDR", "Subtitles"]);
        let layout = Facts { raster: (1918, 802), audio: Some((1, Delivery::AsIs)), ..Facts::default() };
        assert_eq!(words(&layout, &audio, &subs), ["1080p", "5.1 surround"]);
        let mixed = Facts { audio: Some((1, Delivery::Stereo)), ..Facts::default() };
        assert_eq!(words(&mixed, &audio, &subs), ["Stereo"]);
    }

    #[test]
    fn the_chips_wait_for_the_first_picture_then_fade_in_in_turn_and_out_together() {
        let mut chips = PlaybackChips::default();
        chips.observe(at_width(1920), false, false, T0, said(&["1080p", "Stereo"]));
        assert_eq!(chips.fingerprint(T0), 0, "nothing before the first picture");
        chips.observe(at_width(1920), true, false, T0, said(&["1080p", "Stereo"]));
        assert_eq!(alphas(&chips, T0), [0.0, 0.0]);
        assert_eq!(alphas(&chips, T0 + STAGGER_MS)[1], 0.0, "the second waits its turn");
        assert_eq!(alphas(&chips, T0 + STAGGER_MS + FADE_IN_MS), [1.0, 1.0]);
        let out = T0 + STAGGER_MS + FADE_IN_MS + HOLD_MS;
        assert_eq!(alphas(&chips, out), [1.0, 1.0]);
        assert_eq!(alphas(&chips, out + FADE_OUT_MS), [0.0, 0.0]);
        assert_ne!(chips.fingerprint(T0 + 10), chips.fingerprint(T0 + 20), "a fade owes frames");
        assert_eq!(chips.fingerprint(out - 100), chips.fingerprint(out - 50), "whole chips owe none");
        assert_ne!(chips.fingerprint(out + 10), chips.fingerprint(out + 20));
        assert_eq!(chips.fingerprint(out + FADE_OUT_MS), 0);
    }

    #[test]
    fn only_new_words_show_again_and_only_the_changed_chip_fades_in() {
        let mut chips = PlaybackChips::default();
        chips.observe(at_width(1920), true, false, T0, said(&["1080p", "English subtitles"]));
        let first_out = chips.until;
        let later = T0 + 1_000;
        chips.observe(at_width(1918), true, false, later, said(&["1080p", "English subtitles"]));
        assert_eq!(chips.until, first_out, "the same words do not start again");
        chips.observe(Facts { subtitles: Some(1), ..at_width(1920) }, true, false, later, said(&["1080p", "French subtitles"]));
        assert_eq!(alphas(&chips, later), [1.0, 0.0], "the resolution stays, the subtitles fade in");
        assert_eq!(chips.until, later + FADE_IN_MS + HOLD_MS);
        let gone = chips.until + FADE_OUT_MS;
        chips.observe(at_width(1280), true, true, gone, said(&["720p"]));
        assert_eq!(chips.fingerprint(gone), 0, "a busy pipeline is not read");
        chips.observe(at_width(1280), false, false, gone, said(&["720p"]));
        assert_ne!(chips.fingerprint(gone), 0, "a paused picture is");
        chips.reset();
        chips.observe(at_width(1280), false, false, gone, said(&["720p"]));
        assert_eq!(chips.fingerprint(gone), 0, "a new playback waits for its first picture");
    }

    /// Every chip fits [`CHIP_W_CAP`] in every shipped language, with a long language name and the
    /// widest layout.
    #[test]
    fn every_chip_fits_its_cap_in_every_language() {
        use plx_base::fontcov::advances::ShippedMeasure;
        use plx_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let audio = [atmos(), Stream { channels: 8, layout: "7.1".into(), ..Default::default() }];
        let subs = [Stream { id: 3, lang: "Portuguese (Brazil)".into(), ..Default::default() }];
        let mut out = Vec::new();
        for language in SHIPPED {
            let _guard = language_on_this_thread_for_test(language);
            for facts in [
                Facts { raster: (3840, 2160), picture: Picture::DolbyVision, audio: Some((0, Delivery::AsIs)), subtitles: Some(0) },
                Facts { picture: Picture::Hdr, audio: Some((1, Delivery::AsIs)), ..Facts::default() },
                Facts { audio: Some((1, Delivery::Stereo)), ..Facts::default() },
            ] {
                for word in words(&facts, &audio, &subs) {
                    if badge_w(&word, None, &ShippedMeasure) > CHIP_W_CAP * plx_ui::fit::HEADROOM {
                        out.push(format!("{}: {word}", language.tag()));
                    }
                }
            }
        }
        assert!(out.is_empty(), "chips cut short:\n  {}", out.join("\n  "));
    }
}
