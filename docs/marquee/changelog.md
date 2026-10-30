# Marquee changelog

## 0.9.3

- Settings > Subtitles: every part of the subtitle look as a row (preset, font, size, colour, opacity, bold,
  edge and width, background box, position, line spacing, Blend with video, Reveal as spoken, file styling),
  stepped with left and right, beside a live preview that draws sample lines with the player's own renderer.
- Settings > Integrations > Bazarr: the address and API key typed on the television, Test connection and
  Forget. The key is masked and never logged.
- File styling "My look": a subtitle file's own fonts, sizes, colours and edges are replaced by the look;
  signs and positioned lines keep the file's.

## 0.9.2

- Subtitle looks: a preset, font, edge, background and "reveal as spoken" on the player's Style pages
  (Preset, Font, Edge, Background, Reveal as spoken, Blend with video), saved per profile.
- Blend with video preset: soft translucent text that follows the scene and stays below HDR peak.
- Bundled subtitle fonts: Noto Serif, Noto Sans Mono and Atkinson Hyperlegible, plus an emoji fallback.
- "Wrong subtitles?" in the player's Subtitles menu: remove an external subtitle from Plex, or find another
  with Bazarr (score, language, provider, hearing-impaired flag), download it and switch to it.
- Bazarr address and API key stored in the protected session record; `tools/import-bazarr-link.sh` loads
  them once from a key file.
