use super::{facts, Ready, Sent};
use plx_machine::machine::Measure;
use plx_platform::i18n::msg;
use plx_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_W};
use plx_ui::text_view::TextView;
use plx_ui::theme::{self, hero, size, space};
use plx_ui::widgets::{self, Art, Button};
use plx_ui::{Crop, Env, Painter, Rect, View};

pub(super) const HERO_H: f32 = 760.0;
const ACTION_H: f32 = space::XL;
const ACTION_Y: f32 = HERO_H - space::LG - ACTION_H;
const FACT_W: f32 = SCR_W - 2.0 * MARGIN_X - 3.0 * space::LG - hero::TEXT_W;
const ABOUT_LEAD: f32 = plx_ui::HERO_SYN_LEAD;
const ABOUT_HEAD: f32 = space::XL;

pub(super) fn pills<'a>(actions: &'a [facts::Action], top: f32, measure: &'a dyn Measure) -> impl Iterator<Item = (&'a facts::Action, Rect)> {
    let mut x = MARGIN_X;
    actions.iter().map(move |action| {
        let w = Button::pill_w_measured(action.label, size::BODY, true, None, measure).max(hero::PLAY_MIN_W);
        let r = Rect::new(x, top + ACTION_Y, w, ACTION_H);
        x += w + space::SM;
        (action, r)
    })
}

fn line(p: Painter, text: &str, sz: i32, ink: [f32; 4], r: Rect, measure: &dyn Measure) {
    if !text.is_empty() {
        TextView::new(text, sz, ink).max_lines(1).with_measure(measure).draw(p, r);
    }
}

pub(super) fn hero(p: Painter, top: f32, ready: &Ready, hot: Option<u32>, sent: Sent, trailer_failed: bool,
    progress: Option<&super::super::progress_text::Lines>, measure: &dyn Measure) {
    let page = &ready.page;
    let title = &page.detail.title;
    let info = &page.detail.info;
    if let Some(sid) = page.art {
        if !ready.backdrop.is_empty() {
            let r = Rect::new(0.0, top, SCR_W, HERO_H);
            let (tex, w, h) = widgets::resolve_tex_wh_on(sid.raw(), &ready.backdrop, 1280, 720, 0);
            if tex != 0 {
                p.tex_uv(tex, r.cover_uv(w, h, Crop::Centre), r, 0.0, theme::TINT_WHITE);
                let ink = theme::scrim(theme::SCRIM_TEXT_A);
                let clear = theme::scrim(0.0);
                p.grad4(r, [ink, clear, clear, ink]);
                let fade = Rect::new(0.0, top + HERO_H * 0.42, SCR_W, HERO_H * 0.58);
                let ground = theme::SURFACE_APP;
                let clear = theme::with_a(ground, 0.0);
                p.grad4(fade, [clear, clear, ground, ground]);
            }
        }
        if !ready.poster.is_empty() {
            let r = Rect::new(SCR_W - MARGIN_X - CARD_W, top + ACTION_Y - CARD_H, CARD_W, CARD_H);
            widgets::card(p, r, Art::Thumb { sid: sid.raw(), key: &ready.poster, res: widgets::POSTER_RES },
                theme::CARD_RING_RAD, false, 1.0, 0.0);
        }
    }

    let actions = &ready.actions;
    for (action, r) in pills(actions, top, measure) {
        if action.focusable() {
            Button::new(action.label.as_ptr(), size::BODY, r).icon(action.icon)
                .enabled(action.enabled).focused(action.act.elem().is_some_and(|elem| hot == Some(elem)))
                .draw(&Env::inert(), p);
        } else {
            if hot == Some(super::HERO_READ) {
                let ink = theme::OVERLAY_FOCUS_PILL;
                p.rect(r, ACTION_H * 0.5, ink, ink, 0.0);
            }
            widgets::badge(p, r.x + space::SM, r.cy(), action.word(), Some(action.icon), widgets::BadgeStyle::OverArt, measure);
        }
    }
    let mut y = top + ACTION_Y - space::MD;
    let note = if trailer_failed { Some(msg::requests_trailer_failed()) } else { match sent {
        Sent::Sending => Some(msg::requests_sending()),
        Sent::Done if progress.is_none() => Some(msg::requests_sent()),
        Sent::Failed(e) => Some(crate::registry::seerr_error_text(e)),
        _ => None,
    }};
    for note in [note, progress.and_then(|l| l.note.as_deref()), progress.and_then(|l| l.detail.as_deref()), progress.map(|l| l.headline.as_str())].into_iter().flatten() {
        y -= ABOUT_LEAD;
        line(p, note, size::CAPTION, theme::TEXT_PRIMARY, Rect::new(MARGIN_X, y, hero::TEXT_W, ABOUT_LEAD), measure);
    }
    if !title.overview.trim().is_empty() {
        bottom_text(p, &mut y, preview(&title.overview, measure));
    }
    if !title.year.is_empty() || !ready.length.is_empty() || !ready.stage.is_empty() {
        y -= hero::FACTS_H + space::SM;
        widgets::hero_facts(p, Rect::new(MARGIN_X, y, hero::TEXT_W, hero::FACTS_H), &title.year,
            &ready.length, false, ready.stage, measure);
    }
    if !info.tagline.is_empty() {
        bottom_text(p, &mut y, TextView::new(&info.tagline, size::LABEL, theme::TEXT_SECONDARY)
            .max_lines(1).leading(ABOUT_LEAD).with_measure(measure));
    }
    bottom_text(p, &mut y, TextView::new(&title.name, size::DISPLAY, theme::TEXT_PRIMARY).bold()
        .leading(hero::TITLE_LEAD).max_lines(hero::TITLE_LINES).with_measure(measure));
    let chips = [info.certification.as_str()];
    let chips = if chips[0].is_empty() { &[][..] } else { &chips[..] };
    let identity = facts::identity(page);
    if identity.iter().any(|part| !part.is_empty()) || !chips.is_empty() {
        y -= widgets::BADGE_H + space::XS;
        widgets::hero_identity(p, Rect::new(MARGIN_X, y, hero::TEXT_W, widgets::BADGE_H), &identity, None, chips, measure);
    }
}

