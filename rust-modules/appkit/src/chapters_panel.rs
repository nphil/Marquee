//! The playing item's chapters, as the HUD's Chapters section reads them. The strip itself is a
//! `cards::Shelf` the section's compiled state places (`player_hud::strip`); this module is the ONE read
//! of the chapter list and the one number the strip's drawing shares with it.
//!
//! Data comes from the PLAYING leaf (`metadata::playing_chapters`, loaded with `?includeChapters=1`
//! on the same fetch the track store already makes), never from `metadata::current()` — the same
//! identity rule `appkit/track_menu.rs` and `appkit/skip_pill.rs` state. Reading `current()` is what made
//! the Chapters tab vanish for every episode started from a show detail page: `current()` is then
//! the SHOW, and a show container carries no `Chapter[]`.
use plx_data::metadata;

/// Focus rim on the selected chapter. The card family's resting sheen ([`plx_ui::theme::CARD_SHEEN`] .22 /
/// 1px) washes out over the hardware video plane the same way an unkeyed control's edge did — pure
/// white and a thicker stroke are what keep "which chapter" readable from the couch while scrolling.
/// Colour is the unkeyed focus edge ([`plx_ui::theme::CONTROL_RIM_FOCUS_UNKEYED`]); width is deliberately a
/// step over that control's 1.25 so a 288-wide still reads as selected, not merely edged.
pub(crate) const CH_FOCUS_RING_W: f32 = 2.5;

/// the playing leaf's chapters — the ONE read, so within a frame the count, the open, the seek and
/// the draw cannot end up describing different items. ACROSS frames the store can still be replaced
/// (a new play retires it, `route::request_play`).
pub(crate) fn chapters<'a>(meta: metadata::MetadataView<'a>) -> &'a [metadata::Chapter] {
    meta.playing_chapters()
}

/// whether the PLAYING item has chapters — drives showing/hiding the Chapters tab
pub fn has_chapters(meta: metadata::MetadataView<'_>) -> bool {
    !chapters(meta).is_empty()
}

/// A target this close short of a chapter's start counts as inside it: a seek lands on a keyframe a
/// hair before the start it was sent to.
const AT_TOLERANCE_NS: i64 = 250_000_000;
/// DOWN on a target this far into its chapter goes back to that chapter's start; nearer than this it
/// goes on to the previous chapter's, as a previous-track key does.
const BACK_RESTART_NS: i64 = 3_000_000_000;
/// The furthest a step is pulled onto a chapter start: 45 % of the step, never more than this.
const SNAP_CAP_NS: i64 = 8_000_000_000;

fn start_ns(c: &metadata::Chapter) -> i64 {
    c.start_ms.saturating_mul(1_000_000)
}

/// The playing item's chapters as the seek gestures use them: none unless there are two, since a
/// single chapter is the whole item and a jump would go nowhere.
pub fn seek_chapters<'a>(meta: metadata::MetadataView<'a>) -> &'a [metadata::Chapter] {
    let all = chapters(meta);
    if all.len() >= 2 { all } else { &[] }
}

/// The chapter `t_ns` falls in: the last one started by then.
pub fn chapter_at(chapters: &[metadata::Chapter], t_ns: i64) -> Option<usize> {
    chapters.iter().rposition(|c| start_ns(c) <= t_ns.saturating_add(AT_TOLERANCE_NS))
}

/// UP (`forward`) or DOWN on a seek target pending at `base_ns`: the next chapter's start; going back,
/// the start of the chapter the target is in, or the previous chapter's when the target is within
/// [`BACK_RESTART_NS`] of its own. `None` when there is no chapter that way.
pub fn chapter_jump(chapters: &[metadata::Chapter], base_ns: i64, forward: bool) -> Option<i64> {
    let at = chapter_at(chapters, base_ns);
    if forward {
        return chapters.get(at.map_or(0, |i| i + 1)).map(start_ns);
    }
    let i = at?;
    let back = if base_ns - start_ns(&chapters[i]) > BACK_RESTART_NS { i } else { i.saturating_sub(1) };
    Some(start_ns(&chapters[back]))
}

/// A step from `base_ns` that lands at `target_ns` near a chapter start ahead of it lands ON that start:
/// the nearest start past `base_ns` in the step's direction within 45 % of the step ([`SNAP_CAP_NS`] at
/// most). The first chapter's start is the item's beginning and never pulls.
pub fn chapter_snap(chapters: &[metadata::Chapter], base_ns: i64, target_ns: i64) -> i64 {
    let step = target_ns.saturating_sub(base_ns);
    let reach = (step.saturating_abs() / 100 * 45).min(SNAP_CAP_NS);
    chapters
        .iter()
        .skip(1)
        .map(start_ns)
        .filter(|&c| if step > 0 { c > base_ns } else { step < 0 && c < base_ns })
        .filter(|&c| (c - target_ns).abs() <= reach)
        .min_by_key(|&c| (c - target_ns).abs())
        .unwrap_or(target_ns)
}

/// The name a chapter shows: its title, or "Chapter N" when the server sent none.
pub fn chapter_name(ch: &metadata::Chapter) -> String {
    if ch.title.trim().is_empty() {
        plx_platform::i18n::msg::widgets_chapters_number(ch.index)
    } else {
        ch.title.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn list(starts_s: &[i64]) -> Vec<metadata::Chapter> {
        starts_s
            .iter()
            .enumerate()
            .map(|(i, &s)| metadata::Chapter { index: i as i64 + 1, start_ms: s * 1000, title: String::new(), thumb: String::new() })
            .collect()
    }

    #[test]
    fn up_and_down_step_between_chapter_starts() {
        let ch = list(&[0, 100, 200]);
        assert_eq!(chapter_jump(&ch, 50 * S, true), Some(100 * S));
        assert_eq!(chapter_jump(&ch, 100 * S - S / 10, true), Some(200 * S), "a target just short of a start is in that chapter");
        assert_eq!(chapter_jump(&ch, 250 * S, true), None);
        assert_eq!(chapter_jump(&ch, 150 * S, false), Some(100 * S), "well into a chapter: back to its start");
        assert_eq!(chapter_jump(&ch, 102 * S, false), Some(0), "near its start: the previous chapter");
        assert_eq!(chapter_jump(&ch, S, false), Some(0));
    }

    #[test]
    fn a_step_landing_near_a_chapter_start_ahead_snaps_onto_it() {
        let ch = list(&[0, 100, 200]);
        assert_eq!(chapter_snap(&ch, 86 * S, 96 * S), 100 * S);
        assert_eq!(chapter_snap(&ch, 85 * S, 95 * S), 95 * S, "5 s off is outside 45 % of a 10 s step");
        assert_eq!(chapter_snap(&ch, 102 * S, 112 * S), 112 * S, "a start behind the step never pulls");
        assert_eq!(chapter_snap(&ch, 114 * S, 104 * S), 100 * S, "backward steps snap too");
        assert_eq!(chapter_snap(&ch, 30 * S, 92 * S), 100 * S, "a long step reaches 8 s at most");
        assert_eq!(chapter_snap(&ch, 30 * S, 91 * S), 91 * S);
        assert_eq!(chapter_snap(&ch, 8 * S, -2 * S), -2 * S, "the item's beginning never pulls");
    }
}