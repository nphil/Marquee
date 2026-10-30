//! The look of a text subtitle, as numbers: the viewer's size and position picks mapped to the
//! constants the synthesized ASS header (`text_ass::header`) and the screen's HUD lift are built
//! from. Pure; nothing here touches libass, a font or the screen.
//!
//! Media cannot use appkit and the synthesized header needs these values, so the mapping lives
//! here and appkit reads it (plan `libass-text-subs` §2.5). [`today_baseline`] and its companions
//! mirror the maths `player_hud::caption_geometry`/`caption_layout` use, kept with hard-coded
//! constants so a tie test in appkit can pin that the two cannot drift apart.

use plx_plex::plex::session::{AssMode, Reveal, SubEdge, SubFont, SubtitleLook, SubtitlePosition, SubtitleSize};

/// The logical canvas a text-origin subtitle frame is rendered into (`PlayResX/Y` of the header).
/// `plx_base::surface::LOGICAL_*` is the one definition of the canvas; they are whole numbers.
pub const SCR_W: f32 = plx_base::surface::LOGICAL_W;
pub const SCR_H: f32 = plx_base::surface::LOGICAL_H;

/// The caption's face in px at scale 1 (the default Medium size), before [`size_scale`].
pub const CAPTION_FACE: f32 = 36.0;

/// The vertical pitch of one caption line at scale 1; it scales with the face.
pub const CAPTION_LINE_PITCH: f32 = 48.0;

/// The widest a caption line runs at the base face: about 42 characters of bold caption text.
pub const CAPTION_WRAP_W: f32 = 760.0;

/// The widest a caption line ever runs, at any size: three quarters of the screen.
pub const CAPTION_MAX_W: f32 = SCR_W * 0.75;

/// The caption line count the block is bottom-anchored at; the Timing capsule's y is fixed by it.
pub const CAPTION_ANCHOR_LINES: usize = 3;

/// The gap between the bottom of the frame and the bottom of the bespoke caption block at the Low
/// position (`SCR_H - SUB_BASE_Y`), and, symmetrically, the gap between the top of the frame and the
/// top of the first line of a top-aligned cue ([`TOP_MARGIN_V`]). It is the geometry of the block
/// the appkit draw places, which the Timing capsule is cleared against, so it is NOT the libass
/// `MarginV` of the Low position: libass sets the same line on a different box ([`INK_TRIM`]).
pub const BOTTOM_GAP: f32 = 100.0;

/// How much nearer the frame's bottom libass's `MarginV` sits than [`BOTTOM_GAP`], so the ink
/// baseline of a one-line cue lands where [`today_ink_baseline`] puts it. Fixed by the native
/// golden (`ass::tests::synth`): with `MarginV = BOTTOM_GAP`, libass's Inter Bold baseline
/// sat 3.56 / 3 / 2 / 2 px ABOVE today's at the four sizes (971 / 968 / 966 / 963 against
/// 974.56 / 971 / 968 / 965 at Low), because the two layouts put the line on different boxes;
/// shifting every size down by 3 px leaves each within 1. It costs 3 px of the clearance the HUD
/// lift leaves under the band's ceiling (the lifted ink of "Hxg" ends about 2 px past it, today's
/// "g" about 1).
pub const INK_TRIM: f32 = 3.0;

/// The inset the bespoke caption draw adds under a line's top (`player_hud::draw_caption_block`
/// draws at `line_top + 4`).
pub const CAPTION_TEXT_INSET: f32 = 4.0;

/// The top of classic's control row, the one input of [`HudBand::CLASSIC`]: a text caption lifted
/// with the HUD up must end above it, deep marks included (plan §2.4, as amended: the golden's
/// criterion is this row, not the ceiling above it). Nothing else reads it; every reader takes the
/// band as an argument.
const HUD_CONTROLS_TOP_Y: f32 = SCR_H - 288.0;

/// The gap between a HUD's control row and the ceiling a lifted caption is held under: classic's own
/// 792 - 780. The lifted ink of "Hxg" overhangs that ceiling by about 2 px ([`INK_TRIM`]).
pub const CAPTION_INK_CLEARANCE: f32 = 12.0;

/// The `MarginV` of the top-aligned styles: independent of the Position setting.
pub const TOP_MARGIN_V: f32 = BOTTOM_GAP;

/// Where the caption block sits with the transport DOWN, before the Position pick. With the
/// transport UP it lifts to the ceiling of the HUD's [`HudBand`].
pub const SUB_BASE_Y: f32 = SCR_H - BOTTOM_GAP;

