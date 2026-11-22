//! **The automatic audio and subtitle choice**, pure over plain values: what plays and what is shown
//! when the viewer has chosen nothing for this title.
//!
//! The rules (the library owner's, `docs/marquee/track-selection-cases.json` is their table):
//! - Movies and non-anime TV: the original-language audio, never a dub; the subtitle in the
//!   viewer's language is ON, also when the audio is already that language.
//! - Anime: the dub in the viewer's language when the file has one, with the subtitle that matches
//!   the dub's script (a hearing-impaired sidecar, `.en.cc.srt`); with the original audio, or no dub,
//!   the subtitle that matches the original (`.en.srt`).
//! - The subtitle is a sidecar when one exists in the language, else an embedded TEXT track, drawn in
//!   the app. Never a picture track, never a forced or signs-only track, never a server burn.
//! - Either variant stands in for the other when it is the only one ("`.en.cc.srt` only" films).
//! - A remembered choice (a track, or Off) decides before any of this, except an anime's remembered
//!   sidecar: it names the language, the variant above stays the audio's (`route::plan::pick_subtitle`).
//!
//! The case table is a file the Swift phone apps copy; `tests` runs every case against these
//! functions, so the two cannot drift.

use super::sub_layout::{is_image_sub_codec, sub_traits};
use super::{lang_matches, Stream};

/// One audio track as the choice reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioIn {
    pub id: i64,
    pub lang: String,
    /// The title's original-language track.
    pub original: bool,
    /// A track in another language than the original.
    pub dub: bool,
}

/// What the viewer already decided for this title or show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remembered {
    pub audio: Option<i64>,
    pub subtitle: Option<RememberedSub>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RememberedSub {
    Off,
    Track(i64),
}

/// The answer: the audio track (`None` when the choice has no opinion) and the subtitle track
/// (`None` is Off).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub audio: Option<i64>,
    pub subtitle: Option<i64>,
}

/// The language a country's films are originally made in, for the countries of a Plex library. Only
/// languages a Plex audio track is plausibly tagged with; a country not listed names no language.
fn country_language(country: &str) -> Option<&'static str> {
    Some(match country.trim().to_ascii_lowercase().as_str() {
        "united states of america" | "united states" | "usa" | "us" | "united kingdom" | "uk" | "canada"
        | "australia" | "ireland" | "new zealand" => "en",
        "france" => "fr",
        "germany" | "austria" => "de",
        "italy" => "it",
        "spain" | "mexico" | "argentina" | "colombia" | "chile" | "peru" => "es",
        "portugal" | "brazil" => "pt",
        "japan" => "ja",
        "south korea" | "korea" | "republic of korea" => "ko",
        "china" | "hong kong" | "taiwan" | "taiwan, province of china" => "zh",
        "india" => "hi",
        "russia" | "russian federation" | "soviet union" => "ru",
        "sweden" => "sv",
        "norway" => "no",
        "denmark" => "da",
        "finland" => "fi",
        "netherlands" => "nl",
        "poland" => "pl",
        "turkey" => "tr",
        "thailand" => "th",
        "iceland" => "is",
        "hungary" => "hu",
        "czech republic" | "czechia" => "cs",
        "greece" => "el",
        "israel" => "he",
        "iran" => "fa",
        "ukraine" => "uk",
        _ => return None,
    })
}

/// The title's original-language audio track: the first country, in the library's order, whose
/// language has a track (the file's default-flagged one among several); anime falls back to
/// Japanese. A title the library gives countries for, but none of them names a language the file
/// has, falls back to the file's own default track, else its first (never a commentary, a
/// description or a track titled as a dub), as the phone apps do for every title. A commentary or
/// description track is never the original. `None` for a title with no country that is not anime,
/// which leaves the choice to the server's and the file's own defaults.
pub fn original_audio(audio: &[Stream], anime: bool, countries: &[String]) -> Option<usize> {
    let usable = |s: &Stream| !s.ad && !super::track_label::is_commentary(&s.title);
    let in_language = |lang: &str| {
        let hits = || {
            audio.iter().enumerate().filter(move |(_, s)| usable(s) && lang_matches(lang, super::audio_language(s)))
        };
        hits().find(|(_, s)| s.default).or_else(|| hits().next()).map(|(i, _)| i)
    };
    let from_file = || {
        let hits = || audio.iter().enumerate().filter(|(_, s)| usable(s) && !super::track_label::names_dub(&s.title));
        hits().find(|(_, s)| s.default).or_else(|| hits().next()).map(|(i, _)| i)
    };
    countries
        .iter()
        .filter_map(|c| country_language(c))
        .find_map(in_language)
        .or_else(|| anime.then(|| in_language("ja")).flatten())
        .or_else(|| (!countries.is_empty()).then(from_file).flatten())
}

