//! Up Next occupies the transport row from the first Plex credits marker while a successor exists.
//! Next and Watch remain available in every mode; only Countdown starts the successor automatically.
//!
//! The data is `route::up_next()` — lifted from the `continuous=1` PlayQueue that every playback
//! already creates, so nothing here asks the server what plays next.
//!
//! It TAKES THE PLACE of the transport's Subtitles/Audio discs — the same control row the Skip
//! button uses, the same right edge — with the still and its caption stacked directly above. The
//! countdown is the primary button's own fill sweep (`Button::progress`), so the control and the
//! timer are one object rather than a button beside a rail.
//!
//! **Two buttons, and they are deliberately not equals** (the design's own note): *Next Episode*
//! takes the [`ctrl_slot`](crate::player_hud::classic::ctrl_slot) width — the floor that keeps the row
//! from visibly shrinking when it appears — while *Watch Credits* takes only its own label's, since
//! a second full-width capsule would read as a pair of equals when one continues watching and the
//! other does nothing at all. They are laid out in that spatial order, so LEFT reaches Watch
//! Credits and RIGHT returns.
//!
//! It is HUD furniture, not an overlay: playback keeps running underneath and the credits keep
//! rolling. That is what makes the countdown honest — `app.rs` holds the HUD up for as long as it
//! is armed, so the timer is never running behind a hidden transport.
//!
//! **The row's cursor is not kept here.** It is `hud_nav.btn`, exactly as it is for the discs, so
//! LEFT/RIGHT need no special case and [`ControlSlot::items`](crate::player_hud::ControlSlot)
//! answers for the clamp. This module owns the countdown and nothing else — which is also why the
//! cancel rule lives at `app.rs`'s one frame block rather than being spread across the key arms.
//!
//! *(A full-screen post-play CARD — show poster, DISPLAY title, countdown ring — was built for the
//! cross-show case and lives at commit `84ff9328`. It is not here because its trigger does not
//! exist: `continuous=1` is per-show, and a final episode returns `totalCount=1`, so the queue
//! never hands us a different show. Sourcing it from Continue Watching is the owner's call.)*
#![allow(dead_code)]
use plx_media::route::UpNext;
use plx_ui::hstack::{Align, HStack, Item};
use plx_ui::vstack::{Anchor, VStack};
use plx_ui::label::{HAlign, Label};
use plx_ui::theme;
use plx_ui::widgets::{draw_card, Button, ControlGround};
use plx_ui::{Env, Painter, Rect, View};
use std::ffi::CString;
use std::os::raw::c_int;


/// The row's two items, in the order they are DRAWN — so `hud_nav.btn` and the arrow keys agree
/// without a translation table (LEFT lands on 0 because 0 is the left one).
///
/// These are the two FIXED control ids of Up Next, `watch_credits` and `play_next`: ordinals 0 and
/// 1 of the row, so the engine addresses them as `control_elem(BTN_CREDITS)` and
/// `control_elem(BTN_NEXT)`. They carry no [`HudCommand`](crate::player_hud::command::HudCommand) and no package may rebind them, because
/// the countdown's meaning depends on which of the two is focused ([`countdown_may_run`]).
pub const BTN_CREDITS: c_int = 0;
pub const BTN_NEXT: c_int = 1;
/// Where the focus ring parks when the row appears: the PRIMARY, which here is the right-hand item
/// rather than the usual 0. `ControlSlot::primary_btn` is what reads this, so the appear edge in
/// `app.rs` stays one line for every occupant.
pub const PRIMARY_BTN: c_int = BTN_NEXT;

/// Countdown and cancellation choices for one playback.
#[derive(Default)]
pub struct Countdown {
    current: Option<crate::player_hud::ControlSlot>,
    timer: Option<(u32, u32)>, // deadline and configured duration
    intro_cancelled: bool,
    credits_cancelled: bool,
    first_credits: Option<plx_data::metadata::Marker>,
}

/// Whether this owns the control row this frame. The precedence itself lives in ONE place —
/// [`crate::player_hud::slot_for`] — so this is just a read of the resolved slot.
pub fn is_shown(slot: crate::player_hud::ControlSlot) -> bool {
    matches!(slot, crate::player_hud::ControlSlot::UpNext(_))
}