/// **The band a HUD occupies at the bottom of the screen**, as the three numbers every caption
/// reader needs: the top of the control row (`controls_top_y`), the ceiling a lifted caption is held
/// under (`ceil_y`, clear of the row's top by [`CAPTION_INK_CLEARANCE`]) and the fixed distance a
/// text caption moves up while the HUD is up (`lift`, `SUB_BASE_Y - ceil_y`, the same at every
/// Position and size: plan §2.4). It is a value the layout reports, never a global: the readers
/// (`caption_geometry_for`, `hud_lift`, `today_baseline`, the libass lift) take it as an argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudBand {
    pub ceil_y: f32,
    pub controls_top_y: f32,
    pub lift: f32,
}

impl HudBand {
    /// The one constructor: `ceil_y` and `lift` follow from the control row's top.
    pub const fn from_controls_top(controls_top_y: f32) -> Self {
        let ceil_y = controls_top_y - CAPTION_INK_CLEARANCE;
        Self { ceil_y, controls_top_y, lift: SUB_BASE_Y - ceil_y }
    }

    /// Classic's band: control row at 792, ceiling 780, lift 200 (`text_style::tests` pins them).
    pub const CLASSIC: Self = Self::from_controls_top(HUD_CONTROLS_TOP_Y);
}

/// The Timing capsule's bottom y: [`SUB_BASE_Y`] cleared by the tallest caption block that stays
/// bottom-anchored, so a three-line caption never overlaps the capsule.
pub const CAPSULE_BOTTOM_Y: f32 = SUB_BASE_Y - CAPTION_ANCHOR_LINES as f32 * CAPTION_LINE_PITCH;

/// The screen's bottom margin (`plx_ui::consts::MARGIN_Y`, which media cannot name): the lowest y
/// a caption block may reach with the transport down.
pub const CAPTION_FLOOR_Y: f32 = SCR_H - 54.0;

/// libass sizes a face by its OS/2 win metrics, not by its em: for the shipped Inter Bold,
/// `(usWinAscent 2269 + usWinDescent 660) / unitsPerEm 2048`. A style's `Fontsize` is therefore
/// `CAPTION_FACE * scale * INTER_BOLD_WIN_EM` to draw today's em. `text_style::tests::inter_win_ratio`
/// reads the shipped TTF and fails if this stops being true.
pub const INTER_BOLD_WIN_ASCENT_EM: f64 = 2269.0 / 2048.0;
pub const INTER_BOLD_WIN_DESCENT_EM: f64 = 660.0 / 2048.0;
pub const INTER_BOLD_WIN_EM: f64 = INTER_BOLD_WIN_ASCENT_EM + INTER_BOLD_WIN_DESCENT_EM;

/// Inter Bold's `hhea` ascender over its em (`1984 / 2048`): SDL_ttf puts the baseline of the
/// bespoke draw `ceil(face * INTER_BOLD_HHEA_ASCENT_EM)` rows under a line's draw y.
/// `text_style::tests::inter_win_ratio` pins it to the shipped TTF.
pub const INTER_BOLD_HHEA_ASCENT_EM: f32 = 1984.0 / 2048.0;

/// The face-size multiplier for the viewer's Settings > Playback size pick.
pub fn size_scale(size: SubtitleSize) -> f32 {
    match size {
        SubtitleSize::Small => 0.78,
        SubtitleSize::Medium => 1.0,
        SubtitleSize::Large => 1.25,
        SubtitleSize::ExtraLarge => 1.5,
        SubtitleSize::Huge => 1.8,
    }
}

/// How far ABOVE the Low baseline the viewer's Position pick lifts the caption. Low is the
/// baseline every build before the preference drew, so it lifts nothing.
pub fn position_lift(position: SubtitlePosition) -> f32 {
    match position {
        SubtitlePosition::Low => 0.0,
        SubtitlePosition::Middle => 160.0,
        SubtitlePosition::High => 320.0,
    }
}

/// The pixel width a caption line wraps at for the viewer's size `scale`.
pub fn wrap_width(scale: f32) -> f32 {
    (CAPTION_WRAP_W * scale).min(CAPTION_MAX_W)
}

/// The left and right margin of the synthesized styles: the wrap width centred on the frame,
/// rounded to the whole pixel an ASS margin is.
pub fn side_margin(scale: f32) -> u32 {
    ((SCR_W - wrap_width(scale)) / 2.0).round().max(0.0) as u32
}

/// The `MarginV` of the bottom-aligned styles: the gap under the last line ([`BOTTOM_GAP`] less
/// [`INK_TRIM`]) plus the Position lift.
pub fn bottom_margin_v(position: SubtitlePosition) -> u32 {
    (BOTTOM_GAP - INK_TRIM + position_lift(position)).round() as u32
}