/// [`AudioIn`] for each track of `audio`, with the original marked by [`original_audio`].
pub fn audio_inputs(audio: &[Stream], anime: bool, countries: &[String]) -> Vec<AudioIn> {
    let original = original_audio(audio, anime, countries);
    audio
        .iter()
        .enumerate()
        .map(|(i, s)| AudioIn {
            id: s.id,
            lang: super::audio_language(s).to_string(),
            original: original == Some(i),
            dub: original.is_some_and(|o| o != i),
        })
        .collect()
}

/// The audio to play: a remembered track, else for anime the dub in the viewer's language
/// (`lang`) when there is one, else the original. `None` when nothing is known to be original.
pub fn pick_audio(audio: &[AudioIn], anime: bool, lang: &str, remembered: Option<i64>) -> Option<i64> {
    if let Some(id) = remembered.filter(|id| audio.iter().any(|a| a.id == *id)) {
        return Some(id);
    }
    let dub = anime.then(|| audio.iter().find(|a| a.dub && lang_matches(lang, &a.lang))).flatten();
    dub.or_else(|| audio.iter().find(|a| a.original)).map(|a| a.id)
}

/// Is `s` a subtitle the app draws from text: a downloaded text file, or a text track inside the
/// container (not a picture, not closed captions riding the video).
fn drawable_text(s: &Stream) -> bool {
    if s.external {
        s.sidecar_renderable()
    } else {
        s.id > 0 && !s.in_video && !s.codec.is_empty() && !is_image_sub_codec(&s.codec)
    }
}

/// The subtitle track to show, or `None` for Off. `dub_audio` says the audio that plays is the
/// viewer-language dub of an anime, which asks for the dub-matched variant; every other audio asks
/// for the one that matches the original.
pub fn pick_subtitle(
    subs: &[Stream], lang: &str, dub_audio: bool, remembered: Option<RememberedSub>,
) -> Option<i64> {
    match remembered {
        Some(RememberedSub::Off) => return None,
        Some(RememberedSub::Track(id)) if subs.iter().any(|s| s.id == id) => return Some(id),
        _ => {}
    }
    let full = |s: &&Stream| {
        let t = sub_traits(s);
        lang_matches(lang, &s.lang_code) && drawable_text(s) && !t.forced && !t.signs && !t.commentary
    };
    let candidates = || subs.iter().filter(full);
    let variant = |external: bool| {
        let of = move || candidates().filter(move |s| s.external == external);
        of().find(|s| sub_traits(s).dub == dub_audio).or_else(|| of().next())
    };
    variant(true).or_else(|| variant(false)).map(|s| s.id)
}