/// PURE — may the countdown keep running, the transport standing this way?
///
/// ONE rule for every way a user can take hold of the transport, and it is a STEADY STATE rather
/// than an edge on purpose: focus off the control row means they are driving something else, and
/// focus on *Watch Credits* means they have reached for the alternative. Either way, engaging the
/// transport is not consent to be pulled into the next episode. Arrowing back to *Next Episode*
/// does not re-arm — [`cancel`] latches — so a countdown, once refused, stays refused for the
/// episode.
///
/// `bare_transport` is the third way and the one that reads as an omission until it bites: an
/// OVERLAY — a track menu, the Info card, the Chapters strip — is the only way of taking hold
/// of the transport that never moves the focus ring. Worse,
/// [`crate::player_hud::classic::draw_hud`] draws the control row only for the BARE transport, so with
/// the Info card open the tile is not on screen at all. Without this term a countdown that was
/// already running when the card opened kept its clock behind a panel nobody could see it through
/// and cut to the next episode out of nowhere — the same failure `HudState::raise_for_offer`
/// exists to forbid, reached by the other door. A panel one presses OPEN is exactly as much
/// "engaging the transport" as an arrow key.
///
/// `play_next_focused` is whether the focused control is `play_next` (`btn == BTN_NEXT`): the
/// rule needs the identity of ONE control, not an index into a row.
pub fn countdown_may_run(bare_transport: bool, row_focused: bool, play_next_focused: bool) -> bool {
    bare_transport && row_focused && play_next_focused
}

impl Countdown {
    /// Update the visible slot. Synthetic continuity offers must pass `false`.
    pub fn tick(&mut self, slot: crate::player_hud::ControlSlot, now: u32, authoritative: bool) {
        self.tick_with(slot, now, authoritative, plx_media::route::intro_skip_delay(),
            plx_media::route::credits_skip_delay(), plx_media::route::next_episode_mode());
    }

    fn tick_with(&mut self, slot: crate::player_hud::ControlSlot, now: u32, authoritative: bool,
        intro: plx_plex::plex::session::AutoSkipDelay, credits: plx_plex::plex::session::CreditsCountdownDelay,
        next: plx_media::route::NextEpisodeMode) {
        use crate::player_hud::ControlSlot;
        use plx_data::metadata::MarkerKind;
        let (kind, marker, delay) = match slot {
            ControlSlot::Skip(p) => (Some(p.kind), Some(p.marker), match p.kind {
                MarkerKind::Intro => intro.milliseconds(),
                MarkerKind::Credits => None,
            }),
            ControlSlot::UpNext(m) => (Some(MarkerKind::Credits), Some(m),
                if next == plx_media::route::NextEpisodeMode::Countdown { Some(credits.milliseconds()) } else { None }),
            ControlSlot::Discs(_) => (None, None, None),
        };
        if authoritative && kind == Some(MarkerKind::Credits) && self.first_credits.is_none() {
            self.first_credits = marker;
        }
        let changed = self.current != Some(slot);
        self.current = kind.map(|_| slot);
        let cancelled = match kind {
            Some(MarkerKind::Intro) => self.intro_cancelled,
            Some(MarkerKind::Credits) => self.credits_cancelled,
            None => false,
        };
        let duration = if authoritative && !cancelled { delay.map(|ms| ms as u32) } else { None };
        match duration {
            None => self.timer = None,
            Some(ms) if changed || self.timer.map(|(_, duration)| duration) != Some(ms) => {
                self.timer = Some((now.wrapping_add(ms), ms));
            }
            Some(_) => {}
        }
    }

    /// The first real credits offer remains available after its marker has ended.
    pub fn credits_offer(&self, pos_ms: i64) -> Option<plx_data::metadata::Marker> {
        self.first_credits.filter(|marker| pos_ms >= marker.start_ms)
    }

    /// Refuse automatic action for this marker kind for the rest of the episode.
    pub fn cancel_kind(&mut self, kind: plx_data::metadata::MarkerKind) {
        use plx_data::metadata::MarkerKind;
        match kind {
            MarkerKind::Intro => self.intro_cancelled = true,
            MarkerKind::Credits => self.credits_cancelled = true,
        }
        if self.current.map(|slot| match slot {
            crate::player_hud::ControlSlot::Skip(p) => p.kind == kind,
            crate::player_hud::ControlSlot::UpNext(_) => kind == MarkerKind::Credits,
            crate::player_hud::ControlSlot::Discs(_) => false,
        }).unwrap_or(false) {
            self.timer = None;
        }
    }

