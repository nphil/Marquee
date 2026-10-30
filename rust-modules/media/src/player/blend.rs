//! **"Blend with video"**: the caption's brightness follows the picture instead of sitting at
//! full white on every scene.
//!
//! **Where it comes from.** Apple TV's default subtitles are slightly translucent with a soft shadow
//! so they sit in the picture instead of on it; Infuse changed its HDR subtitles from pure white to a
//! softer white-grey because full-brightness text on an HDR panel dominates a dark scene and blooms
//! on a local-dimming set (the Firecore forum thread "Subtitle brightness settings (HDR/DV)"); ITU-R
//! BT.2408 says graphics white should sit well under the HDR peak. This module is the
//! scene-following part of that: a target brightness from what is known of the scene, eased so a cut
//! never makes the text flicker.
//!
//! **What the player can know.** The picture is on a hardware video plane that GL cannot read back,
//! so the scene's luma is an INPUT ([`Scene::luma`]): the Settings preview knows its sample frame's,
//! and a player that has no estimate passes `None` and gets the neutral level. The HDR flag is always
//! known and always caps the level ([`HDR_CAP`]), on top of the viewer's HDR overlay brightness.
//!
//! Pure: the follower is a value, time comes in as an argument.

/// What is known about the picture behind the caption.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scene {
    /// Mean luma of the picture behind the caption, 0..=1 (display-referred), if known.
    pub luma: Option<f32>,
    /// The picture is HDR.
    pub hdr: bool,
}

impl Scene {
    pub const UNKNOWN: Scene = Scene { luma: None, hdr: false };
}

/// The caption's level on a dark scene: soft, so it does not glare.
pub const DARK_LEVEL: f32 = 0.74;
/// The caption's level on a bright scene: near white, so it still reads.
pub const BRIGHT_LEVEL: f32 = 0.94;
/// The level when nothing is known of the scene.
pub const NEUTRAL_LEVEL: f32 = 0.86;
/// The most an HDR picture's caption is allowed, before the HDR overlay brightness applies.
pub const HDR_CAP: f32 = 0.82;
/// Seconds the level takes to cover 63 % of a change.
pub const TIME_CONSTANT_S: f32 = 0.9;
/// A change of target smaller than this is not followed: no flicker from a noisy estimate.
const DEAD_BAND: f32 = 0.02;

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The level the caption wants on `scene`: brighter pictures get brighter text, an unknown scene the
/// neutral level, HDR never above [`HDR_CAP`].
pub fn target(scene: Scene) -> f32 {
    let level = match scene.luma {
        Some(l) => DARK_LEVEL + (BRIGHT_LEVEL - DARK_LEVEL) * smoothstep(0.05, 0.60, l.clamp(0.0, 1.0)),
        None => NEUTRAL_LEVEL,
    };
    if scene.hdr { level.min(HDR_CAP) } else { level }
}

/// The eased level. A value: the player owns one for the picture it draws over.
#[derive(Clone, Copy, Debug, Default)]
pub struct Follower {
    level: Option<f32>,
    goal: f32,
    at_ms: u32,
}

impl Follower {
    /// The level now: it eases toward `target(scene)` at [`TIME_CONSTANT_S`], and a target that moved
    /// less than the dead band from the one being followed is ignored. The first call snaps, so the
    /// first cue of a playback is never seen easing in from nothing.
    pub fn level(&mut self, now_ms: u32, scene: Scene) -> f32 {
        let want = target(scene);
        let Some(level) = self.level else {
            (self.level, self.goal, self.at_ms) = (Some(want), want, now_ms);
            return want;
        };
        if (want - self.goal).abs() >= DEAD_BAND {
            self.goal = want;
        }
        let dt = now_ms.wrapping_sub(self.at_ms).min(2_000) as f32 / 1000.0;
        self.at_ms = now_ms;
        // 1 - e^-x as a rational (x(6+x) / (6+4x+x^2): within 0.5 % of it up to x = 1, and exactly 0 and 1
        // at the ends), so the follower needs no libm call (`ci/allow/libm.txt`)
        let x = dt / TIME_CONSTANT_S;
        let step = x * (6.0 + x) / (6.0 + 4.0 * x + x * x);
        let next = level + (self.goal - level) * step;
        self.level = Some(next);
        next
    }

