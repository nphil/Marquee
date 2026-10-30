//! **The look of a subtitle, beyond the three quick picks.** Size, Position and Color keep their
//! ladders ([`SubtitleSize`](super::SubtitleSize), [`SubtitlePosition`](super::SubtitlePosition),
//! [`SubtitleTone`](super::SubtitleTone), the Style pages of the player's Subtitles menu); this is
//! everything else the Settings > Subtitles page edits: the font, an exact size, a free colour and
//! opacity, the edge, a background box or window, where the cue sits, the line spacing, the
//! "Blend with video" tone, "Reveal as spoken" and what happens to a file's own styling.
//!
//! A value of "follow the ladder" is spelled `size_pct == 0`, `color == None`: the viewer's most
//! recent pick wins, and the player's Style pages clear the matching field when they pick a rung,
//! so a rung picked mid-playback is never overridden by an older exact value.
//!
//! The struct is `Copy` (the live cell and the pref framework both need that), every field has a
//! default that reproduces the look every build before this one drew, and the soft parse costs a
//! garbled FIELD its own value only: an unknown spelling in one field leaves the others alone.

use serde::{Deserialize, Deserializer, Serialize};

/// A closed set that parses softly: an unknown spelling is the default, never a failure.
macro_rules! lenient_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? } default $default:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$vmeta])* $variant),+ }
        impl Default for $name {
            fn default() -> Self { $name::$default }
        }
        impl $name {
            /// Every value, in the order the Settings row steps through them.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// The stored spelling.
            pub fn spelling(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
            /// A stored spelling back to its value; anything else is `None`.
            pub fn from_spelling(s: &str) -> Option<Self> {
                match s { $($text => Some($name::$variant),)+ _ => None }
            }
            /// This value's place in [`Self::ALL`].
            pub fn index(self) -> usize {
                Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
            }
            /// The value `delta` places on, wrapping at both ends.
            pub fn step(self, delta: i32) -> Self {
                let n = Self::ALL.len() as i32;
                Self::ALL[(self.index() as i32 + delta).rem_euclid(n) as usize]
            }
        }
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.spelling())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = serde_json::Value::deserialize(d)?;
                Ok(v.as_str().and_then($name::from_spelling).unwrap_or_default())
            }
        }
    };
}

lenient_enum! {
    /// Which look the other fields were last set from: a preset's name, or `Custom` once any
    /// field was edited by hand. A label for the Settings row; it never changes how a cue draws.
    LookPreset {
        Marquee = "marquee",
        Blend = "blend",
        AppleTv = "apple_tv",
        Classic = "classic",
        Netflix = "netflix",
        Outline = "outline",
        LargePrint = "large_print",
        HighContrast = "high_contrast",
        Custom = "custom",
    } default Marquee
}

lenient_enum! {
    /// The bundled family. Every one falls back to the Default face and the per-script faces for
    /// the glyphs it lacks, so none can show a box for a script the app can draw.
    SubFont {
        /// Inter.
        Default = "default",
        /// Noto Serif.
        Serif = "serif",
        /// Noto Sans Mono.
        Mono = "mono",
        /// Atkinson Hyperlegible: shapes drawn to keep similar letters apart.
        Legible = "legible",
    } default Default
}

lenient_enum! {
    /// How the letters are edged. `Outline` and the two bevels are drawn by libass; `Drop` is its
    /// hard shadow; `Soft` is the diffuse depth shadow the player draws behind the letters.
    SubEdge {
        None = "none",
        Outline = "outline",
        Drop = "drop",
        Raised = "raised",
        Depressed = "depressed",
        Soft = "soft",
    } default Outline
}

lenient_enum! {
    /// The box behind the words: none, one per line, or one window around the whole cue.
    SubBack {
        Off = "off",
        Line = "line",
        Window = "window",
    } default Off
}

lenient_enum! {
    /// Which screen edge a cue without its own placement hangs from.
    SubAnchor {
        Bottom = "bottom",
        Top = "top",
    } default Bottom
}

lenient_enum! {
    /// "Reveal as spoken": show a cue whole, a word at a time, or a line at a time.
    Reveal {
        Off = "off",
        Word = "word",
        Line = "line",
    } default Off
}

lenient_enum! {
    /// What a file's own ASS/SSA styling does: `Keep` draws it as authored, `Mine` replaces its
    /// fonts, colours, edges and sizes with the viewer's look (placement and karaoke stay).
    AssMode {
        Keep = "keep",
        Mine = "mine",
    } default Keep
}