fn bottom_text(p: Painter, y: &mut f32, text: TextView<'_>) {
    let h = text.measure_h(hero::TEXT_W);
    *y -= h + space::XS;
    text.draw(p, Rect::new(MARGIN_X, *y, hero::TEXT_W, h));
}

pub(super) fn preview<'a>(text: &'a str, measure: &'a dyn Measure) -> TextView<'a> {
    plx_ui::hero_synopsis(text.trim(), "").break_long_words().with_measure(measure)
}

fn synopsis<'a>(text: &'a str, measure: &'a dyn Measure) -> TextView<'a> {
    TextView::new(text, size::LABEL, theme::TEXT_READING).leading(ABOUT_LEAD).break_long_words().with_measure(measure)
}

fn about_w(ready: &Ready) -> f32 {
    if ready.facts.is_empty() { SCR_W - 2.0 * MARGIN_X - 2.0 * space::LG } else { hero::TEXT_W }
}

pub(super) fn about_h(ready: &Ready, measure: &dyn Measure) -> f32 {
    let syn_h = ready.about_overview().map_or(0.0, |text| synopsis(text, measure).measure_h(about_w(ready)));
    ABOUT_HEAD + syn_h.max(ABOUT_LEAD * ready.facts.len() as f32) + 2.0 * space::LG
}

pub(super) fn about_rect(r: Rect) -> Rect {
    Rect::new(MARGIN_X, r.y, SCR_W - 2.0 * MARGIN_X, r.h - space::MD)
}

pub(super) fn about(p: Painter, r: Rect, ready: &Ready, hot: bool, measure: &dyn Measure) {
    let plate = about_rect(r);
    if hot {
        let ink = theme::OVERLAY_FOCUS_PILL;
        p.rect(plate, theme::CARD_RING_RAD, ink, ink, 0.0);
    }
    let x = plate.x + space::LG;
    let y = plate.y + space::MD;
    let text = ready.about_overview();
    plx_ui::cards::draw_heading_c(p.translate(0.0, y), msg::browse_detail_about_c(), c"", c"", x, 0.0, 1.0, measure);
    if let Some(text) = text {
        synopsis(text, measure).draw(p, Rect::new(x, y + ABOUT_HEAD, about_w(ready), plate.h - ABOUT_HEAD));
    }
    let fact_x = if text.is_some() { x + hero::TEXT_W + space::LG } else { x };
    let label_w = FACT_W * 0.38;
    for (i, fact) in ready.facts.iter().enumerate() {
        let fy = y + ABOUT_HEAD + i as f32 * ABOUT_LEAD;
        line(p, fact.label, size::CAPTION, theme::TEXT_SECONDARY, Rect::new(fact_x, fy, label_w - space::SM, ABOUT_LEAD), measure);
        line(p, &fact.value, size::LABEL, theme::TEXT_PRIMARY, Rect::new(fact_x + label_w, fy, FACT_W - label_w, ABOUT_LEAD), measure);
    }
}
