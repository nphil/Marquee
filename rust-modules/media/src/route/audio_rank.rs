//! **Which sound track to prefer among the ones the TV can carry.** The track the planner picks
//! (`plan::pick_dp_audio_eligible`: the viewer's choice, language, the file's default) is upgraded to
//! the best sibling of the same language and kind, ranked by what reaches the receiver:
//!
//! 1. Dolby TrueHD, only while the sound chain carries it in the MAT carrier ([`Whole::truehd`],
//!    `truehd`; the stream is the lossless one, untouched);
//! 2. DTS-HD Master Audio, High Resolution and DTS:X, only while the sound chain passes DTS-HD whole
//!    ([`Whole::dts_hd`], `ff::lossless`; likewise untouched);
//! 3. E-AC3 with Dolby Atmos;
//! 4. DTS (the core);
//! 5. E-AC3;
//! 6. AC3.
//!
//! **TrueHD is on the list only with the carrier.** The television's pipeline has no TrueHD or MAT
//! input (libpf's Load names MP3, AC3, AAC, WMA, DTS, AC3 PLUS, OPUS, FLAC, PCM and a few more), and
//! it passes TrueHD only from an HDMI input, so a TrueHD track reaches the receiver only as 8-channel
//! PCM that the helper routes past the mixer. Without that, a file with TrueHD and an AC3/E-AC3
//! sibling plays the sibling (`devcaps::DP_AUDIO_CODECS`), the best the chain can carry.
//!
//! The viewer's explicit pick and any track outside these families are never replaced.
use plx_data::metadata::Stream;

/// What the sound chain passes to the receiver untouched, so a lossless track is the stream itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Whole {
    /// DTS-HD, whole (`ff::lossless::planned`).
    pub dts_hd: bool,
    /// TrueHD, in the MAT carrier (`truehd::planned`).
    pub truehd: bool,
}

impl Whole {
    pub(super) fn any(self) -> bool {
        self.dts_hd || self.truehd
    }
}

/// How good a track's delivery is; `None` for a codec this ranking does not speak for.
pub(super) fn rank(s: &Stream, whole: Whole) -> Option<u8> {
    let profile = s.profile.to_ascii_lowercase();
    match plx_platform::devcaps::audio_codec_id(&s.codec).as_ref() {
        "truehd" if whole.truehd => Some(6),
        "dts" if whole.dts_hd && is_dts_hd(&profile) => Some(5),
        "dts" => Some(2),
        "eac3" if s.has_atmos() => Some(4),
        "eac3" => Some(1),
        "ac3" => Some(0),
        _ => None,
    }
}

/// A DTS profile with an extension on the core (Master Audio, High Resolution, Express excepted:
/// it is core-only, lossy), as PMS spells it: `dts-hd ma`, `dts-hd hra`, `dts:x`.
pub(crate) fn is_dts_hd(profile: &str) -> bool {
    profile.contains("dts-hd ma") || profile.contains("dts-hd hra") || profile.contains("dts:x") || profile.contains("dts-x")
}