/// The y (frame pixels, from the top) of the bottom of the last line box of a bottom-aligned cue
/// at `position`: `PlayResY - MarginV`, where libass puts it (plan §2.2). The screen's lift uses it
/// to tell a bottom region from a top or middle one.
pub fn bottom_anchor_y(position: SubtitlePosition) -> f32 {
    SCR_H - bottom_margin_v(position) as f32
}

/// The `Fontsize` of the synthesized styles at `scale` for the default face: today's em times the
/// Inter win ratio.
pub fn font_size(scale: f32) -> f64 {
    font_size_for(SubFont::Default, scale)
}

/// How libass is told to find and size one family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontInfo {
    /// The `Fontname` of the regular weight.
    pub regular: &'static str,
    /// The `Fontname` of the bold weight.
    pub bold: &'static str,
    /// Whether the bold weight is picked by the style's `Bold` flag (a family with a regular and a
    /// bold face) or by its name (the Default face's Bold answers to "PlxSubs" at `Bold` 0).
    pub bold_by_flag: bool,
    /// `(usWinAscent + usWinDescent) / unitsPerEm` of the family's faces: libass sizes a face by its
    /// win metrics, so a `Fontsize` of `em * win_em` draws that em. `tests::win_ratios` reads the
    /// shipped files and fails if one stops matching.
    pub win_em: f64,
}

/// Noto Serif's win ratio, (usWinAscent 1069 + usWinDescent 389) / unitsPerEm 1000
/// (`pkg/fonts/subs/families.json`; `tests::the_family_win_ratios_are_the_shipped_files`).
pub const SERIF_WIN_EM: f64 = 1.458;
/// Noto Sans Mono's win ratio, (1229 + 389) / 1000.
pub const MONO_WIN_EM: f64 = 1.618;
/// Atkinson Hyperlegible's win ratio, (950 + 290) / 1000.
pub const LEGIBLE_WIN_EM: f64 = 1.24;

pub fn font_info(font: SubFont) -> FontInfo {
    match font {
        SubFont::Default => FontInfo { regular: "Inter", bold: super::text_ass::STYLE_FONT, bold_by_flag: false, win_em: INTER_BOLD_WIN_EM },
        SubFont::Serif => FontInfo { regular: "PlxSerif", bold: "PlxSerif", bold_by_flag: true, win_em: SERIF_WIN_EM },
        SubFont::Mono => FontInfo { regular: "PlxMono", bold: "PlxMono", bold_by_flag: true, win_em: MONO_WIN_EM },
        SubFont::Legible => FontInfo { regular: "PlxLegible", bold: "PlxLegible", bold_by_flag: true, win_em: LEGIBLE_WIN_EM },
    }
}

/// The `Fontsize` that draws today's em at `scale` in `font`.
pub fn font_size_for(font: SubFont, scale: f32) -> f64 {
    CAPTION_FACE as f64 * scale as f64 * font_info(font).win_em
}

/// The renderer-wide line spacing (`ass_set_line_spacing`, frame pixels) that turns libass's
/// face-height advance into today's pitch of `CAPTION_LINE_PITCH * scale`.
pub fn line_spacing(scale: f32) -> f64 {
    CAPTION_LINE_PITCH as f64 * scale as f64 - font_size(scale)
}

/// [`line_spacing`] for a font and a line-pitch percentage (100 is the usual pitch).
pub fn line_spacing_for(font: SubFont, scale: f32, line_pct: u8) -> f64 {
    CAPTION_LINE_PITCH as f64 * scale as f64 * f64::from(line_pct) / 100.0 - font_size_for(font, scale)
}

/// The part of the viewer's [`SubtitleLook`] that changes what libass draws or how a cue's events
/// read: a change of any of these is a RESTYLE (a new header, a replaced track), and nothing else
/// is. Colour, opacity, the box, the soft shadow, the anchor and nudge and the Blend tone are
/// applied when the frame is painted, so they take effect on the next frame with no restyle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub font: SubFont,
    pub bold: bool,
    /// Exact size in percent, 0 to follow the Size ladder.
    pub size_pct: u16,
    pub edge: SubEdge,
    pub edge_w: u8,
    pub line_pct: u8,
    pub reveal: Reveal,
    pub ass: AssMode,
}

impl Layout {
    pub fn of(look: &SubtitleLook) -> Self {
        Self {
            font: look.font,
            bold: look.bold,
            size_pct: look.size_pct,
            edge: look.edge,
            edge_w: look.edge_w,
            line_pct: look.line_pct,
            reveal: look.reveal,
            ass: look.ass,
        }
    }
}

