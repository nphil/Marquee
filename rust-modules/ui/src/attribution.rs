//! Third-party attribution: a partner's logo above the notice its terms require, drawn as one
//! block in a credits page.
//!
//! The TMDB logo is the only user today. TMDB's terms allow its approved logo only unaltered (no
//! recolouring, fading, flipping, rotating or change of aspect ratio) and less prominent than the
//! marks that identify the application, so the logo is rasterised from the official SVG at its own
//! aspect ratio, drawn with [`theme::IMAGE_AS_AUTHORED`], and held at the couch legibility floor
//! ([`theme::size::CAPTION`]) in height: a third of the screen title's size.

use crate::icons;
use crate::text_view::TextView;
use crate::{theme, Painter, Rect};
use std::cell::RefCell;
use std::os::raw::c_uint;

/// The official "Primary long (blue)" TMDB logo, byte for byte as published at
/// `https://www.themoviedb.org/assets/v4/logos/v2/blue_long_2-9665a76b…svg`. A test pins its hash.
pub const TMDB_LOGO_SVG: &str = include_str!("../../../assets/brand/tmdb-blue-long.svg");

/// SHA-256 of [`TMDB_LOGO_SVG`], which is also the hash TMDB embeds in the asset's address.
#[cfg(test)]
const TMDB_LOGO_SHA256: &str = "9665a76b1ae401a510ec1e0ca40ddcb3b0cfe45f1d51b77a308fea0845885648";

/// The SVG's `viewBox` (`0 0 489.04 35.4`): the logo's aspect ratio, which must not change.
const LOGO_VIEW_W: f32 = 489.04;
const LOGO_VIEW_H: f32 = 35.4;

/// The drawn logo's height: the couch legibility floor, so the wordmark reads like caption text.
pub const LOGO_H: f32 = theme::size::CAPTION as f32;

/// The logo's drawn size at its fixed height.
pub fn logo_size() -> (f32, f32) {
    (LOGO_H * LOGO_VIEW_W / LOGO_VIEW_H, LOGO_H)
}

// (pixel size) → GL texture, kept for the process like `icons`' cache: one entry per size, and
// the size is fixed, so it holds one. A `thread_local!` because the name belongs to the render
// thread's GL context.
thread_local! {
    static LOGO_TEX: RefCell<Vec<(i32, i32, c_uint)>> = const { RefCell::new(Vec::new()) };
}

fn logo_tex(w: i32, h: i32) -> c_uint {
    if let Some(tex) = LOGO_TEX.with(|c| c.borrow().iter().find(|e| e.0 == w && e.1 == h).map(|e| e.2)) {
        return tex;
    }
    // Straight alpha, colours kept: `icons`' mask path forces rgb to white, which would flatten
    // the logo's gradient.
    let tex = match plx_gfx::svg::rasterize(TMDB_LOGO_SVG, w, h) {
        Some(rgba) => icons::upload(w, h, &rgba),
        None => 0,
    };
    LOGO_TEX.with(|c| c.borrow_mut().push((w, h, tex)));
    tex
}

/// Draw the TMDB logo with its top-left at `(x, y)`; the rectangle it fills.
pub fn draw_tmdb_logo(p: Painter, x: f32, y: f32) -> Rect {
    let (w, h) = logo_size();
    let r = Rect::new(x, y, w, h);
    if p.is_recording() {
        return r;
    }
    let scale = plx_base::surface::render_scale();
    let tex = logo_tex(w.round() as i32 * scale, h.round() as i32 * scale);
    if tex != 0 {
        p.tex(tex, r, 0.0, theme::IMAGE_AS_AUTHORED);
    }
    r
}

/// The block's height at `width`: the logo, one SM gap and the notice.
pub fn credit_height(notice: &str, width: f32) -> f32 {
    LOGO_H + theme::space::SM + notice_view(notice).measure_h(width)
}

fn notice_view(notice: &str) -> TextView<'_> {
    TextView::new(notice, theme::size::CAPTION, theme::TEXT_SECONDARY)
        .leading(theme::size::CAPTION as f32 + theme::space::XS)
}

/// Draw the TMDB logo and `notice` under it, the block's bottom edge on `frame`'s bottom edge and
/// its left edge on `frame`'s. Returns the block's rectangle.
pub fn draw_tmdb_credit(p: Painter, frame: Rect, notice: &str) -> Rect {
    let h = credit_height(notice, frame.w);
    let top = frame.y + frame.h - h;
    let logo = draw_tmdb_logo(p, frame.x, top);
    notice_view(notice).draw(
        p,
        Rect::new(frame.x, logo.y + logo.h + theme::space::SM, frame.w, h - LOGO_H - theme::space::SM),
    );
    Rect::new(frame.x, top, frame.w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_the_official_file_unaltered() {
        let digest = plx_base::sha256::sha256(TMDB_LOGO_SVG.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, TMDB_LOGO_SHA256);
        assert!(TMDB_LOGO_SVG.contains(r#"viewBox="0 0 489.04 35.4""#));
    }

    #[test]
    fn the_logo_keeps_its_aspect_ratio_and_rasterises_in_colour() {
        let (w, h) = logo_size();
        assert!((w / h - LOGO_VIEW_W / LOGO_VIEW_H).abs() < 1e-4);
        let (pw, ph) = (w.round() as i32, h.round() as i32);
        let rgba = plx_gfx::svg::rasterize(TMDB_LOGO_SVG, pw, ph).expect("the official SVG parses");
        assert_eq!(rgba.len(), (pw * ph * 4) as usize);
        let ink: Vec<&[u8]> = rgba.chunks_exact(4).filter(|px| px[3] == 255).collect();
        assert!(!ink.is_empty(), "the wordmark has solid pixels");
        let (min_r, max_r) = ink.iter().fold((255u8, 0u8), |(lo, hi), px| (lo.min(px[0]), hi.max(px[0])));
        assert!(max_r - min_r > 40, "the green-to-blue gradient survives (red spans {min_r}..{max_r})");
        assert!(ink.iter().any(|px| px[2] > px[0]), "blue end of the gradient is present");
    }

    #[test]
    fn the_logo_is_far_smaller_than_the_screen_title() {
        let (w, h) = logo_size();
        assert!(h * 3.0 <= theme::size::HERO as f32, "{h} high against a {} title", theme::size::HERO);
        assert!(w < crate::route_screen::RouteLayout::screen().narrative.w);
    }
}
