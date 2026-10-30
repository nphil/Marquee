# Subtitles: styling, text handling, "Wrong subtitles?"

Mocks: `docs/marquee/mocks/07-subtitle-style.html`, `08-fix-subtitles.html`, `09-integrations.html`
(renders in `mocks/shots-subs/`, rebuilt with `node render-subs.mjs`). Same tokens and checks as the rest
of the Marquee design.

## The look

One stored setting per profile, `subtitle_look` (`plx_plex::plex::session::SubtitleLook`): preset, font,
bold, size, colour, opacity, edge and its width, background (none, line box, window) with colour, opacity,
corner radius and padding, anchor (bottom or top) and nudge, line spacing, Blend with video, Reveal as
spoken, and how authored ASS is treated. The Size, Position and Color ladders of the player's Style pages
stay and win: a ladder pick clears the matching field of the look and the preset reads Custom.

Built: Preset, Font, Edge, Background, Reveal as spoken and Blend with video on the player's Style pages,
all live, and Settings > Subtitles (`screens/src/settings_subtitles.rs`): every field as a row, with a live
preview. Rows step with LEFT and RIGHT (OK steps forward and wraps) and a pick goes through the same write door
as the Style pages. The mock's sliders and colour swatches are value read-outs in the shared table (`30%`,
`Yellow`, `4 px`): no shared row has a slider, and a page does not draw its own. The Preset row steps through
the presets instead of opening a list. The preview is the real renderer: the sample lines go through
`text_ass`'s converter and header, libass renders the whole 1920 by 1080 frame, and `AssSubtitles::draw_look`
scales it into the picture with the page's own look (a pick shows in the frame it is made), so size, line pitch,
placement, edge, box and the Blend tone are the player's arithmetic. The picture is a gradient (daylight,
night, snow) whose luma the Blend tone follows; a sixth sample is a script with a look of its own, which shows
what File styling does.

Two kinds of effect, by where they happen:

- **Restyle (libass, `text_ass::header`)**: font, weight, size, line pitch, hard edge styles, reveal tags.
  A change re-derives the events of the playing cue set.
- **Paint (`player::paint`)**: colour, opacity, the Blend tone, the soft diffuse shadow, the rounded
  box and window (measured from the ink of each line), anchor and nudge. Instant, no libass pass.

### Presets

| Preset | Modelled on |
|---|---|
| Blend with video (recommended) | Apple TV's default subtitles with the softened HDR white Infuse added after HDR glare complaints; level capped per ITU-R BT.2408 |
| Apple TV | tvOS Classic / Outline text styles |
| Classic | Broadcast closed captions (CEA-708 default: white on a black box) |
| Netflix | Netflix's Drop shadow edge |
| Outline | VLC's outline and tvOS Outline Text |
| Large print | tvOS Large Text |
| High contrast | Yellow on black, the long-standing low-vision caption choice |
| Marquee | The app's own Inter Bold with a thin outline (today's look) |

**Blend with video** follows scene brightness. The video sits on a hardware plane that GL cannot read,
so a frosted backing sampled from the picture is not possible; the tone follows a luma estimate (the
preview supplies the sample frame's, playback the HDR cap and the overlay gain) through an eased follower
with a dead band, so it never flickers.

### Reveal as spoken

Off, Word by word, Line by line. Per-word timing is estimated from the cue: syllable and character weights,
pauses at punctuation, about 10 % of the cue left as a tail, a soft fade per word. Karaoke (`\k`) in a file
is used exactly as authored. Exact word timing needs audio alignment on the server (for example WhisperX
on the NAS GPU); that is a later option and is not built.

## Text handling

- **Fonts**: Inter (Default), Noto Serif, Noto Sans Mono and Atkinson Hyperlegible (Regular and Bold each),
  all SIL OFL 1.1 (`THIRD-PARTY-NOTICES.md`), cut by `tools/cut-sub-families.py` into `pkg/fonts/subs/`.
  Every family falls back to the default family, which has the Noto script faces, the CJK face and the
  emoji fallback: no family draws a box where Default would draw a glyph.
- **Encodings**: BOM, UTF-8, UTF-16 and legacy code pages by detection (`sub_charset`).
- **Authored ASS** is kept as authored unless the look's `ass` field says `Mine` (Settings > Subtitles > File
  styling): libass (`ass_set_selective_style_override`, `plx_ass_set_style_override`, ABI 5) then replaces the
  font, size, weight and edge of every event that is not explicitly placed with the header's own, in white
  letters (the look's colour and opacity are painted over the frame as for a text subtitle), and inline
  `\fn \fs \c` tags are ignored on those events. A positioned event (a sign) keeps the file's look. A karaoke
  line keeps its timing and loses its two colours.

## Wrong subtitles?

Player > Subtitles > Wrong subtitles?:

1. **Remove this subtitle**: only for an external file (a sidecar). A first OK arms the row, a second
   sends `DELETE /library/streams/{id}`. An embedded track says it cannot be removed.
2. **Find another with Bazarr**: Bazarr finds the item (title, year, season and episode, file name), asks
   its providers (`/api/providers/movies|episodes`, tens of seconds), lists score, language, provider and
   the hearing-impaired and forced flags, downloads the pick (`POST /api/providers/...`), asks PMS to
   refresh the item and selects the new track when it appears. Every step has its own state: not set up,
   searching, no results, not reachable, key refused, not in Bazarr, downloading, added.

Bazarr's address and API key live in the protected half of the session record, like the Plex tokens, and
reach only the one kind of request that needs them. `http://` is accepted only for home-network hosts.
They are loaded once from a file with `tools/import-bazarr-link.sh` (`plex::session::bazarr_import`: a
`0600` file in the runtime directory, read once at start, then deleted). The typed entry is Settings > Integrations > Bazarr (mock `09-integrations.html`;
`screens/src/settings_bazarr.rs`): the television's own keyboard (the one Search raises), the address in the
clear and the key as dots, Test connection, Forget. The page never reads the stored key: the app answers
`PreferenceCmd::BazarrRead` with the address and whether a key is set, and Test connection reads the stored
pair on the app's side.

## Extension points

`plex::bazarr` takes a `Transport`, so a Sonarr or Radarr client ("get another version of this file") is a
second client over the same transport and the same link type, with its own job kind beside
`Work::Bazarr*` in `data::subsearch`. Nothing of it is built.

## Sources

- 9to5Mac, tvOS 26.4 subtitle styles: <https://9to5mac.com/> (Classic, Large Text, Outline Text, Transparent Background)
- Firecore forum, Infuse HDR subtitle brightness
- How-To Geek, 2026-10-28, HDR subtitle glare
- ITU-R BT.2408, reference white for HDR graphics
- CEA-708 caption defaults; Netflix timed-text style guide (drop shadow); VLC outline
- Bazarr source, `bazarr/api/` (read 2026-10-30)
