//! The per-profile choice for instant D-pad seeking.
use serde::{Deserialize, Serialize};

/// Delay before acting on a Plex intro or credits marker.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoSkipDelay {
    #[default]
    Manual,
    Seconds3, Seconds5, Seconds10, Seconds15, Seconds30,
}
impl AutoSkipDelay {
    pub const LADDER: [Self; 6] = [Self::Manual, Self::Seconds3, Self::Seconds5, Self::Seconds10, Self::Seconds15, Self::Seconds30];
    pub fn from_index(i: u8) -> Self { Self::LADDER.get(i as usize).copied().unwrap_or_default() }
    pub fn index(self) -> u8 { Self::LADDER.iter().position(|&v| v == self).unwrap_or(0) as u8 }
    pub fn is_default(&self) -> bool { *self == Self::Manual }
    pub fn milliseconds(self) -> Option<u64> {
        match self { Self::Manual => None, Self::Seconds3 => Some(3_000), Self::Seconds5 => Some(5_000), Self::Seconds10 => Some(10_000), Self::Seconds15 => Some(15_000), Self::Seconds30 => Some(30_000) }
    }
}

/// Countdown length while Next episode is set to Countdown.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CreditsCountdownDelay { Seconds3, #[default] Seconds5, Seconds10, Seconds15, Seconds30 }
impl CreditsCountdownDelay {
    pub const LADDER: [Self; 5] = [Self::Seconds3, Self::Seconds5, Self::Seconds10, Self::Seconds15, Self::Seconds30];
    pub fn from_index(i: u8) -> Self { Self::LADDER.get(i as usize).copied().unwrap_or_default() }
    pub fn index(self) -> u8 { Self::LADDER.iter().position(|&v| v == self).unwrap_or(1) as u8 }
    pub fn is_default(&self) -> bool { *self == Self::default() }
    pub const fn milliseconds(self) -> u64 {
        match self { Self::Seconds3 => 3_000, Self::Seconds5 => 5_000, Self::Seconds10 => 10_000, Self::Seconds15 => 15_000, Self::Seconds30 => 30_000 }
    }
}

/// Remember explicit audio selections separately for each title on this profile.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RememberAudioChoices {
    #[serde(rename = "off")]
    Off,
    #[default]
    #[serde(rename = "on")]
    On,
}

impl RememberAudioChoices {
    pub const LADDER: [Self; 2] = [Self::Off, Self::On];
    pub fn from_index(i: u8) -> Self { Self::LADDER.get(i as usize).copied().unwrap_or_default() }
    pub fn index(self) -> u8 { Self::LADDER.iter().position(|&m| m == self).unwrap_or(1) as u8 }
    pub fn is_default(&self) -> bool { *self == Self::default() }
}

/// Whether Left/Right on a hidden transport jumps at once instead of first revealing the scrubber —
/// one per profile, like [`super::DeckPress`]. Absence is `Off`, the reveal-first behaviour every
/// build before the preference had.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstantDpadSeeking {
    #[default]
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "on")]
    On,
}

impl InstantDpadSeeking {
    /// Every option, the order the picker lists them in.
    pub const LADDER: [InstantDpadSeeking; 2] = [InstantDpadSeeking::Off, InstantDpadSeeking::On];

    /// An in-memory index back to an option — out of range is the default, for the reason
    /// [`super::SubtitleTone::from_index`] gives: the ladder can grow or shrink.
    pub fn from_index(i: u8) -> InstantDpadSeeking {
        Self::LADDER.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u8 {
        Self::LADDER.iter().position(|&m| m == self).unwrap_or(0) as u8
    }

    /// `skip_serializing_if` needs a function, not just `PartialEq` with `Default::default()`.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether Dolby Vision is declared when the TV reports support, or never — one per profile, like
/// [`InstantDpadSeeking`]. `Off` sends no Dolby Vision node, so a stream with a cross-compatible
/// base layer (Profile 8.1) plays that layer, and one without (Profile 5) is refused direct play
/// and asked of the server, which may refuse it too (code 2000, `docs/pms-api.md`). Absence is
/// `Auto`, which trusts the firmware capability probe (`tv.config.supportDolbyHDRContents`).
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DolbyVisionMode {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "off")]
    Off,
}

