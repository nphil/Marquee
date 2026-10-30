//! **What the player paints with a text caption besides libass's own pixels**: the ink (colour,
//! opacity, the Blend with video level), the soft depth shadow, the background box and the
//! placement (which edge, and the nudge). All of it is arithmetic on the viewer's [`SubtitleLook`]
//! and on the frame libass rendered, so it lives here, pure, and the screen only draws what these
//! functions return.
//!
//! None of it reaches libass: a change of colour, opacity, box, shadow or nudge takes effect on the
//! next frame with no restyle and no re-render, which is what makes the Settings preview instant.

use super::ass::Rect;
use super::text_style::{SCR_H, TOP_MARGIN_V};
use plx_plex::plex::session::{SubAnchor, SubBack, SubEdge, SubtitleLook};

/// The ink a caption is drawn with: `base` is the tone ladder's tint (`theme::SUBTITLE_INKS`),
/// `level` the Blend with video brightness when the look asks for it.
///
/// An authored ASS/SSA frame keeps its own colours (the ladder's tint applies as it always did)
/// unless the look says `Mine`; a text frame takes the look's colour, opacity and Blend level.
pub fn compose_ink(base: [f32; 4], look: &SubtitleLook, text_frame: bool, level: f32) -> [f32; 4] {
    if !text_frame && look.ass == plx_plex::plex::session::AssMode::Keep {
        return base;
    }
    let mut ink = base;
    if let Some(c) = look.color {
        ink[0] = ((c >> 16) & 0xFF) as f32 / 255.0;
        ink[1] = ((c >> 8) & 0xFF) as f32 / 255.0;
        ink[2] = (c & 0xFF) as f32 / 255.0;
    }
    if look.blend {
        for channel in &mut ink[..3] {
            *channel *= level;
        }
    }
    ink[3] *= f32::from(look.opacity) / 100.0;
    ink
}

/// One tap of the soft shadow: an offset in screen pixels and the alpha of the black copy drawn
/// there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tap {
    pub dx: f32,
    pub dy: f32,
    pub alpha: f32,
}

/// The copies of the caption that make a diffuse shadow: two rings of eight around a point a little
/// below the letters, the outer ring wider and fainter. `strength` is the look's edge strength
/// (1..=8) and `k` the frame's scale to the 1920 screen (the preview is smaller). Drawn under the
/// letters in black, the overlapping copies add up to a falloff that is densest at the letters.
pub fn soft_taps(strength: u8, k: f32) -> Vec<Tap> {
    let s = f32::from(strength.clamp(1, 8)) * k;
    let rings: [(f32, f32); 2] = [(0.9 * s, 0.11), (2.0 * s, 0.065)];
    let mut taps = Vec::with_capacity(16);
    for (radius, alpha) in rings {
        for (ux, uy) in RING {
            taps.push(Tap { dx: radius * ux, dy: 0.7 * s + radius * uy, alpha });
        }
    }
    taps
}

/// Eight points of the unit circle, 45 degrees apart from the right: constants, because the taps are
/// fixed offsets and no sine is worth a libm call per tap per frame (`ci/allow/libm.txt` keeps those
/// out of this layer).
const RING: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.707_106_8, 0.707_106_8),
    (0.0, 1.0),
    (-0.707_106_8, 0.707_106_8),
    (-1.0, 0.0),
    (-0.707_106_8, -0.707_106_8),
    (0.0, -1.0),
    (0.707_106_8, -0.707_106_8),
];

/// A line of ink in a frame, as the rows of one region found it. `region` indexes the frame's rects
/// (its lift is that region's); the rest is in frame pixels, `x1` and `y1` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub region: usize,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

/// Alpha (of 255) under which a pixel is not ink: the faint edge of a blur or an antialiased outline.
const INK_ALPHA: u8 = 24;