/// The exact sizes the Size row steps through, in percent of the Medium face.
pub const SIZE_STEPS: [u16; 15] = [50, 60, 70, 80, 90, 100, 110, 120, 130, 150, 175, 200, 225, 250, 300];
/// The colours the Colour row offers, as `0xRRGGBB`: white, soft white, yellow, cyan, green, pink.
pub const COLORS: [u32; 6] = [0xFFFFFF, 0xE8E4F0, 0xFFE680, 0x7FE3F0, 0x9EE493, 0xFFA3C4];
/// The background box colours: black, plum, slate, brown.
pub const BACK_COLORS: [u32; 4] = [0x000000, 0x1A1626, 0x2B2740, 0x3A1F1F];

/// The look. Field ranges are enforced by [`SubtitleLook::clamped`], which every reader of a
/// stored value applies, so a hand-edited file cannot ask libass for a 4000 px outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SubtitleLook {
    pub preset: LookPreset,
    pub font: SubFont,
    /// The heavier weight of the family. Inter Bold is what every build before this drew.
    pub bold: bool,
    /// Exact size, percent of the Medium face, 40..=300; 0 follows the Size ladder.
    pub size_pct: u16,
    /// Exact colour `0xRRGGBB`; `None` follows the Color ladder.
    pub color: Option<u32>,
    /// Opacity of the letters, 10..=100.
    pub opacity: u8,
    pub edge: SubEdge,
    /// Edge strength 1..=8: pixels of outline or hard shadow, or the spread of the soft shadow.
    pub edge_w: u8,
    pub back: SubBack,
    /// Opacity of the box, 0..=100.
    pub back_opacity: u8,
    /// Colour of the box, `0xRRGGBB`.
    pub back_color: u32,
    /// Corner radius of the box in screen pixels, 0..=40.
    pub back_radius: u8,
    /// Air between the words and the box edge, 0..=40 pixels.
    pub back_pad: u8,
    pub anchor: SubAnchor,
    /// Pixels the cue is moved away from the edge it hangs from, on top of the Position ladder:
    /// positive moves it inward, negative toward the edge (the safe area is the floor).
    pub nudge: i16,
    /// Line pitch, percent of the face's usual 48 px at Medium, 70..=200.
    pub line_pct: u8,
    /// "Blend with video": the letters ease toward the scene's brightness and stay below the HDR
    /// peak (the preset's name carries the reference).
    pub blend: bool,
    pub reveal: Reveal,
    pub ass: AssMode,
}
impl SubtitleLook {
    /// The look every build before the Subtitles page drew: Inter Bold, white, a 2 px black
    /// outline, no box, at the ladder's size and position. A const so a process-global cell can
    /// start from it.
    pub const DEFAULT: SubtitleLook = SubtitleLook {
        preset: LookPreset::Marquee,
        font: SubFont::Default,
        bold: true,
        size_pct: 0,
        color: None,
        opacity: 100,
        edge: SubEdge::Outline,
        edge_w: 2,
        back: SubBack::Off,
        back_opacity: 70,
        back_color: 0x000000,
        back_radius: 8,
        back_pad: 10,
        anchor: SubAnchor::Bottom,
        nudge: 0,
        line_pct: 100,
        blend: false,
        reveal: Reveal::Off,
        ass: AssMode::Keep,
    };
}

impl Default for SubtitleLook {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Field by field: a value of the wrong type, or a spelling this build does not know, leaves that
/// ONE field at its default and every other field as stored. A value that is not an object at all
/// is the default look.
impl<'de> Deserialize<'de> for SubtitleLook {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let mut look = SubtitleLook::default();
        let Some(object) = value.as_object() else { return Ok(look) };
        macro_rules! take {
            ($($field:ident),+) => {$(
                if let Some(v) = object.get(stringify!($field)).and_then(|v| serde_json::from_value(v.clone()).ok()) {
                    look.$field = v;
                }
            )+};
        }
        take!(
            preset, font, bold, size_pct, color, opacity, edge, edge_w, back, back_opacity, back_color, back_radius,
            back_pad, anchor, nudge, line_pct, blend, reveal, ass
        );
        Ok(look.clamped())
    }
}

impl SubtitleLook {
    /// Every field inside its range. Idempotent.
    pub fn clamped(mut self) -> Self {
        self.size_pct = if self.size_pct == 0 { 0 } else { self.size_pct.clamp(40, 300) };
        self.opacity = self.opacity.clamp(10, 100);
        self.edge_w = self.edge_w.clamp(1, 8);
        self.back_opacity = self.back_opacity.min(100);
        self.back_radius = self.back_radius.min(40);
        self.back_pad = self.back_pad.min(40);
        self.nudge = self.nudge.clamp(-60, 400);
        self.line_pct = self.line_pct.clamp(70, 200);
        self.color = self.color.map(|c| c & 0xFF_FFFF);
        self.back_color &= 0xFF_FFFF;
        self
    }

    /// This look with one field changed by hand: the preset label becomes `Custom`.
    pub fn edited(self, change: impl FnOnce(&mut Self)) -> Self {
        let mut next = self;
        change(&mut next);
        next.preset = LookPreset::Custom;
        next.clamped()
    }