    /// Refuse automatic action for the currently displayed marker kind.
    pub fn cancel(&mut self) {
        let kind = match self.current {
            Some(crate::player_hud::ControlSlot::Skip(p)) => Some(p.kind),
            Some(crate::player_hud::ControlSlot::UpNext(_)) => Some(plx_data::metadata::MarkerKind::Credits),
            _ => None,
        };
        if let Some(kind) = kind { self.cancel_kind(kind); }
    }

    pub fn armed(&self) -> bool { self.timer.is_some() }

    fn remaining_ms(&self, now: u32) -> u32 {
        let Some((deadline, duration)) = self.timer else { return 0; };
        if let Some(held) = plx_machine::motion::held_clock_ms() {
            return duration.saturating_sub(held).max(1);
        }
        let left = deadline.wrapping_sub(now);
        if left == 0 || left >= 0x8000_0000 { 0 } else { left.min(duration) }
    }

    /// Elapsed fraction of the configured delay, or None when disarmed.
    pub fn progress(&self, now: u32) -> Option<f32> {
        self.timer.map(|(_, duration)| 1.0 - self.remaining_ms(now) as f32 / duration as f32)
    }

    pub fn expired(&self, now: u32) -> bool {
        self.armed() && self.remaining_ms(now) == 0
    }

    /// An expiry belongs only to the exact marker that armed it.
    pub fn expired_for(&self, slot: crate::player_hud::ControlSlot, now: u32) -> bool {
        self.current == Some(slot) && self.expired(now)
    }

    /// A remote action in the expiration frame wins over automatic playback.
    pub fn cancel_expired_for_input(&mut self, slot: crate::player_hud::ControlSlot, now: u32, pending: bool) {
        if pending && self.expired_for(slot, now) { self.cancel(); }
    }

    /// Clone before the playback queue is replaced.
    pub fn take(&mut self, ps: &plx_media::route::PlaybackSession) -> Option<UpNext> {
        self.cancel_kind(plx_data::metadata::MarkerKind::Credits);
        plx_media::route::up_next(ps).cloned()
    }

    /// Start a new episode without carrying its predecessor's choices.
    pub fn reset(&mut self) { *self = Self::default(); }
}

// ---- geometry: the control row's slot, with the next episode's still stacked above it. The still
// takes the PRIMARY button's width and right edge, so the group is one clean two-edge column — an
// earlier version gave the still its own width and drew it permanently scaled, which put its right
// edge 8px past the pill's and its left 15px inside, a near-miss that reads as unfinished from the
// couch. ----

/// still → caption: the caption is the still's label, so it hugs it…
const CAP_GAP: f32 = theme::space::XS;
/// …and the buttons are separate actionable objects, so they get more air. Unequal gaps are what
/// create the grouping; an earlier 8/8 left the caption belonging to neither.
const BTN_GAP: f32 = theme::space::MD;
/// between the two buttons — the design's `--space-sm`, tighter than the gap to the caption above
/// so the pair reads as one row before it reads as two controls
const PILL_GAP: f32 = theme::space::SM;
const CAPTION_H: f32 = 30.0;




/// The caption is RIGHT-ALIGNED text, so it may run wider than the still/button column without
/// breaking it — the column's edges are the two solid rectangles, and a text run has no left edge
/// to break. It gets its own budget because clamping it to the button's width elided the episode
/// NAME, which is the one thing the caption is for.
const CAPTION_W: f32 = 460.0;

/// The tile's four frames, in draw order up the column.
pub struct Layout {
    pub still: Rect,
    pub caption: Rect,
    pub credits: Rect,
    pub next: Rect,
}

/// PURE — the whole column from the two MEASURED pill widths, so the draw and the pointer hit-test
/// are one arrangement and the arrangement is host-testable. Measuring is the impure half and lives
/// in [`layout`]: `text_width` is `TTF_SizeUTF8`, which the host suite cannot even link against.
pub fn layout_of(next_w: f32, credits_w: f32) -> Layout {
    // Where the row hangs, as the layout reports it: the stand-in lands on exactly these.
    layout_at(crate::player_hud::classic::frame_geometry().row_anchor, next_w, credits_w)
}