/// The whole choice: [`pick_audio`], then [`pick_subtitle`] for the audio it chose.
pub fn select(audio: &[AudioIn], subs: &[Stream], anime: bool, lang: &str, remembered: &Remembered) -> Selection {
    let chosen = pick_audio(audio, anime, lang, remembered.audio);
    let dub_audio = anime
        && chosen.and_then(|id| audio.iter().find(|a| a.id == id)).is_some_and(|a| a.dub && lang_matches(lang, &a.lang));
    Selection { audio: chosen, subtitle: pick_subtitle(subs, lang, dub_audio, remembered.subtitle) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    const CASES: &str = include_str!("../../../../docs/marquee/track-selection-cases.json");

    #[derive(Deserialize)]
    struct Table {
        language: String,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        anime: bool,
        audio: Vec<CaseAudio>,
        subtitles: Vec<CaseSub>,
        #[serde(default)]
        remembered: CaseRemembered,
        expect: CaseExpect,
    }

    #[derive(Deserialize)]
    struct CaseAudio {
        id: i64,
        language: String,
        original: bool,
        dub: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CaseSub {
        id: i64,
        language: String,
        external: bool,
        forced: bool,
        hearing_impaired: bool,
        codec: String,
        #[serde(default)]
        title: String,
    }

    #[derive(Deserialize, Default)]
    struct CaseRemembered {
        audio: Option<i64>,
        subtitle: Option<serde_json::Value>,
    }

    #[derive(Deserialize)]
    struct CaseExpect {
        audio: Option<i64>,
        subtitle: Option<i64>,
    }

    fn stream_of(s: &CaseSub) -> Stream {
        Stream {
            id: s.id,
            lang_code: s.language.clone(),
            external: s.external,
            key: if s.external { format!("/library/streams/{}", s.id) } else { String::new() },
            forced: s.forced,
            sdh: s.hearing_impaired,
            codec: s.codec.clone(),
            title: s.title.clone(),
            ..Default::default()
        }
    }

    #[test]
    fn every_case_of_the_shared_table_holds() {
        let table: Table = serde_json::from_str(CASES).expect("the case table parses");
        assert!(table.cases.len() >= 20, "the table lost cases");
        for case in &table.cases {
            let audio: Vec<AudioIn> = case
                .audio
                .iter()
                .map(|a| AudioIn { id: a.id, lang: a.language.clone(), original: a.original, dub: a.dub })
                .collect();
            let subs: Vec<Stream> = case.subtitles.iter().map(stream_of).collect();
            let subtitle = match &case.remembered.subtitle {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(s)) if s == "off" => Some(RememberedSub::Off),
                Some(serde_json::Value::Number(n)) => Some(RememberedSub::Track(n.as_i64().unwrap())),
                Some(other) => panic!("{}: remembered subtitle {other}", case.name),
            };
            let remembered = Remembered { audio: case.remembered.audio, subtitle };
            let got = select(&audio, &subs, case.anime, &table.language, &remembered);
            assert_eq!(
                (got.audio, got.subtitle),
                (case.expect.audio, case.expect.subtitle),
                "case {:?}",
                case.name
            );
        }
    }

    fn track(id: i64, lang: &str, default: bool, title: &str) -> Stream {
        Stream { id, lang_code: lang.into(), default, title: title.into(), ..Default::default() }
    }

    fn countries(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_original_is_the_first_country_language_the_file_has() {
        let tracks = [track(1, "eng", true, ""), track(2, "fre", false, ""), track(3, "fre", false, "Commentary")];
        assert_eq!(original_audio(&tracks, false, &countries(&["France", "United States of America"])), Some(1));
        let dubbed = [track(1, "eng", true, ""), track(2, "fre", false, "")];
        assert_eq!(original_audio(&dubbed, false, &countries(&["France"])), Some(1));
        assert_eq!(original_audio(&dubbed, false, &[]), None, "no country, no opinion");
    }

    /// Exhuma (Plex Country "Republic of Korea"): the Korean track, not the English dub the account's
    /// language would otherwise pick.
    #[test]
    fn plex_country_spellings_name_their_language() {
        let tracks = [track(1, "eng", true, ""), track(2, "kor", false, "")];
        assert_eq!(original_audio(&tracks, false, &countries(&["Republic of Korea"])), Some(1));
        let tracks = [track(1, "eng", true, ""), track(2, "zho", false, "")];
        assert_eq!(original_audio(&tracks, false, &countries(&["Taiwan, Province of China"])), Some(1));
    }

    /// A country the table does not know, or whose language the file lacks: the file's own default
    /// (else first) track, never a dub-titled, commentary or description track.
    #[test]
    fn a_country_without_a_usable_language_falls_back_to_the_files_default_track() {
        let latvia = countries(&["Latvia"]);
        let tracks = [track(1, "eng", false, "English Dub"), track(2, "lav", true, ""), track(3, "eng", false, "")];
        assert_eq!(original_audio(&tracks, false, &latvia), Some(1));
        let tracks = [track(1, "eng", false, "Commentary"), track(2, "lav", false, ""), track(3, "eng", false, "")];
        assert_eq!(original_audio(&tracks, false, &latvia), Some(1), "the first when none is default");
        let only_dub = [track(1, "eng", true, "English Dub")];
        assert_eq!(original_audio(&only_dub, false, &latvia), None, "a dub is not the original");
        let dubbed = [track(1, "eng", true, ""), track(2, "fre", false, "")];
        assert_eq!(original_audio(&dubbed, false, &countries(&["Japan"])), Some(0), "no Japanese track in the file");
    }

    #[test]
    fn anime_without_a_country_falls_back_to_japanese_and_a_commentary_is_never_original() {
        let tracks = [track(1, "eng", true, ""), track(2, "jpn", false, "Commentary"), track(3, "jpn", false, "")];
        assert_eq!(original_audio(&tracks, true, &[]), Some(2), "the commentary is skipped");
        assert_eq!(original_audio(&tracks, false, &[]), None);
    }

    #[test]
    fn inputs_mark_every_other_track_a_dub() {
        let tracks = [track(1, "eng", false, ""), track(2, "jpn", true, "")];
        let inputs = audio_inputs(&tracks, true, &countries(&["Japan"]));
        assert_eq!(
            inputs.iter().map(|a| (a.id, a.original, a.dub)).collect::<Vec<_>>(),
            [(1, false, true), (2, true, false)]
        );
        let unknown = audio_inputs(&tracks, false, &[]);
        assert!(unknown.iter().all(|a| !a.original && !a.dub), "without a known original nothing is a dub");
    }
}