    /// A number that differs for every distinct look, so a frame loop that redraws only on change
    /// can notice a colour, opacity, box, shadow or nudge changing (a 64-bit FNV-1a over every field).
    pub fn stamp(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        mix(self.preset.index() as u64);
        mix(self.font.index() as u64);
        mix(u64::from(self.bold));
        mix(u64::from(self.size_pct));
        mix(self.color.map_or(u64::MAX, u64::from));
        mix(u64::from(self.opacity));
        mix(self.edge.index() as u64);
        mix(u64::from(self.edge_w));
        mix(self.back.index() as u64);
        mix(u64::from(self.back_opacity));
        mix(u64::from(self.back_color));
        mix(u64::from(self.back_radius));
        mix(u64::from(self.back_pad));
        mix(self.anchor.index() as u64);
        mix(self.nudge as i64 as u64);
        mix(u64::from(self.line_pct));
        mix(u64::from(self.blend));
        mix(self.reveal.index() as u64);
        mix(self.ass.index() as u64);
        h
    }

    /// The size rung `delta` steps from `current` (percent; 0 reads as 100) on [`SIZE_STEPS`],
    /// clamped at both ends. The nearest rung is used when `current` is not on the list.
    pub fn size_step(current: u16, delta: i32) -> u16 {
        let at = if current == 0 { 100 } else { current };
        let nearest = SIZE_STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| (i32::from(**s) - i32::from(at)).abs())
            .map_or(0, |(i, _)| i);
        SIZE_STEPS[(nearest as i32 + delta).clamp(0, SIZE_STEPS.len() as i32 - 1) as usize]
    }

    /// Does the look draw a box behind the words?
    pub fn has_back(&self) -> bool {
        self.back != SubBack::Off && self.back_opacity > 0
    }
}

impl LookPreset {
    /// The presets the picker lists, best first. `Custom` is a label, not a choice.
    pub const PICKER: [LookPreset; 8] = [
        LookPreset::Blend,
        LookPreset::AppleTv,
        LookPreset::Classic,
        LookPreset::Netflix,
        LookPreset::Outline,
        LookPreset::LargePrint,
        LookPreset::HighContrast,
        LookPreset::Marquee,
    ];

    /// The complete look of this preset. A preset leaves the Size ladder, the Position ladder and
    /// the Color ladder to the viewer unless it exists to set one (Large print sets the size,
    /// High contrast the colour).
    pub fn look(self) -> SubtitleLook {
        let base = SubtitleLook::default();
        let look = match self {
            LookPreset::Marquee | LookPreset::Custom => base,
            // Apple TV's default look softened the way Infuse softens HDR white: translucent
            // letters, a diffuse shadow, no box, brightness that follows the scene.
            LookPreset::Blend => SubtitleLook {
                bold: false,
                color: Some(0xEDEBF4),
                opacity: 92,
                edge: SubEdge::Soft,
                edge_w: 3,
                blend: true,
                ..base
            },
            // Apple TV's default: white, medium weight, a soft shadow, no box.
            LookPreset::AppleTv => SubtitleLook { bold: true, edge: SubEdge::Soft, edge_w: 2, ..base },
            // Apple's Classic and the CEA-708 default: white on a black box, square corners.
            LookPreset::Classic => SubtitleLook {
                bold: false,
                edge: SubEdge::None,
                back: SubBack::Line,
                back_opacity: 75,
                back_radius: 0,
                back_pad: 8,
                ..base
            },
            // Netflix: white, regular weight, a hard drop shadow.
            LookPreset::Netflix => SubtitleLook { bold: false, edge: SubEdge::Drop, edge_w: 2, ..base },
            // Apple's Outline Text and VLC: white with a thick black outline.
            LookPreset::Outline => SubtitleLook { bold: true, edge_w: 3, ..base },
            // Apple's Large Text: bigger, heavier, outlined.
            LookPreset::LargePrint => SubtitleLook { bold: true, size_pct: 150, edge_w: 3, ..base },
            // Yellow on black: the accessibility pairing with the most contrast.
            LookPreset::HighContrast => SubtitleLook {
                bold: true,
                color: Some(0xFFE45C),
                edge: SubEdge::Outline,
                edge_w: 3,
                back: SubBack::Line,
                back_opacity: 85,
                back_radius: 6,
                back_pad: 8,
                ..base
            },
        };
        SubtitleLook { preset: self, ..look }.clamped()
    }