/// The lines of ink in `rects`: runs of rows that hold ink, with a gap of up to `merge_gap` blank
/// rows not splitting a run (the dot of an "i" is not a line), each with the columns its ink spans.
/// Pixel data is RGBA; a rect whose buffer is short is skipped.
pub fn line_bands(rects: &[Rect], merge_gap: i32) -> Vec<Band> {
    /// A run of inked rows being read: first and last row, leftmost and rightmost inked column.
    #[derive(Clone, Copy)]
    struct Run {
        first: usize,
        last: usize,
        left: usize,
        right: usize,
    }
    impl Run {
        fn band(self, region: usize, rect: &Rect) -> Band {
            Band {
                region,
                x0: rect.x + self.left as i32,
                y0: rect.y + self.first as i32,
                x1: rect.x + self.right as i32 + 1,
                y1: rect.y + self.last as i32 + 1,
            }
        }
    }
    let gap = merge_gap.max(0) as usize;
    let mut bands = Vec::new();
    for (region, rect) in rects.iter().enumerate() {
        let (w, h) = (rect.width.max(0) as usize, rect.height.max(0) as usize);
        if w == 0 || h == 0 || rect.rgba.len() < w * h * 4 {
            continue;
        }
        let mut run: Option<Run> = None;
        for row in 0..h {
            let px = &rect.rgba[row * w * 4..(row + 1) * w * 4];
            let Some(l) = px.chunks_exact(4).position(|p| p[3] > INK_ALPHA) else { continue };
            let r = px.chunks_exact(4).rposition(|p| p[3] > INK_ALPHA).unwrap_or(l);
            let fresh = Run { first: row, last: row, left: l, right: r };
            run = match run {
                Some(cur) if row - cur.last - 1 <= gap => {
                    Some(Run { first: cur.first, last: row, left: cur.left.min(l), right: cur.right.max(r) })
                }
                Some(cur) => {
                    bands.push(cur.band(region, rect));
                    Some(fresh)
                }
                None => Some(fresh),
            };
        }
        if let Some(cur) = run {
            bands.push(cur.band(region, rect));
        }
    }
    bands
}

/// One rounded rectangle to fill behind the words: frame pixels (`dy` not yet applied) and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxRect {
    pub region: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
}

/// The boxes behind `bands` for `look`: one per line, or one window around the lines of each region.
/// `k` is the frame's scale to the 1920 screen. Padding is the look's air in screen pixels, a half of
/// it above and below a line; the corner radius never exceeds half the box's height.
pub fn boxes(bands: &[Band], look: &SubtitleLook, k: f32) -> Vec<BoxRect> {
    if !look.has_back() {
        return Vec::new();
    }
    let pad = f32::from(look.back_pad) * k;
    let radius = f32::from(look.back_radius) * k;
    let make = |region: usize, x0: i32, y0: i32, x1: i32, y1: i32, px: f32, py: f32| {
        let (x, y) = (x0 as f32 - px, y0 as f32 - py);
        let (w, h) = ((x1 - x0) as f32 + 2.0 * px, (y1 - y0) as f32 + 2.0 * py);
        BoxRect { region, x, y, w, h, radius: radius.min(h / 2.0).min(w / 2.0) }
    };
    match look.back {
        SubBack::Off => Vec::new(),
        SubBack::Line => bands.iter().map(|b| make(b.region, b.x0, b.y0, b.x1, b.y1, pad, pad * 0.5)).collect(),
        SubBack::Window => {
            let mut out: Vec<BoxRect> = Vec::new();
            let mut regions: Vec<usize> = bands.iter().map(|b| b.region).collect();
            regions.dedup();
            for region in regions {
                let of: Vec<&Band> = bands.iter().filter(|b| b.region == region).collect();
                let (x0, y0) = (of.iter().map(|b| b.x0).min().unwrap_or(0), of.iter().map(|b| b.y0).min().unwrap_or(0));
                let (x1, y1) = (of.iter().map(|b| b.x1).max().unwrap_or(0), of.iter().map(|b| b.y1).max().unwrap_or(0));
                out.push(make(region, x0, y0, x1, y1, pad * 1.4, pad));
            }
            out
        }
    }
}

/// Where the viewer put the caption: the edge it hangs from and how far it is moved inward from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub anchor: SubAnchor,
    pub nudge: i32,
}

impl Placement {
    /// Bottom, not nudged: where every build before the Subtitles page drew.
    pub const DEFAULT: Placement = Placement { anchor: SubAnchor::Bottom, nudge: 0 };

    pub fn of(look: &SubtitleLook) -> Self {
        Self { anchor: look.anchor, nudge: i32::from(look.nudge) }
    }
}

/// The lowest y a caption block may reach: the screen's bottom safe margin (`ui::consts::MARGIN_Y`,
/// 54, which media hard-codes as `text_style::CAPTION_FLOOR_Y` does).
pub const SAFE_BOTTOM_Y: f32 = SCR_H - 54.0;