impl Default for Layout {
    /// The look every build before the Subtitles page drew.
    fn default() -> Self {
        Layout::of(&SubtitleLook::default())
    }
}

/// What the synthesized header is a function of: the viewer's size and position picks and the
/// layout half of the look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    pub size: SubtitleSize,
    pub position: SubtitlePosition,
    pub layout: Layout,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::picks(SubtitleSize::Medium, SubtitlePosition::Low)
    }
}

impl TextStyle {
    /// Two ladder picks with the default layout: the look before the Subtitles page.
    pub const fn picks(size: SubtitleSize, position: SubtitlePosition) -> Self {
        Self {
            size,
            position,
            layout: Layout {
                font: SubFont::Default,
                bold: true,
                size_pct: 0,
                edge: SubEdge::Outline,
                edge_w: 2,
                line_pct: 100,
                reveal: Reveal::Off,
                ass: AssMode::Keep,
            },
        }
    }

    /// The picks and the look as the preferences hold them now.
    pub fn current() -> Self {
        Self::of(crate::route::subtitle_size(), crate::route::subtitle_position(), &crate::route::subtitle_look())
    }

    /// The style of two ladder picks and a look.
    pub fn of(size: SubtitleSize, position: SubtitlePosition, look: &SubtitleLook) -> Self {
        Self { size, position, layout: Layout::of(look) }
    }

    /// The face-size multiplier: the exact size when the look has one, else the Size ladder's.
    pub fn scale(&self) -> f32 {
        if self.layout.size_pct != 0 {
            f32::from(self.layout.size_pct) / 100.0
        } else {
            size_scale(self.size)
        }
    }

    /// The `Fontsize` of this style's header.
    pub fn font_size(&self) -> f64 {
        font_size_for(self.layout.font, self.scale())
    }

    /// The renderer-wide line spacing of this style (see [`line_spacing_for`]).
    pub fn line_spacing(&self) -> f64 {
        line_spacing_for(self.layout.font, self.scale(), self.layout.line_pct)
    }

    /// A number that differs for every distinct style, so a stored copy is stale exactly when the
    /// header it was built for no longer matches. A 32-bit FNV-1a over every field.
    pub fn generation(&self) -> u32 {
        let l = &self.layout;
        let mut h: u32 = 0x811c_9dc5;
        let mut mix = |v: u32| {
            for b in v.to_le_bytes() {
                h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
            }
        };
        mix(self.size.index() as u32);
        mix(self.position.index() as u32);
        mix(l.font.index() as u32);
        mix(u32::from(l.bold));
        mix(u32::from(l.size_pct));
        mix(l.edge.index() as u32);
        mix(u32::from(l.edge_w));
        mix(u32::from(l.line_pct));
        mix(l.reveal.index() as u32);
        mix(l.ass.index() as u32);
        h
    }
}

/// The generation of the picks as the preferences hold them now (see [`TextStyle::generation`]).
pub fn generation() -> u32 {
    TextStyle::current().generation()
}

/// The edge half of a synthesized `Style:` line, from the look's edge style and strength. Colours
/// are ASS `&HAABBGGRR` as a number (alpha 0 is opaque). Only the edges libass can draw are here:
/// the soft depth shadow of [`SubEdge::Soft`] is painted behind the letters with the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeFields {
    pub outline: f64,
    pub shadow: f64,
    pub outline_colour: u32,
    pub back_colour: u32,
}

/// `edge` at strength `w` (pixels at the 1080 canvas).
pub fn edge_fields(edge: SubEdge, w: u8) -> EdgeFields {
    let w = f64::from(w.clamp(1, 8));
    // today's outline: black at 85 %, fully transparent shadow
    const BLACK_85: u32 = 0x2600_0000;
    const NO_BACK: u32 = 0xFF00_0000;
    match edge {
        SubEdge::None => EdgeFields { outline: 0.0, shadow: 0.0, outline_colour: BLACK_85, back_colour: NO_BACK },
        SubEdge::Outline => EdgeFields { outline: w, shadow: 0.0, outline_colour: BLACK_85, back_colour: NO_BACK },
        SubEdge::Drop => EdgeFields { outline: 0.0, shadow: w, outline_colour: BLACK_85, back_colour: 0x4000_0000 },
        SubEdge::Raised => EdgeFields { outline: 1.0, shadow: 1.5, outline_colour: 0x60FF_FFFF, back_colour: 0x2000_0000 },
        SubEdge::Depressed => EdgeFields { outline: 1.0, shadow: 1.5, outline_colour: 0x2000_0000, back_colour: 0x70FF_FFFF },
        SubEdge::Soft => EdgeFields { outline: 1.0, shadow: 0.0, outline_colour: 0x8000_0000, back_colour: NO_BACK },
    }
}