/// `picked`, or the best-ranked track of its language and commentary-ness that `plays`, when that
/// outranks it. First of equals, so the file's order breaks ties. `same(a, b)` is the planner's
/// same-language-and-kind test.
pub(super) fn upgraded(
    tracks: &[Stream], picked: usize, whole: Whole,
    plays: impl Fn(&Stream) -> bool, same: impl Fn(usize, usize) -> bool,
) -> usize {
    let Some(current) = rank(&tracks[picked], whole) else {
        return picked;
    };
    (0..tracks.len())
        .filter(|&i| same(picked, i) && plays(&tracks[i]))
        .filter_map(|i| rank(&tracks[i], whole).filter(|&r| r > current).map(|r| (r, i)))
        .fold(None, |best: Option<(u8, usize)>, (r, i)| match best {
            Some((br, _)) if br >= r => best,
            _ => Some((r, i)),
        })
        .map_or(picked, |(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(codec: &str, profile: &str) -> Stream {
        Stream { codec: codec.into(), profile: profile.into(), channels: 6, ..Default::default() }
    }

    const DTS_HD: Whole = Whole { dts_hd: true, truehd: false };
    const TRUEHD: Whole = Whole { dts_hd: true, truehd: true };

    fn pick(tracks: &[Stream], picked: usize, dts_hd: bool) -> usize {
        upgraded(tracks, picked, Whole { dts_hd, truehd: false }, |t| t.codec != "truehd", |_, _| true)
    }

    #[test]
    fn lossless_dts_outranks_everything_while_the_chain_passes_it() {
        let t = [track("ac3", ""), track("eac3", "dolby digital plus + dolby atmos"), track("dca", "dts-hd ma")];
        assert_eq!(pick(&t, 0, true), 2);
        assert_eq!(pick(&t, 1, true), 2);
        // not passed whole: the core is plain DTS, below the Atmos track
        assert_eq!(pick(&t, 0, false), 1);
        assert_eq!(pick(&t, 2, false), 1, "E-AC3 Atmos outranks the DTS core");
    }

    #[test]
    fn the_order_below_lossless_is_atmos_dts_eac3_ac3() {
        let t = [track("ac3", ""), track("eac3", ""), track("dts", "dts"), track("eac3", "dolby digital plus + dolby atmos")];
        assert_eq!(pick(&t, 0, false), 3);
        assert_eq!(pick(&t[..3], 0, false), 2);
        assert_eq!(pick(&[track("ac3", ""), track("eac3", "")], 0, false), 1);
        assert_eq!(pick(&[track("eac3", ""), track("ac3", "")], 0, false), 0, "never down");
    }

    #[test]
    fn truehd_outranks_everything_only_while_the_carrier_is_planned() {
        let t = [track("ac3", ""), track("dts", "dts-hd ma"), track("truehd", "truehd + dolby atmos")];
        assert_eq!(upgraded(&t, 0, TRUEHD, |_| true, |_, _| true), 2);
        assert_eq!(upgraded(&t, 1, TRUEHD, |_| true, |_, _| true), 2, "above lossless DTS too");
        assert_eq!(upgraded(&t, 0, DTS_HD, |_| true, |_, _| true), 1, "no carrier: TrueHD is not ranked");
        assert_eq!(pick(&t, 0, true), 1, "the planner's own pass never takes a TrueHD track");
        assert_eq!(upgraded(&t, 2, TRUEHD, |_| true, |_, _| true), 2, "nothing outranks it");
    }

    #[test]
    fn a_codec_outside_the_families_stays_put() {
        let t = [track("aac", ""), track("dts", "dts-hd ma"), track("truehd", "")];
        assert_eq!(pick(&t, 0, true), 0, "a codec outside the families is the viewer's");
        assert_eq!(upgraded(&t, 0, TRUEHD, |_| true, |_, _| true), 0);
    }

    #[test]
    fn only_a_track_of_the_same_language_and_kind_that_plays_replaces_the_pick() {
        let t = [track("ac3", ""), track("dts", "dts-hd ma")];
        assert_eq!(upgraded(&t, 0, DTS_HD, |_| true, |_, _| false), 0, "another language");
        assert_eq!(upgraded(&t, 0, DTS_HD, |s| s.codec != "dts", |_, _| true), 0, "a track the TV cannot take");
    }

    #[test]
    fn dts_express_is_not_lossless() {
        assert_eq!(rank(&track("dts", "dts-hd ma"), DTS_HD), Some(5));
        assert_eq!(rank(&track("dts", "dts-hd hra"), DTS_HD), Some(5));
        assert_eq!(rank(&track("dts", "dts-x"), DTS_HD), Some(5));
        assert_eq!(rank(&track("dts", "dts-hd express"), DTS_HD), Some(2));
        assert_eq!(rank(&track("dts", "dts"), DTS_HD), Some(2));
    }
}