    /// Forget the level: the next [`Follower::level`] snaps. For a new playback or a closed preview.
    pub fn reset(&mut self) {
        self.level = None;
    }

    /// Has the level settled on its goal (within half the dead band)? What a caller that only
    /// redraws on change asks, so an easing caption keeps the frame loop awake until it is done.
    pub fn settled(&self) -> bool {
        self.level.is_none_or(|l| (l - self.goal).abs() < DEAD_BAND / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brighter_scenes_get_brighter_text_and_unknown_is_neutral() {
        let dark = target(Scene { luma: Some(0.02), hdr: false });
        let mid = target(Scene { luma: Some(0.3), hdr: false });
        let bright = target(Scene { luma: Some(0.9), hdr: false });
        assert!(dark < mid && mid < bright, "{dark} {mid} {bright}");
        assert_eq!(dark, DARK_LEVEL);
        assert_eq!(bright, BRIGHT_LEVEL);
        assert_eq!(target(Scene::UNKNOWN), NEUTRAL_LEVEL);
        assert_eq!(target(Scene { luma: Some(-3.0), hdr: false }), DARK_LEVEL, "out of range clamps");
    }

    #[test]
    fn hdr_caps_the_level_but_never_raises_it() {
        assert_eq!(target(Scene { luma: Some(0.9), hdr: true }), HDR_CAP);
        assert_eq!(target(Scene { luma: Some(0.0), hdr: true }), DARK_LEVEL, "already under the cap");
        assert_eq!(target(Scene { luma: None, hdr: true }), HDR_CAP);
    }

    #[test]
    fn the_first_call_snaps_and_a_cut_eases_over_about_a_second() {
        let mut f = Follower::default();
        let night = Scene { luma: Some(0.0), hdr: false };
        let day = Scene { luma: Some(0.8), hdr: false };
        assert_eq!(f.level(1_000, night), DARK_LEVEL);
        let mut last = DARK_LEVEL;
        for step in 1..=60u32 {
            let l = f.level(1_000 + step * 33, day);
            assert!(l >= last - 1e-6, "monotone toward the goal: {last} -> {l}");
            assert!(l <= BRIGHT_LEVEL + 1e-6);
            last = l;
        }
        assert!(last > 0.9, "about two seconds on, most of the way there: {last}");
        let early = {
            let mut g = Follower::default();
            g.level(0, night);
            g.level(33, day)
        };
        assert!(early - DARK_LEVEL < 0.01, "one frame after the cut the text has barely moved: {early}");
    }

    #[test]
    fn noise_inside_the_dead_band_does_not_move_the_level() {
        let mut f = Follower::default();
        let a = f.level(0, Scene { luma: Some(0.30), hdr: false });
        let mut b = a;
        for i in 1..120u32 {
            let wobble = 0.30 + if i % 2 == 0 { 0.01 } else { -0.01 };
            b = f.level(i * 33, Scene { luma: Some(wobble), hdr: false });
        }
        assert!((a - b).abs() < 0.005, "{a} vs {b}");
        assert!(f.settled());
    }

    #[test]
    fn a_long_pause_does_not_jump_and_reset_snaps_again() {
        let mut f = Follower::default();
        f.level(0, Scene { luma: Some(0.0), hdr: false });
        let after_pause = f.level(600_000, Scene { luma: Some(0.9), hdr: false });
        assert!(after_pause > 0.9, "a two-second cap on dt, long enough to settle: {after_pause}");
        f.reset();
        assert_eq!(f.level(600_100, Scene { luma: Some(0.0), hdr: false }), DARK_LEVEL);
    }
}
