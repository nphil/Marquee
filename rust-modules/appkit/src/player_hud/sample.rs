//! The sample line a style picker shows while no caption is on screen: the caption as the viewer's
//! look would draw it (face, size, colour, edge, box, placement), so a pick is seen at once.
//!
//! It is drawn with the client's own text, not by libass: the subtitle engine holds one source at a
//! time, and a second one for a sample would evict the film's own. A real cue replaces it the moment
//! one arrives, drawn by libass from the same look.

use super::*;
use plx_media::player::paint::{compose_ink, soft_taps};
use plx_media::player::text_style::{edge_fields, font_info, TextStyle, TOP_MARGIN_V};
use plx_plex::plex::session::{SubAnchor, SubBack, SubEdge};

/// An ASS `&HAABBGGRR` colour (alpha 0 opaque) as an ink.
fn ass_ink(c: u32) -> [f32; 4] {
    let ch = |shift: u32| ((c >> shift) & 0xFF) as f32 / 255.0;
    [ch(0), ch(8), ch(16), 1.0 - ch(24)]
}

fn rgb_ink(c: u32, opacity: u8) -> [f32; 4] {
    let ch = |shift: u32| ((c >> shift) & 0xFF) as f32 / 255.0;
    [ch(16), ch(8), ch(0), f32::from(opacity) / 100.0]
}

pub fn draw_style_sample(text: &str, hud_up: bool, band: HudBand, model: &HudModel, measure: &dyn Measure) {
    let look = &model.sub_look;
    let scale = TextStyle::of(model.sub_size, model.sub_position, look).scale();
    let face = (CAPTION_FACE * scale).round() as c_int;
    let bold = c_int::from(look.bold);
    let stem = font_info(look.font).regular;
    let tagged = |s: &str| plx_gfx::text::faced(stem, s);
    let width = |s: &str| measure.width_str(&tagged(s), face, look.bold);

    let mut lines: Vec<String> = Vec::new();
    for seg in text.split('\n').map(str::trim).filter(|s| !s.is_empty()) {
        lines.extend(wrap_px(seg, wrap_width(scale), &width));
    }
    if lines.is_empty() {
        return;
    }
    let (_, baseline, floor) =
        caption_geometry_for_block(model.sub_size, model.sub_position, hud_up, band, anchored_height(lines.len(), scale));
    let lay = caption_layout(lines.len(), scale, baseline, floor);
    let face = lay.face;
    let pitch = lay.pitch * f32::from(look.line_pct) / 100.0;
    let nudge = f32::from(look.nudge);
    let top = match look.anchor {
        SubAnchor::Top => TOP_MARGIN_V + nudge,
        SubAnchor::Bottom => lay.top - nudge,
    }
    .max(SAFE.y);
    let line_top = |i: usize| top + i as f32 * pitch;

    let p = Painter::root();
    let cx = SCR_W * 0.5;
    let widths: Vec<f32> = lines.iter().map(|l| measure.width_str(&tagged(l), face, look.bold)).collect();

    // the box behind the words: one per line, or one window around the block
    if look.back != SubBack::Off {
        let pad = f32::from(look.back_pad);
        let r = f32::from(look.back_radius);
        let col = rgb_ink(look.back_color, look.back_opacity);
        let widest = widths.iter().copied().fold(0.0, f32::max);
        match look.back {
            SubBack::Line => {
                for (i, w) in widths.iter().enumerate() {
                    p.rrect(Rect::new(cx - w * 0.5 - pad, line_top(i), w + 2.0 * pad, pitch), r, r, col);
                }
            }
            _ => p.rrect(
                Rect::new(cx - widest * 0.5 - pad, top - pad, widest + 2.0 * pad, pitch * lines.len() as f32 + 2.0 * pad),
                r,
                r,
                col,
            ),
        }
    }

    let k = scale; // an edge is measured on the 1080 canvas at Medium
    let edge = edge_fields(look.edge, look.edge_w);
    let ink = compose_ink(subtitle_ink_for(model.sub_tone), look, true, 1.0);
    let inset = CAPTION_TEXT_INSET * scale;
    for (i, ln) in lines.iter().enumerate() {
        let Ok(cs) = CString::new(tagged(ln)) else { continue };
        let (x, y) = (cx, line_top(i) + inset);
        let copy = |dx: f32, dy: f32, col: [f32; 4]| {
            p.text(cs.as_ptr(), x + dx, y + dy, face, col, 1, bold);
        };
        if look.edge == SubEdge::Soft {
            for t in soft_taps(look.edge_w, k) {
                copy(t.dx, t.dy, [0.0, 0.0, 0.0, t.alpha]);
            }
        }
        let shadow = edge.shadow as f32 * k;
        if shadow > 0.0 {
            copy(shadow, shadow, ass_ink(edge.back_colour));
        }
        let r = edge.outline as f32 * k;
        if r > 0.0 {
            let outline = ass_ink(edge.outline_colour);
            for (ux, uy) in [(1.0, 0.0), (0.707, 0.707), (0.0, 1.0), (-0.707, 0.707), (-1.0, 0.0), (-0.707, -0.707), (0.0, -1.0), (0.707, -0.707)] {
                if r > 3.0 {
                    copy(ux * r * 0.5, uy * r * 0.5, outline);
                }
                copy(ux * r, uy * r, outline);
            }
        }
        copy(0.0, 0.0, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ass_colour_reads_back_to_an_ink() {
        assert_eq!(ass_ink(0x2600_0000), [0.0, 0.0, 0.0, 1.0 - 0x26 as f32 / 255.0]);
        let yellow = ass_ink(0x0080_E6FF);
        assert!((yellow[0] - 1.0).abs() < 1e-6 && yellow[3] == 1.0, "{yellow:?}");
    }
}