/// [`layout_of`] over any row anchor: the column from the two pill widths, right-anchored at
/// `anchor.right` on the row `anchor.top` / `anchor.h` describe. PURE. The layout compiler
/// (`hud_format::compile`) calls it with the anchor of the segments its package's Up Next replaces,
/// so a package's offer and the classic one are the one arrangement.
pub fn layout_at(anchor: crate::player_hud::geometry::RowAnchor, next_w: f32, credits_w: f32) -> Layout {
    use crate::player_hud::geometry::RowAnchor;
    let RowAnchor { right: ctrl_right, top: ctrl_y, h: ctrl_h } = anchor;
    // The two pills are one run, right-anchored at the control row's edge, `PILL_GAP` apart.
    let pills = [Item::new(credits_w), Item::new(next_w)];
    let row = HStack::new(&pills, PILL_GAP, Align::Trailing(ctrl_right));
    let (credits_x, _) = row.span(0).unwrap_or_default();
    let (next_x, _) = row.span(1).unwrap_or_default();
    let next = Rect::new(next_x, ctrl_y, next_w, ctrl_h);
    let credits = Rect::new(credits_x, next.y, credits_w, next.h);
    // The column, bottom-up from the control row: still, caption, row. The unequal gaps are what
    // group them (the caption hugs the still; the buttons get more air), carried as the caption's
    // own `after`. The still's height is its 16:9 frame at the primary's width.
    let still_h = (next_w * 9.0 / 16.0).round();
    let column = [
        Item::new(still_h),
        Item::new(CAPTION_H).after(BTN_GAP),
        Item::new(ctrl_h),
    ];
    let col = VStack::new(&column, CAP_GAP, Anchor::Bottom(ctrl_y + ctrl_h));
    let (still_y, _) = col.span(0).unwrap_or_default();
    let (caption_y, _) = col.span(1).unwrap_or_default();
    let caption = Rect::new(ctrl_right - CAPTION_W, caption_y, CAPTION_W, CAPTION_H);
    let still = Rect::new(ctrl_right - next_w, still_y, next_w, still_h);
    Layout {
        still,
        caption,
        credits,
        next,
    }
}

/// [`layout_of`] over the two measured widths. The primary takes the same control-row slot the
/// Subtitles/Audio pair occupies — never narrower than the pair it replaces, so the row does not
/// visibly shrink when it appears — while *Watch Credits* takes only its own label's width:
/// deliberately NOT a second `ctrl_slot`, because that floor exists to hold the row's right edge
/// steady, which is the primary's job, and two equal capsules would say the two choices are
/// equivalent.
pub fn layout(row: &mut crate::player_hud::motion::HudMotion, measure: &dyn plx_machine::machine::Measure) -> Layout {
    layout_of(
        crate::player_hud::classic::ctrl_slot(row, plx_platform::i18n::msg::widgets_next_episode(), measure).w,
        plx_ui::widgets::Button::pill_w_measured(plx_platform::i18n::msg::widgets_next_credits_c(), theme::size::BODY, false, None, measure),
    )
}

/// [`layout`] without writing the row's width memo — what `ControlSlot::item_rect` places the two
/// buttons at. Only the two BUTTONS are pointer targets: the still and its caption are not, since
/// with two actions in the row a click on the artwork has no single obvious meaning, and guessing
/// one is how a stray click starts an episode the user did not ask for.
pub fn layout_peek(row: &crate::player_hud::motion::HudMotion, measure: &dyn plx_machine::machine::Measure) -> Layout {
    layout_of(
        crate::player_hud::classic::ctrl_slot_w(row, plx_platform::i18n::msg::widgets_next_episode(), measure),
        plx_ui::widgets::Button::pill_w_measured(plx_platform::i18n::msg::widgets_next_credits_c(), theme::size::BODY, false, None, measure),
    )
}