impl Layout {
    /// This layout's edge half.
    pub fn edge_fields(&self) -> EdgeFields {
        edge_fields(self.edge, self.edge_w)
    }
}

/// `&H26000000`: an ASS colour as the header writes it.
pub fn ass_colour(c: u32) -> String {
    format!("&H{c:08X}")
}

/// What `player_hud::caption_geometry` computes today: the viewer's scale, the y the caption
/// block's bottom is placed at, and the lowest y the block may reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TodayBaseline {
    pub scale: f32,
    pub baseline: f32,
    pub floor: f32,
}

/// Port of `player_hud::caption_geometry(hud_up)` for a given `scale`, `position` and HUD `band`.
pub fn today_baseline(position: SubtitlePosition, scale: f32, hud_up: bool, band: HudBand) -> TodayBaseline {
    let baseline = (if hud_up { band.ceil_y } else { SUB_BASE_Y }) - position_lift(position);
    let floor = if hud_up { band.ceil_y } else { CAPTION_FLOOR_Y };
    TodayBaseline { scale, baseline, floor }
}

/// Where today's one-line caption puts the BASELINE of its glyphs (the row just under a flat-bottomed
/// capital such as "H"): `baseline` and `floor` above are the edges of the block, and the text is
/// drawn at `top + CAPTION_TEXT_INSET` with the baseline `ceil(ascender)` rows under that. This is
/// the number the libass look is held to (`ass::tests::synth`), 5 / 9 / 12 / 15 px above
/// [`TodayBaseline::baseline`] at the four sizes. A port of that draw: `draw_caption_block`
/// (appkit) takes its inset from [`CAPTION_TEXT_INSET`], and the ascent is
/// SDL_ttf's `ceil(hhea ascender)` as `gfx::text` documents it, not measured on a television.
pub fn today_ink_baseline(position: SubtitlePosition, scale: f32, hud_up: bool, band: HudBand) -> f32 {
    let lay = today_layout(1, scale, today_baseline(position, scale, hud_up, band).baseline);
    lay.top + CAPTION_TEXT_INSET + (INTER_BOLD_HHEA_ASCENT_EM * lay.face as f32).ceil()
}

/// Where today's caption block of `lines` lines is drawn: the face in px, the line pitch, and the
/// y of the first line's box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TodayLayout {
    pub face: i32,
    pub pitch: f32,
    pub top: f32,
}