impl DolbyVisionMode {
    pub const DEFAULT: Self = Self::Auto;
    /// Every option, the order the picker lists them in.
    pub const LADDER: [DolbyVisionMode; 2] = [DolbyVisionMode::Auto, DolbyVisionMode::Off];

    pub fn from_index(i: u8) -> DolbyVisionMode {
        Self::LADDER.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u8 {
        Self::LADDER.iter().position(|&m| m == self).unwrap_or(0) as u8
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The capability a route's presentation is planned from: `Auto` is the set's own answer, `Off`
    /// is `Unsupported` whatever the set reports. Applied where a route is planned
    /// (`route::frozen_dv_decision`), to the presentation only: never to the probe itself and never
    /// to the capability a playing route's `DvDecision` stores, so Diagnostics and the `dv:` log
    /// lines still show what the television said.
    pub fn gate(self, probed: plx_platform::devcaps::dv::DvCapability) -> plx_platform::devcaps::dv::DvCapability {
        match self {
            DolbyVisionMode::Auto => probed,
            DolbyVisionMode::Off => plx_platform::devcaps::dv::DvCapability::Unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::session::{effective, own_prefs, put_own, DolbyVisionModePref,
        InstantDpadSeekingPref, Preferences, ProfilePrefs};

    #[test]
    fn instant_dpad_seeking_is_defaulted_and_scoped_per_profile() {
        for json in ["{}", r#"{"instant_dpad_seeking":"future"}"#, r#"{"instant_dpad_seeking":null}"#] {
            let prefs: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(prefs.instant_dpad_seeking, InstantDpadSeeking::Off);
            let encoded = serde_json::to_value(&prefs).unwrap();
            assert!(encoded.get("instant_dpad_seeking").is_none());
        }
        let prefs = Preferences::default();
        let mut records = Vec::new();
        assert!(put_own::<InstantDpadSeekingPref>(&mut records, "alice", "account", InstantDpadSeeking::On));
        assert_eq!(effective::<InstantDpadSeekingPref>(&prefs, own_prefs(&records, "alice", "account")), InstantDpadSeeking::On);
        for (user, account) in [("bob", "account"), ("alice", "other")] {
            assert_eq!(effective::<InstantDpadSeekingPref>(&prefs, own_prefs(&records, user, account)), InstantDpadSeeking::Off);
        }
        let encoded = serde_json::to_string(&records[0].prefs).unwrap();
        assert_eq!(serde_json::from_str::<ProfilePrefs>(&encoded).unwrap(), records[0].prefs);
    }

    #[test]
    fn remember_audio_defaults_softly_and_is_isolated_by_profile_and_account() {
        use crate::plex::session::RememberAudioChoicesPref;
        for json in ["{}", r#"{"remember_audio_choices":"future"}"#, r#"{"remember_audio_choices":null}"#] {
            let prefs: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(prefs.remember_audio_choices, RememberAudioChoices::On);
            assert!(serde_json::to_value(&prefs).unwrap().get("remember_audio_choices").is_none());
        }
        let prefs = Preferences::default();
        let mut records = Vec::new();
        assert!(put_own::<RememberAudioChoicesPref>(&mut records, "alice", "account", RememberAudioChoices::Off));
        let encoded = serde_json::to_string(&records[0].prefs).unwrap();
        let decoded: ProfilePrefs = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.remember_audio_choices, Some(RememberAudioChoices::Off));
        assert_eq!(effective::<RememberAudioChoicesPref>(&prefs, own_prefs(&records, "alice", "account")), RememberAudioChoices::Off);
        for (user, account) in [("bob", "account"), ("alice", "other")] {
            assert_eq!(effective::<RememberAudioChoicesPref>(&prefs, own_prefs(&records, user, account)), RememberAudioChoices::On);
        }
        let invalid: ProfilePrefs = serde_json::from_str(r#"{"remember_audio_choices":"future"}"#).unwrap();
        assert_eq!(invalid.remember_audio_choices, None);
        assert_eq!(RememberAudioChoices::from_index(255), RememberAudioChoices::On);
    }

    #[test]
    fn skip_delays_are_softly_defaulted_and_profile_isolated() {
        use crate::plex::session::{IntroSkipDelayPref, CreditsSkipDelayPref};
        for json in ["{}", r#"{"intro_skip_delay":"unknown","credits_skip_delay":"manual"}"#] {
            let prefs: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(prefs.intro_skip_delay, AutoSkipDelay::Manual);
            assert_eq!(prefs.credits_skip_delay, CreditsCountdownDelay::Seconds5);
        }
        assert_eq!(AutoSkipDelay::LADDER.map(AutoSkipDelay::milliseconds), [None, Some(3000), Some(5000), Some(10000), Some(15000), Some(30000)]);
        assert_eq!(CreditsCountdownDelay::LADDER.map(CreditsCountdownDelay::milliseconds), [3000, 5000, 10000, 15000, 30000]);
        let prefs = Preferences::default();
        let mut records = Vec::new();
        assert!(put_own::<IntroSkipDelayPref>(&mut records, "alice", "account", AutoSkipDelay::Seconds3));
        assert!(put_own::<CreditsSkipDelayPref>(&mut records, "alice", "account", CreditsCountdownDelay::Seconds30));
        let own = own_prefs(&records, "alice", "account");
        assert_eq!(effective::<IntroSkipDelayPref>(&prefs, own), AutoSkipDelay::Seconds3);
        assert_eq!(effective::<CreditsSkipDelayPref>(&prefs, own), CreditsCountdownDelay::Seconds30);
        let json = serde_json::to_string(&records[0].prefs).unwrap();
        assert_eq!(serde_json::from_str::<ProfilePrefs>(&json).unwrap(), records[0].prefs);
        for (user, account) in [("bob", "account"), ("alice", "other")] {
            let own = own_prefs(&records, user, account);
            assert_eq!(effective::<IntroSkipDelayPref>(&prefs, own), AutoSkipDelay::Manual);
            assert_eq!(effective::<CreditsSkipDelayPref>(&prefs, own), CreditsCountdownDelay::Seconds5);
        }
    }

    #[test]
    fn dolby_vision_mode_is_defaulted_and_scoped_per_profile() {
        for json in ["{}", r#"{"dolby_vision_mode":"future"}"#, r#"{"dolby_vision_mode":null}"#] {
            let prefs: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(prefs.dolby_vision_mode, DolbyVisionMode::Auto);
            let encoded = serde_json::to_value(&prefs).unwrap();
            assert!(encoded.get("dolby_vision_mode").is_none());
        }
        let prefs = Preferences::default();
        let mut records = Vec::new();
        assert!(put_own::<DolbyVisionModePref>(&mut records, "alice", "account", DolbyVisionMode::Off));
        assert_eq!(effective::<DolbyVisionModePref>(&prefs, own_prefs(&records, "alice", "account")), DolbyVisionMode::Off);
        for (user, account) in [("bob", "account"), ("alice", "other")] {
            assert_eq!(effective::<DolbyVisionModePref>(&prefs, own_prefs(&records, user, account)), DolbyVisionMode::Auto);
        }
        let encoded = serde_json::to_string(&records[0].prefs).unwrap();
        assert_eq!(serde_json::from_str::<ProfilePrefs>(&encoded).unwrap(), records[0].prefs);
    }

    #[test]
    fn dolby_vision_off_plans_as_an_unsupported_set_and_auto_trusts_the_probe() {
        use plx_platform::devcaps::dv::DvCapability::{Supported, Unknown, Unsupported};
        for probed in [Supported, Unknown, Unsupported] {
            assert_eq!(DolbyVisionMode::Auto.gate(probed), probed);
            assert_eq!(DolbyVisionMode::Off.gate(probed), Unsupported);
        }
    }
}