/// The caption — `"Up Next · S2, E4 · Laura"`. PRIMARY and bold, not secondary: it sits at y≈760
/// where the HUD scrim is only ~0.28, so over a bright end card a secondary grey measures ~1.3:1 —
/// invisible. It is also the only thing distinguishing this episode's ID from the now-playing one
/// on the same band, hence the explicit "Up Next ·" kicker rather than a bare "S2, E4".
fn caption(u: &UpNext) -> String {
    let episode = if u.season > 0 || u.index > 0 {
        plx_ui::fmt::episode_kicker(u.season, u.index, &u.ep_title)
    } else {
        u.ep_title.clone()
    };
    plx_platform::i18n::msg::widgets_next_caption(&episode)
}

pub fn draw(
    ps: &plx_media::route::PlaybackSession,
    row: &mut crate::player_hud::motion::HudMotion,
    up: &Countdown,
    p: Painter,
    focused: bool,
    btn: c_int,
    now: u32,
    measure: &dyn plx_machine::machine::Measure,
) {
    let Some(u) = plx_media::route::up_next(ps) else {
        return;
    };
    let l = layout(row, measure);
    let (pop_credits, pop_next) = (row.scale(BTN_CREDITS), row.scale(BTN_NEXT));
    // NOT scaled: `CARD_FOCUS_SCALE` is the terminal value of a focus spring the shelves drive, and
    // passing it as a constant drew the still permanently 7% oversized — overhanging the column on
    // both sides. The still is decoration on a focusable row, not a focusable tile itself, so it
    // keeps the card treatment (sheen + resting shadow) at rest scale.
    draw_card(
        p,
        l.still,
        plx_media::route::item_sid(plx_media::route::cur_sid(ps)).raw(),
        &u.thumb,
        (480, 270),
        10.0,
        false,
        1.0,
    );

    let elided = plx_gfx::text::elide_by(&caption(u), l.caption.w, false, |t| {
        measure.width_str(t, theme::size::CAPTION, true)
    });
    if let Ok(cs) = CString::new(elided) {
        Label::new(cs.as_ptr(), theme::size::CAPTION, theme::TEXT_PRIMARY)
            .bold()
            .h(HAlign::Right)
            .draw(p, l.caption);
    }

    let e = Env::inert();
    // The focus pop is the CONTROL ROW's, not this card's: these two stand in the transport's own
    // slot and share its cursor, so they share its springs (`HudMotion::scale`).
    Button::new(plx_platform::i18n::msg::widgets_next_credits_c().as_ptr(), theme::size::BODY, l.credits)
        .focused(focused && btn == BTN_CREDITS)
        // Both buttons on this card stand on LIVE CREDITS — the video plane, under this card's own
        // scrim — so both take the unkeyed ground (`ControlGround`). It is what the app's
        // `ControlStyle::Keyline` was hand-rolling for exactly this surface, and the right way
        // round: a light film over the scrim rather than a darker plate cut into the picture.
        .ground(ControlGround::Unkeyed)
        .scale(pop_credits)
        .draw(&e, p);

    // The primary IS the countdown — its fill sweeps left→right and, at the far edge, the episode
    // starts. Driven straight off the remaining MILLISECONDS and redrawn every frame, so the sweep
    // is continuous; the label carries no seconds, because the pill's width is derived from its
    // label and a ticking numeral would resize the button and slide its centred text every second.
    let Ok(label) = CString::new(plx_platform::i18n::msg::widgets_next_episode()) else {
        return;
    };
    let mut b = Button::new(label.as_ptr(), theme::size::BODY, l.next)
        .focused(focused && btn == BTN_NEXT)
        .ground(ControlGround::Unkeyed)
        .scale(pop_next);
    if up.armed() {
        // It animates from a CLOCK, so `plx_machine::idle`'s spring instrumentation cannot see it — the trap
        // `Xfade::tick` and `Spinner::draw` both shipped frozen in. The player route bypasses the
        // frame gate outright today, so this changes nothing now; it is what keeps that reversible.
        // A HELD clock (`remaining_ms`) draws one fixed fill, so there is nothing to redraw — the
        // same exception `widgets::Spinner` makes.
        if !plx_machine::motion::phase_clocks_held() {
            plx_machine::idle::invalidate();
        }
        b = b.progress(up.progress(now).unwrap_or(0.0));
    }
    b.draw(&e, p);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::player_hud::{ControlSlot, Controls};
    use plx_data::metadata::{Marker, MarkerKind};
    use plx_plex::plex::session::{AutoSkipDelay as IntroDelay, CreditsCountdownDelay as CreditsDelay};
    use plx_media::route::NextEpisodeMode as Mode;

    fn marker(kind: MarkerKind, start_ms: i64, final_seg: bool) -> Marker {
        Marker { kind, start_ms, end_ms: start_ms + 1_000, final_seg }
    }

    fn skip(kind: MarkerKind, start: i64, final_seg: bool) -> ControlSlot {
        ControlSlot::Skip(crate::skip_pill::prompt_for(marker(kind, start, final_seg)))
    }

    fn tick(c: &mut Countdown, slot: ControlSlot, now: u32, authoritative: bool,
        intro: IntroDelay, credits: CreditsDelay, mode: Mode) {
        c.tick_with(slot, now, authoritative, intro, credits, mode);
    }

    #[test]
    fn expired_timer_cannot_activate_a_different_marker() {
        let mut c = Countdown::default();
        let intro = skip(MarkerKind::Intro, 100, false);
        let credits = ControlSlot::UpNext(marker(MarkerKind::Credits, 900, true));
        tick(&mut c, intro, 10, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(c.expired_for(intro, 3010));
        assert!(!c.expired_for(credits, 3010));
        c.cancel_expired_for_input(intro, 3010, true);
        assert!(!c.expired_for(intro, 3010));
        tick(&mut c, intro, 3011, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.armed(), "remote input must not re-arm on the next frame");
    }

    #[test]
    fn intro_delays_and_progress_follow_each_setting_and_wrap() {
        for delay in IntroDelay::LADDER {
            let mut c = Countdown::default();
            let start = u32::MAX - 1_000;
            let slot = skip(MarkerKind::Intro, 100, false);
            tick(&mut c, slot, start, true, delay, CreditsDelay::default(), Mode::Countdown);
            if let Some(ms) = delay.milliseconds() {
                let ms = ms as u32;
                assert_eq!(c.progress(start), Some(0.0));
                assert_eq!(c.progress(start.wrapping_add(ms / 2)), Some(0.5));
                tick(&mut c, slot, start.wrapping_add(1_000), true, delay, CreditsDelay::default(), Mode::Countdown);
                assert!(!c.expired(start.wrapping_add(ms - 1)));
                assert!(c.expired(start.wrapping_add(ms)));
                assert_eq!(c.progress(start.wrapping_add(ms)), Some(1.0));
            } else {
                assert!(!c.armed());
                assert_eq!(c.progress(start), None);
            }
        }
    }

    #[test]
    fn credits_only_auto_advance_a_real_offer_with_a_successor_in_countdown_mode() {
        let m = marker(MarkerKind::Credits, 10_000, true);
        for mode in [Mode::Countdown, Mode::AfterCredits, Mode::Off] {
            for authoritative in [false, true] {
                let mut c = Countdown::default();
                tick(&mut c, ControlSlot::UpNext(m), 100, authoritative, IntroDelay::Manual, CreditsDelay::default(), mode);
                assert_eq!(c.armed(), authoritative && mode == Mode::Countdown);
                if c.armed() {
                    assert!(!c.expired(5_099));
                    assert!(c.expired(5_100));
                }
            }
        }
        for final_seg in [false, true] {
            let mut c = Countdown::default();
            tick(&mut c, skip(MarkerKind::Credits, 10_000, final_seg), 100, true,
                IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
            assert!(!c.armed());
        }
    }

    #[test]
    fn credits_delay_controls_timing_and_setting_changes_restart_the_timer() {
        let slot = ControlSlot::UpNext(marker(MarkerKind::Credits, 10_000, false));
        for delay in CreditsDelay::LADDER {
            let mut c = Countdown::default();
            let ms = delay.milliseconds() as u32;
            tick(&mut c, slot, 0, true, IntroDelay::Manual, delay, Mode::Countdown);
            assert_eq!(c.progress(ms / 2), Some(0.5));
            assert!(!c.expired(ms - 1));
            assert!(c.expired(ms));
        }
        let mut c = Countdown::default();
        tick(&mut c, slot, 0, true, IntroDelay::Manual, CreditsDelay::Seconds3, Mode::Countdown);
        tick(&mut c, slot, 2_000, true, IntroDelay::Manual, CreditsDelay::Seconds10, Mode::Countdown);
        assert_eq!(c.progress(2_000), Some(0.0));
        assert!(c.expired(12_000));
        tick(&mut c, slot, 13_000, false, IntroDelay::Manual, CreditsDelay::Seconds10, Mode::Countdown);
        assert!(!c.armed());
        tick(&mut c, slot, 14_000, true, IntroDelay::Manual, CreditsDelay::Seconds10, Mode::Countdown);
        assert_eq!(c.progress(14_000), Some(0.0));
    }

    #[test]
    fn disappearance_restarts_a_timer_but_cancellation_survives_seeks_and_new_markers() {
        let mut c = Countdown::default();
        let intro = skip(MarkerKind::Intro, 100, false);
        let discs = ControlSlot::Discs(Controls::NONE);
        let credit = ControlSlot::UpNext(marker(MarkerKind::Credits, 10_000, true));
        tick(&mut c, intro, 0, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        tick(&mut c, discs, 2_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.armed());
        tick(&mut c, intro, 4_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.expired(6_999));
        assert!(c.expired(7_000));
        c.cancel();
        tick(&mut c, discs, 8_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        tick(&mut c, skip(MarkerKind::Intro, 200, false), 9_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.armed());
        tick(&mut c, credit, 10_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(c.armed());
        c.cancel_kind(MarkerKind::Credits);
        tick(&mut c, discs, 11_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        tick(&mut c, ControlSlot::UpNext(marker(MarkerKind::Credits, 20_000, true)), 12_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.armed());
        c.reset();
        tick(&mut c, intro, 13_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(c.armed());
        tick(&mut c, credit, 14_000, true, IntroDelay::Seconds3, CreditsDelay::default(), Mode::Countdown);
        assert!(c.armed());
    }

    #[test]
    fn credits_continuity_is_authoritative_episode_scoped_and_never_arms_on_its_own() {
        let mut c = Countdown::default();
        let first = marker(MarkerKind::Credits, 10_000, false);
        let later = marker(MarkerKind::Credits, 20_000, true);
        tick(&mut c, ControlSlot::UpNext(first), 0, false, IntroDelay::Manual, CreditsDelay::default(), Mode::Countdown);
        assert_eq!(c.credits_offer(15_000), None);
        tick(&mut c, skip(MarkerKind::Credits, 10_000, false), 1, true, IntroDelay::Manual, CreditsDelay::default(), Mode::Off);
        c.cancel_kind(MarkerKind::Credits);
        tick(&mut c, ControlSlot::Discs(Controls::NONE), 2, true, IntroDelay::Manual, CreditsDelay::default(), Mode::Off);
        assert_eq!(c.credits_offer(9_999), None);
        assert_eq!(c.credits_offer(10_000), Some(first));
        tick(&mut c, ControlSlot::UpNext(later), 3, true, IntroDelay::Manual, CreditsDelay::default(), Mode::Countdown);
        assert!(!c.armed());
        assert_eq!(c.credits_offer(30_000), Some(first));
        c.reset();
        assert_eq!(c.credits_offer(30_000), None);
    }


    /// The one cancel rule, exhaustively. It is a STEADY STATE, so it has to answer for every
    /// (row focused × which button) pair rather than for a transition: the countdown survives only
    /// while the ring is resting on the primary, which is where the appear edge parks it.
    #[test]
    fn the_countdown_runs_only_while_the_ring_rests_on_the_primary() {
        assert!(
            countdown_may_run(true, true, true),
            "the resting state the appear edge creates"
        );
        assert!(
            !countdown_may_run(true, true, false),
            "reaching for Watch Credits is reaching for the alternative"
        );
        for b in [BTN_CREDITS, BTN_NEXT] {
            assert!(
                !countdown_may_run(true, false, b == BTN_NEXT),
                "focus off the row is the user driving the transport, whatever btn still says"
            );
        }
    }

    /// The overlay term, which no focus state can stand in for: an open panel never moves the ring,
    /// and the Info card and Chapters strip do not draw the control row at all — so the ONE state
    /// that looks most like consent (ring at rest on the primary) is exactly the one that would run
    /// a countdown nobody can see. Every combination is refused while a panel is up.
    #[test]
    fn an_open_panel_refuses_the_countdown_however_the_ring_rests_under_it() {
        for b in [BTN_CREDITS, BTN_NEXT] {
            for focused in [false, true] {
                assert!(
                    !countdown_may_run(false, focused, b == BTN_NEXT),
                    "a tile behind a panel counts down where it cannot be seen or refused"
                );
            }
        }
    }

    /// The ring parks on the RIGHT item, which is the one place in the app where a row's primary is
    /// not index 0 — a Skip pill's only item is, and so is the first disc. If this ever silently
    /// became 0 the countdown would arm and be cancelled by its own resting state on the very next
    /// frame, and the tile would look like a timer that never runs.
    #[test]
    fn the_primary_is_the_right_hand_item_and_the_countdown_survives_it() {
        assert_eq!(PRIMARY_BTN, BTN_NEXT);
        assert!(countdown_may_run(true, true, PRIMARY_BTN == BTN_NEXT));
    }

    /// Geometry, from the design's own column: the two pills share a baseline and the control row's
    /// right edge, Watch Credits sits `space::SM` to the LEFT of the primary (so LEFT reaches it),
    /// and the still shares the primary's width and edge in a 16:9 frame. Driven through the PURE
    /// half with the two widths supplied, because the impure one measures with `TTF_SizeUTF8` and
    /// the host suite cannot link SDL2_ttf at all.
    #[test]
    fn the_column_is_right_anchored_on_one_edge() {
        // the real orders of magnitude: `ctrl_slot`'s floor is 232, and "Watch Credits" measures a
        // little under it — which is the asymmetry the design is making a point of
        let l = layout_of(243.0, 226.0);
        let (n, c, t, cap) = (l.next, l.credits, l.still, l.caption);
        let right = crate::player_hud::classic::frame_geometry().row_anchor.right;
        assert_eq!(n.x + n.w, right, "the primary holds the row's right edge");
        assert_eq!(t.x + t.w, right, "…and the still shares it");
        assert_eq!(
            cap.x + cap.w,
            right,
            "…and the caption's right-aligned run is measured from it"
        );
        assert_eq!(t.w, n.w, "one column, one pair of edges");
        assert_eq!(
            t.h,
            (n.w * 9.0 / 16.0).round(),
            "16:9 — it is a video frame"
        );
        assert!(
            c.x + c.w < n.x,
            "Watch Credits is drawn LEFT of the primary, so LEFT reaches it"
        );
        assert_eq!(n.x - (c.x + c.w), PILL_GAP);
        assert_eq!(c.y, n.y, "the pair shares a baseline");
        assert_eq!(c.h, n.h);
        // stacked bottom-up with the unequal gaps that do the grouping
        assert_eq!(cap.y + cap.h + BTN_GAP, n.y);
        assert_eq!(t.y + t.h + CAP_GAP, cap.y);
        assert!(
            CAP_GAP < BTN_GAP,
            "the caption belongs to the still, not to the buttons"
        );
        // The whole tile sits ABOVE the control row's own band and inside the frame — it is the
        // tallest thing the transport ever puts there, and the still is what could run off the top.
        assert!(t.y > 0.0, "the column fits on the panel");
        assert!(c.x > 0.0, "…and does not run off its left edge");
    }

    /// The caption names the successor and says so: the kicker survives, the address comes from the
    /// shared `fmt::episode_kicker` (so the HUD, the pre-roll ctx line and this cannot drift), and an
    /// unaddressed leaf degrades to the bare title instead of drawing "S0, E0".
    #[test]
    fn the_caption_is_the_shared_episode_vocabulary_behind_an_up_next_kicker() {
        let ep = UpNext {
            season: 2,
            index: 4,
            ep_title: "Laura".into(),
            ..Default::default()
        };
        assert_eq!(caption(&ep), "Up Next \u{b7} S2, E4 \u{b7} Laura");
        let bare = UpNext {
            ep_title: "Laura".into(),
            ..Default::default()
        };
        assert_eq!(
            caption(&bare),
            "Up Next \u{b7} Laura",
            "no address, no empty ordinal"
        );
    }
}