    /// This preset picked over `current`: every field the preset styles, with what a file's own
    /// styling does left as the viewer had it (a behaviour of the player, not a look).
    pub fn applied(self, current: &SubtitleLook) -> SubtitleLook {
        SubtitleLook { ass: current.ass, ..self.look() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_marquee_preset_and_the_look_before_this_page() {
        assert_eq!(SubtitleLook::default(), LookPreset::Marquee.look());
        let d = SubtitleLook::default();
        assert!(d.bold && d.size_pct == 0 && d.color.is_none() && d.opacity == 100);
        assert_eq!((d.edge, d.edge_w, d.back, d.anchor, d.nudge), (SubEdge::Outline, 2, SubBack::Off, SubAnchor::Bottom, 0));
        assert!(!d.blend && d.reveal == Reveal::Off && d.ass == AssMode::Keep);
    }

    #[test]
    fn a_preset_keeps_how_the_viewer_treats_a_files_own_styling() {
        let mine = SubtitleLook { ass: AssMode::Mine, ..SubtitleLook::default() };
        for p in LookPreset::PICKER {
            let next = p.applied(&mine);
            assert_eq!(next.ass, AssMode::Mine, "{p:?}");
            assert_eq!(SubtitleLook { ass: AssMode::Keep, ..next }, p.look(), "{p:?} styles everything else");
        }
    }

    #[test]
    fn every_preset_is_in_range_and_names_itself() {
        for p in LookPreset::PICKER {
            let look = p.look();
            assert_eq!(look.preset, p);
            assert_eq!(look, look.clamped(), "{p:?} is already inside its ranges");
        }
        assert!(LookPreset::Blend.look().blend && LookPreset::Blend.look().edge == SubEdge::Soft);
        assert!(!LookPreset::PICKER.contains(&LookPreset::Custom));
    }

    #[test]
    fn stored_looks_round_trip_and_a_garbled_field_costs_only_itself() {
        for p in LookPreset::PICKER {
            let look = p.look();
            let json = serde_json::to_string(&look).unwrap();
            assert_eq!(serde_json::from_str::<SubtitleLook>(&json).unwrap(), look, "{p:?}");
        }
        let mixed: SubtitleLook = serde_json::from_str(
            r#"{"font":"comic","bold":false,"edge":"drop","edge_w":"wide","size_pct":150,"future":1}"#,
        )
        .unwrap_or_else(|e| panic!("a bad spelling must not fail the whole look: {e}"));
        assert_eq!(mixed.font, SubFont::Default, "an unknown spelling reads as the default");
        assert!(!mixed.bold && mixed.edge == SubEdge::Drop && mixed.size_pct == 150);
        assert!(serde_json::from_str::<SubtitleLook>("{}").is_ok());
    }

    #[test]
    fn ranges_are_enforced_and_idempotent() {
        let wild = SubtitleLook {
            size_pct: 9999,
            opacity: 0,
            edge_w: 200,
            back_opacity: 250,
            back_radius: 250,
            back_pad: 250,
            nudge: i16::MAX,
            line_pct: 1,
            color: Some(0xFF00_0000),
            back_color: 0xFF12_3456,
            ..SubtitleLook::default()
        }
        .clamped();
        assert_eq!((wild.size_pct, wild.opacity, wild.edge_w), (300, 10, 8));
        assert_eq!((wild.back_opacity, wild.back_radius, wild.back_pad, wild.nudge, wild.line_pct), (100, 40, 40, 400, 70));
        assert_eq!((wild.color, wild.back_color), (Some(0), 0x12_3456));
        assert_eq!(wild, wild.clamped());
        assert_eq!(SubtitleLook { size_pct: 0, ..wild }.clamped().size_pct, 0, "0 is the ladder, not a size");
    }

    #[test]
    fn size_steps_walk_the_list_from_the_nearest_rung_and_stop_at_the_ends() {
        assert_eq!(SubtitleLook::size_step(0, 1), 110, "0 follows the ladder: Medium is 100");
        assert_eq!(SubtitleLook::size_step(100, -1), 90);
        assert_eq!(SubtitleLook::size_step(300, 1), 300);
        assert_eq!(SubtitleLook::size_step(50, -1), 50);
        assert_eq!(SubtitleLook::size_step(137, 0), 130, "an off-list value snaps to the nearest rung");
    }

    #[test]
    fn enums_step_and_wrap() {
        assert_eq!(SubFont::Default.step(-1), SubFont::Legible);
        assert_eq!(SubFont::Legible.step(1), SubFont::Default);
        assert_eq!(SubEdge::ALL.len(), 6);
        for e in SubEdge::ALL {
            assert_eq!(SubEdge::from_spelling(e.spelling()), Some(*e));
        }
    }

    #[test]
    fn an_edit_makes_the_preset_custom() {
        let blend = LookPreset::Blend.look();
        let edited = blend.edited(|l| l.opacity = 60);
        assert_eq!(edited.preset, LookPreset::Custom);
        assert_eq!(edited.opacity, 60);
        assert_eq!(edited.edge, blend.edge);
    }
}