/// How far UP the moving group of a text frame goes, given the group's `top` and `bottom` y, the
/// transport HUD's lift (0 with the HUD down) and the placement; the result keeps the group on the
/// screen, inside the bottom safe margin. A bottom-anchored group rises by the HUD's lift and the
/// nudge; a top-anchored one is taken to the top margin plus the nudge, whatever the HUD does.
pub fn rise(top: f32, bottom: f32, hud_lift: f32, placement: Placement) -> f32 {
    let wanted = match placement.anchor {
        SubAnchor::Bottom => hud_lift + placement.nudge as f32,
        SubAnchor::Top => top - (TOP_MARGIN_V + placement.nudge as f32),
    };
    wanted.min(top.max(0.0)).max(bottom - SAFE_BOTTOM_Y)
}

/// The edge styles that need the soft shadow painted behind the letters.
pub fn wants_soft_shadow(look: &SubtitleLook) -> bool {
    look.edge == SubEdge::Soft
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_plex::plex::session::{AssMode, LookPreset};
    use std::sync::Arc;

    fn rect(x: i32, y: i32, w: i32, h: i32, inked: &[(i32, i32, i32)]) -> Rect {
        // `inked`: (row, from column, to column) runs of opaque pixels
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for &(row, from, to) in inked {
            for col in from..=to {
                let at = ((row * w + col) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        Rect { x, y, width: w, height: h, rgba: Arc::from(rgba) }
    }

    #[test]
    fn ink_follows_the_look_for_a_text_frame_and_keeps_an_authored_one() {
        let white = [1.0, 1.0, 1.0, 1.0];
        let look = SubtitleLook { color: Some(0xFFE680), opacity: 80, ..SubtitleLook::default() };
        let ink = compose_ink(white, &look, true, 1.0);
        assert_eq!(ink[3], 0.8);
        assert!((ink[0] - 1.0).abs() < 1e-6 && (ink[1] - 0.902).abs() < 1e-3 && (ink[2] - 0.502).abs() < 1e-3, "{ink:?}");
        assert_eq!(compose_ink(white, &look, false, 1.0), white, "authored colours are not repainted");
        let mine = SubtitleLook { ass: AssMode::Mine, ..look };
        assert_eq!(compose_ink(white, &mine, false, 1.0)[3], 0.8, "Mine takes over an authored frame");
        let blend = LookPreset::Blend.look();
        let soft = compose_ink(white, &blend, true, 0.8);
        assert!(soft[0] < 0.8 && soft[3] < 1.0, "Blend scales the colour by its level: {soft:?}");
        assert_eq!(compose_ink(white, &SubtitleLook::default(), true, 0.5), white, "the default look changes nothing");
    }

    #[test]
    fn the_soft_shadow_is_sixteen_faint_copies_below_the_letters() {
        let taps = soft_taps(3, 1.0);
        assert_eq!(taps.len(), 16);
        assert!(taps.iter().all(|t| t.alpha > 0.0 && t.alpha < 0.2));
        let mean_dy: f32 = taps.iter().map(|t| t.dy).sum::<f32>() / 16.0;
        assert!(mean_dy > 0.0, "the shadow sits below the letters: {mean_dy}");
        let mean_dx: f32 = taps.iter().map(|t| t.dx).sum::<f32>() / 16.0;
        assert!(mean_dx.abs() < 1e-4, "and is centred on them: {mean_dx}");
        let wider = soft_taps(6, 1.0);
        assert!(wider.iter().map(|t| t.dx.abs()).fold(0.0, f32::max) > taps.iter().map(|t| t.dx.abs()).fold(0.0, f32::max));
        let preview = soft_taps(3, 0.5);
        assert!((preview[8].dx - taps[8].dx * 0.5).abs() < 1e-4, "a smaller frame has a proportionally smaller shadow");
        // the centre of the falloff: the copies stack to well under opaque
        let stacked: f32 = 1.0 - taps.iter().fold(1.0, |acc, t| acc * (1.0 - t.alpha));
        assert!(stacked > 0.6 && stacked < 0.95, "{stacked}");
    }

    #[test]
    fn lines_are_found_by_their_rows_and_a_dot_does_not_split_one() {
        // two lines of ink 8 blank rows apart, the second with a 2-row dot gap inside it
        let r = rect(100, 800, 40, 60, &[(2, 5, 30), (3, 5, 30), (4, 6, 29), (14, 8, 12), (15, 8, 12), (18, 8, 34), (19, 8, 34)]);
        let bands = line_bands(&[r], 4);
        assert_eq!(bands.len(), 2, "{bands:?}");
        assert_eq!(bands[0], Band { region: 0, x0: 105, y0: 802, x1: 131, y1: 805 });
        assert_eq!(bands[1], Band { region: 0, x0: 108, y0: 814, x1: 135, y1: 820 }, "rows 14..19 are one line: the gap is 2");
        assert!(line_bands(&[rect(0, 0, 10, 10, &[])], 4).is_empty(), "no ink, no band");
        let short = Rect { x: 0, y: 0, width: 10, height: 10, rgba: Arc::from(vec![0u8; 8]) };
        assert!(line_bands(&[short], 4).is_empty(), "a short buffer is skipped, never read past");
    }

    #[test]
    fn a_line_box_hugs_each_line_and_a_window_wraps_them_all() {
        let bands = [
            Band { region: 0, x0: 400, y0: 900, x1: 1000, y1: 940 },
            Band { region: 0, x0: 500, y0: 950, x1: 900, y1: 990 },
        ];
        let line = SubtitleLook { back: SubBack::Line, back_opacity: 80, back_pad: 10, back_radius: 8, ..SubtitleLook::default() };
        let boxes_ = boxes(&bands, &line, 1.0);
        assert_eq!(boxes_.len(), 2);
        assert_eq!((boxes_[0].x, boxes_[0].y, boxes_[0].w, boxes_[0].h), (390.0, 895.0, 620.0, 50.0));
        assert_eq!(boxes_[0].radius, 8.0);
        let window = SubtitleLook { back: SubBack::Window, ..line };
        let w = boxes(&bands, &window, 1.0);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].x, w[0].w), (400.0 - 14.0, 600.0 + 28.0), "wider air at the sides of a window");
        assert!(w[0].y < 900.0 && w[0].y + w[0].h > 990.0);
        let round = SubtitleLook { back_radius: 40, ..line };
        assert!(boxes(&bands, &round, 1.0)[1].radius <= 25.0, "the corner never exceeds half the height");
        assert!(boxes(&bands, &SubtitleLook { back: SubBack::Off, ..line }, 1.0).is_empty());
        assert!(boxes(&bands, &SubtitleLook { back_opacity: 0, ..line }, 1.0).is_empty(), "an invisible box is no box");
        let preview = boxes(&bands, &line, 0.5);
        assert_eq!(preview[0].radius, 4.0, "the preview's corners are proportionally smaller");
    }

    #[test]
    fn a_bottom_caption_rises_by_the_hud_and_the_nudge_and_stays_on_screen() {
        let bottom = Placement::DEFAULT;
        assert_eq!(rise(900.0, 980.0, 0.0, bottom), 0.0, "the default look does not move");
        assert_eq!(rise(900.0, 980.0, 200.0, bottom), 200.0, "the HUD's lift, as before");
        assert_eq!(rise(900.0, 980.0, 0.0, Placement { anchor: SubAnchor::Bottom, nudge: 120 }), 120.0);
        assert_eq!(rise(900.0, 980.0, 200.0, Placement { anchor: SubAnchor::Bottom, nudge: 120 }), 320.0);
        assert_eq!(rise(900.0, 980.0, 0.0, Placement { anchor: SubAnchor::Bottom, nudge: -60 }), -46.0, "down, but not past the safe margin");
        assert_eq!(rise(100.0, 180.0, 0.0, Placement { anchor: SubAnchor::Bottom, nudge: 400 }), 100.0, "never off the top");
    }

    #[test]
    fn a_top_caption_goes_to_the_top_margin_whatever_the_hud_does() {
        let top = Placement { anchor: SubAnchor::Top, nudge: 0 };
        assert_eq!(rise(900.0, 980.0, 0.0, top), 800.0, "its top lands on the 100 px margin");
        assert_eq!(rise(900.0, 980.0, 200.0, top), 800.0, "the HUD is at the bottom: it does not matter");
        assert_eq!(rise(860.0, 980.0, 0.0, top), 760.0, "a taller block rises less, so its top stays put");
        assert_eq!(rise(900.0, 980.0, 0.0, Placement { anchor: SubAnchor::Top, nudge: 50 }), 750.0);
    }

    #[test]
    fn only_the_soft_edge_asks_for_the_diffuse_shadow() {
        assert!(wants_soft_shadow(&LookPreset::Blend.look()));
        assert!(!wants_soft_shadow(&LookPreset::Netflix.look()));
        assert!(!wants_soft_shadow(&SubtitleLook::default()));
    }
}