/// Port of `player_hud::caption_layout` for a block that fits (`lines <= CAPTION_ANCHOR_LINES`,
/// the only blocks the removed fit steps never touched): its bottom on `baseline`.
pub fn today_layout(lines: usize, scale: f32, baseline: f32) -> TodayLayout {
    let anchored = lines.min(CAPTION_ANCHOR_LINES) as f32;
    TodayLayout {
        face: (CAPTION_FACE * scale).round() as i32,
        pitch: CAPTION_LINE_PITCH * scale,
        top: baseline - anchored * CAPTION_LINE_PITCH * scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(b: &[u8], at: usize) -> u32 {
        u16::from_be_bytes([b[at], b[at + 1]]) as u32
    }

    fn be32(b: &[u8], at: usize) -> usize {
        u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
    }

    /// (unitsPerEm, usWinAscent, usWinDescent) read from an SFNT's `head` and `OS/2` tables.
    fn win_metrics(font: &[u8]) -> (u32, u32, u32) {
        let tables = be16(font, 4) as usize;
        let find = |tag: &[u8; 4]| {
            (0..tables)
                .map(|i| 12 + i * 16)
                .find(|&at| &font[at..at + 4] == tag)
                .map(|at| be32(font, at + 8))
                .unwrap_or_else(|| panic!("table {tag:?} missing"))
        };
        let head = find(b"head");
        let os2 = find(b"OS/2");
        (be16(font, head + 18), be16(font, os2 + 74), be16(font, os2 + 76))
    }

    #[test]
    fn inter_win_ratio() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pkg/appfont-bold.ttf");
        let font = std::fs::read(&path).expect("the shipped bold face");
        let hhea = {
            let tables = be16(&font, 4) as usize;
            let at = (0..tables).map(|i| 12 + i * 16).find(|&at| &font[at..at + 4] == b"hhea").expect("hhea");
            be16(&font, be32(&font, at + 8) + 4)
        };
        assert_eq!(hhea, 1984, "Inter Bold's hhea ascender changed");
        assert!((hhea as f32 / 2048.0 - INTER_BOLD_HHEA_ASCENT_EM).abs() < 1e-9);
        let (upem, ascent, descent) = win_metrics(&font);
        assert_eq!((upem, ascent, descent), (2048, 2269, 660), "Inter Bold's metrics changed");
        let ratio = (ascent + descent) as f64 / upem as f64;
        assert!((ratio - INTER_BOLD_WIN_EM).abs() < 1e-9, "{ratio} != {INTER_BOLD_WIN_EM}");
        assert_eq!(INTER_BOLD_WIN_ASCENT_EM, ascent as f64 / upem as f64);
        assert_eq!(INTER_BOLD_WIN_DESCENT_EM, descent as f64 / upem as f64);
    }

    #[test]
    fn font_size_gives_todays_em_and_pitch() {
        for size in SubtitleSize::LADDER {
            let scale = size_scale(size);
            let fs = font_size(scale);
            assert!((fs / INTER_BOLD_WIN_EM - 36.0 * scale as f64).abs() < 1e-6, "em at {scale}");
            assert!((fs + line_spacing(scale) - 48.0 * scale as f64).abs() < 1e-9, "pitch at {scale}");
        }
        assert!((font_size(1.0) - 51.5).abs() < 0.05, "plan §2.2 quotes about 51.5");
        assert!((line_spacing(1.0) + 3.5).abs() < 0.05, "plan §2.2 quotes about -3.5");
    }

    #[test]
    fn scales_and_lifts_are_todays() {
        let scales: Vec<f32> = SubtitleSize::LADDER.iter().map(|s| size_scale(*s)).collect();
        assert_eq!(scales, [0.78, 1.0, 1.25, 1.5, 1.8]);
        let lifts: Vec<f32> = SubtitlePosition::LADDER.iter().map(|p| position_lift(*p)).collect();
        assert_eq!(lifts, [0.0, 160.0, 320.0]);
        assert_eq!(SUB_BASE_Y, 980.0);
        assert_eq!(CAPSULE_BOTTOM_Y, 836.0);
    }

    /// `CLASSIC` is the constructor's value for today's control row, and today's three numbers.
    #[test]
    fn classic_band_is_todays_constants() {
        assert_eq!(HUD_CONTROLS_TOP_Y, 792.0);
        assert_eq!(HudBand::CLASSIC, HudBand::from_controls_top(HUD_CONTROLS_TOP_Y));
        assert_eq!(
            HudBand::CLASSIC,
            HudBand { ceil_y: 780.0, controls_top_y: 792.0, lift: 200.0 }
        );
        let band = HudBand::from_controls_top(700.0);
        assert_eq!(band, HudBand { ceil_y: 688.0, controls_top_y: 700.0, lift: 292.0 });
        assert_eq!(band.ceil_y, band.controls_top_y - CAPTION_INK_CLEARANCE);
        assert_eq!(band.lift, SUB_BASE_Y - band.ceil_y);
    }

    #[test]
    fn wrap_margins_centre_the_wrap_width() {
        assert_eq!(side_margin(1.0), 580); // (1920 - 760) / 2
        assert_eq!(side_margin(1.25), 485); // (1920 - 950) / 2
        assert_eq!(side_margin(1.5), 390); // (1920 - 1140) / 2
        assert_eq!(side_margin(0.78), 664); // (1920 - 592.8) / 2 = 663.6
        for size in SubtitleSize::LADDER {
            let s = size_scale(size);
            assert!(wrap_width(s) <= CAPTION_MAX_W);
            assert!((2 * side_margin(s)) as f32 + wrap_width(s) - SCR_W < 1.0);
        }
        // Past 1440 px the cap, not the scale, decides.
        assert_eq!(wrap_width(3.0), CAPTION_MAX_W);
        assert_eq!(side_margin(3.0), 240);
    }

    #[test]
    fn bottom_anchor_follows_position() {
        // BOTTOM_GAP less INK_TRIM, plus the Position lift.
        assert_eq!(bottom_margin_v(SubtitlePosition::Low), 97);
        assert_eq!(bottom_margin_v(SubtitlePosition::Middle), 257);
        assert_eq!(bottom_margin_v(SubtitlePosition::High), 417);
        assert_eq!(bottom_anchor_y(SubtitlePosition::Low), SUB_BASE_Y + INK_TRIM);
        assert_eq!(bottom_anchor_y(SubtitlePosition::High), 663.0);
    }

    #[test]
    fn today_baseline_is_the_old_geometry() {
        for position in SubtitlePosition::LADDER {
            let lift = position_lift(position);
            let down = today_baseline(position, 1.0, false, HudBand::CLASSIC);
            assert_eq!((down.baseline, down.floor), (980.0 - lift, 1026.0));
            let up = today_baseline(position, 1.0, true, HudBand::CLASSIC);
            assert_eq!((up.baseline, up.floor), (780.0 - lift, 780.0));
            // The HUD moves the baseline by the same 200 px at every position.
            assert_eq!(down.baseline - up.baseline, HudBand::CLASSIC.lift);
        }
    }

    /// A band that is not classic's moves the lifted baseline, floor and ink by exactly the
    /// band's difference, and leaves the HUD-down placement alone.
    #[test]
    fn a_non_classic_band_moves_the_lifted_caption_by_its_difference() {
        let band = HudBand::from_controls_top(700.0);
        let shift = HudBand::CLASSIC.controls_top_y - band.controls_top_y;
        assert_eq!(shift, 92.0);
        for position in SubtitlePosition::LADDER {
            for size in SubtitleSize::LADDER {
                let scale = size_scale(size);
                let classic_up = today_baseline(position, scale, true, HudBand::CLASSIC);
                let up = today_baseline(position, scale, true, band);
                assert_eq!(classic_up.baseline - up.baseline, shift, "{position:?} {size:?}");
                assert_eq!(classic_up.floor - up.floor, shift, "{position:?} {size:?}");
                // The ink row goes through the block's top, one more f32 subtraction.
                let ink = today_ink_baseline(position, scale, true, HudBand::CLASSIC)
                    - today_ink_baseline(position, scale, true, band);
                assert!((ink - shift).abs() < 1e-3, "{position:?} {size:?}: {ink} vs {shift}");
                assert_eq!(
                    today_baseline(position, scale, false, band),
                    today_baseline(position, scale, false, HudBand::CLASSIC),
                    "the HUD down ignores the band"
                );
            }
        }
        let down = today_baseline(SubtitlePosition::Low, 1.0, false, band).baseline;
        let up = today_baseline(SubtitlePosition::Low, 1.0, true, band).baseline;
        assert_eq!(down - up, band.lift);
    }

    #[test]
    fn today_ink_baseline_sits_under_the_block_top() {
        // 36 px face: line top 932, drawn at 936, baseline ceil(34.875) = 35 rows down.
        assert_eq!(today_ink_baseline(SubtitlePosition::Low, 1.0, false, HudBand::CLASSIC), 971.0);
        assert_eq!(today_ink_baseline(SubtitlePosition::Low, 1.0, true, HudBand::CLASSIC), 771.0);
        assert_eq!(today_ink_baseline(SubtitlePosition::High, 1.0, false, HudBand::CLASSIC), 651.0);
        // 28 / 45 / 54 px faces, pitch 37.44 / 60 / 72.
        assert!((today_ink_baseline(SubtitlePosition::Low, 0.78, false, HudBand::CLASSIC) - 974.56).abs() < 1e-3);
        assert_eq!(today_ink_baseline(SubtitlePosition::Low, 1.25, false, HudBand::CLASSIC), 968.0);
        assert_eq!(today_ink_baseline(SubtitlePosition::Low, 1.5, false, HudBand::CLASSIC), 965.0);
    }

    #[test]
    fn today_layout_anchors_at_three_lines() {
        let lay = today_layout(1, 1.0, SUB_BASE_Y);
        assert_eq!((lay.face, lay.pitch, lay.top), (36, 48.0, 932.0));
        assert_eq!(today_layout(3, 1.0, SUB_BASE_Y).top, CAPSULE_BOTTOM_Y);
        assert_eq!(today_layout(6, 1.0, SUB_BASE_Y).top, CAPSULE_BOTTOM_Y, "taller blocks keep the 3-line top");
        assert_eq!(today_layout(2, 1.5, SUB_BASE_Y).face, 54);
    }

    #[test]
    fn generation_tells_every_pair_apart() {
        let mut seen = std::collections::BTreeSet::new();
        for size in SubtitleSize::LADDER {
            for position in SubtitlePosition::LADDER {
                assert!(seen.insert(TextStyle::picks(size, position).generation()), "{size:?} {position:?}");
            }
        }
        assert_eq!(seen.len(), 15);
        assert_eq!(TextStyle::default().generation(), TextStyle::picks(SubtitleSize::Medium, SubtitlePosition::Low).generation());
    }

    /// Every layout field is a restyle; a painted-only field is not part of the generation at all.
    #[test]
    fn generation_follows_the_layout_and_ignores_what_is_only_painted() {
        use plx_plex::plex::session::{LookPreset, SubBack};
        let base = SubtitleLook::default();
        let style = |look: &SubtitleLook| TextStyle::of(SubtitleSize::Medium, SubtitlePosition::Low, look);
        let g = style(&base).generation();
        assert_eq!(g, TextStyle::default().generation());
        for changed in [
            SubtitleLook { font: SubFont::Serif, ..base },
            SubtitleLook { bold: false, ..base },
            SubtitleLook { size_pct: 130, ..base },
            SubtitleLook { edge: SubEdge::Drop, ..base },
            SubtitleLook { edge_w: 4, ..base },
            SubtitleLook { line_pct: 120, ..base },
            SubtitleLook { reveal: Reveal::Word, ..base },
            SubtitleLook { ass: AssMode::Mine, ..base },
        ] {
            assert_ne!(style(&changed).generation(), g, "{changed:?}");
        }
        for painted in [
            SubtitleLook { color: Some(0xFFE680), ..base },
            SubtitleLook { opacity: 50, ..base },
            SubtitleLook { back: SubBack::Window, back_opacity: 90, back_radius: 20, back_pad: 20, ..base },
            SubtitleLook { nudge: 120, ..base },
            SubtitleLook { blend: true, ..base },
            SubtitleLook { preset: LookPreset::Custom, ..base },
        ] {
            assert_eq!(style(&painted).generation(), g, "{painted:?} must not restyle");
        }
    }

    /// The look's size beats the ladder's, a zero follows it, and the pitch follows the line setting.
    #[test]
    fn the_exact_size_and_the_line_pitch_come_from_the_look() {
        let look = SubtitleLook { size_pct: 150, line_pct: 120, ..SubtitleLook::default() };
        let style = TextStyle::of(SubtitleSize::Small, SubtitlePosition::Low, &look);
        assert_eq!(style.scale(), 1.5);
        assert_eq!(TextStyle::of(SubtitleSize::Small, SubtitlePosition::Low, &SubtitleLook::default()).scale(), 0.78);
        assert!((style.font_size() - font_size(1.5)).abs() < 1e-9, "the default face's size is today's");
        assert!((style.line_spacing() - (48.0 * 1.5 * 1.2 - font_size(1.5))).abs() < 1e-9);
        assert!((TextStyle::default().line_spacing() - line_spacing(1.0)).abs() < 1e-9, "100 % is today's pitch");
    }

    #[test]
    fn every_edge_has_fields_and_the_default_is_todays_outline() {
        let today = edge_fields(SubEdge::Outline, 2);
        assert_eq!((today.outline, today.shadow, today.outline_colour, today.back_colour), (2.0, 0.0, 0x2600_0000, 0xFF00_0000));
        assert_eq!(ass_colour(today.outline_colour), "&H26000000");
        for edge in SubEdge::ALL {
            let f = edge_fields(*edge, 3);
            assert!(f.outline >= 0.0 && f.shadow >= 0.0, "{edge:?}");
        }
        assert_eq!(edge_fields(SubEdge::None, 3).outline, 0.0);
        assert_eq!(edge_fields(SubEdge::Drop, 3).shadow, 3.0);
        assert_eq!(edge_fields(SubEdge::Outline, 99).outline, 8.0, "strength is clamped");
    }

    /// The constants above are measured from the shipped files: libass sizes a face by its win
    /// metrics, so a wrong ratio would draw a family at the wrong size.
    #[test]
    fn the_family_win_ratios_are_the_shipped_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pkg/fonts/subs");
        for (file, ratio) in [
            ("PlxSerif-Regular.ttf", SERIF_WIN_EM),
            ("PlxSerif-Bold.ttf", SERIF_WIN_EM),
            ("PlxMono-Regular.ttf", MONO_WIN_EM),
            ("PlxMono-Bold.ttf", MONO_WIN_EM),
            ("PlxLegible-Regular.ttf", LEGIBLE_WIN_EM),
            ("PlxLegible-Bold.ttf", LEGIBLE_WIN_EM),
        ] {
            let font = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let (upem, ascent, descent) = win_metrics(&font);
            let measured = (ascent + descent) as f64 / upem as f64;
            assert!((measured - ratio).abs() < 1e-9, "{file}: {measured} != {ratio}");
        }
        for font in [SubFont::Default, SubFont::Serif, SubFont::Mono, SubFont::Legible] {
            // an em of 36 px at Medium draws the same in every family
            assert!((font_size_for(font, 1.0) / font_info(font).win_em - 36.0).abs() < 1e-9);
        }
    }
}
