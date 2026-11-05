# Marquee changelog

## Unreleased

- The launcher dock gets a native 240 px `extraLargeIcon`, so a 4K dock tile is no longer an upscaled 130 px icon. `tools/import-bazarr-link.sh` now gives the staged file the owner of the app's jail directory, which the jailed app requires to read it.
- The detail page, the info card and the audio menu label subtitles "Subtitles", subtitles for the deaf and hard of hearing "Captions" and audio description "Audio description" in plain words instead of the CC, SDH and AD marks, in all eleven languages. The Tracks panel keeps its technical labels.
- Automatic subtitles follow the audio: English audio prefers the dub transcript (an SDH/CC track, a `.en.cc.srt` sidecar, or one titled Dub/Dubtitle), other audio the full English translation, a downloaded one over a built-in one. The subtitle Plex has selected follows like any other automatic pick, an audio switch re-picks unless the viewer chose a subtitle or one remembered for the show decided it, and a playback that starts on remembered audio starts on the matching subtitle. Subtitle menus name tracks in plain words: "English", "English (dub)", "English (on-screen text)", with no format names, acronyms or file titles.
- A built-in subtitle track that a downloaded subtitle of the same language and kind duplicates is left out of the Subtitles menu; a selected or remembered built-in track, and the forced track "On-screen text only" draws, play as the downloaded twin, and a built-in track with no twin stays listed.
- The player's Subtitles menu and settings call forced subtitles on-screen text: the "On-screen text only" row and the On-screen text setting ("Show subtitles only for signs and songs"). Audio rows show the language, "(commentary)" and the channel layout, with no codec names or file titles.
- Playback preserves non-standard picture aspect ratios by default. More offers Fit, Fill/crop, Original size, Zoom and Stretch without restarting playback.
- Bazarr subtitle activation reports confirmed progress through Plex discovery and loading to Active or a typed error; metadata rechecks do not repeat downloads.
- Sidecar readiness requires a positive-duration cue and uses epoch-tagged renderer publication.
- Stats distinguishes measured and estimated rates, supports the profile menu, and offers separate session-only opt-in numeric logging.
- Stats samples native display sync separately from video and app rates, and clears unreadable or stale output rather than retaining it as live.
- Watched-state refreshes settle presented and covered detail pages, including failed reads, without leaving the loading indicator active.
- Returning from playback retains visible artwork within the existing cache limits; fast scrolling prioritizes currently visible uploads.
- Reveal as spoken can use verified SpeechDock audio timing while retaining authored cue bounds and karaoke; estimated or revised sources cannot retain stale aligned words.
- Word-by-word audio alignment now waits for the first word's verified onset, without changing the subtitle's authored interval or estimated fallback.
- Player Info distinguishes SpeechDock word intervals actually applied to the current cue from estimated or unused timing, clearing stale status across renderer and playback changes.
- SpeechDock requests preserve millisecond-authored cue bounds, skip ineligible long cues without rejecting the batch, and start correctly during the first 15 seconds of playback.
- A sidecar subtitle with SpeechDock word timing is rebuilt only when an answer changes some cue's words, not on every repeated request during dialogue, and its estimated script is reused while the subtitle style stays the same.

- Dialogue Enhance uses the output audio layout and recovers playback without enhancement if the enhanced stream cannot start.
- Dialogue Enhance on an Opus track whose source fails to open no longer ends in an error: a cold start retries without enhancement, and any other failed open goes through the usual recovery (HLS fallback or rollback). A slow open is no longer reported as a Load timeout.

- Home prepares the next banner's text during idle time, moving cold text work out of banner transitions without redrawing the settled screen.
- The playback-repair help note fits in every language again (shorter troubleshooting link), and the Russian privacy title and French consent row fit their columns.

## 0.10.0

- Rosé Pine folded-M icon and splash; the package title controls display names across the app and launcher.
- Bazarr accepts the native keyboard’s literal address and key text, retains invalid drafts, and excludes typed text from diagnostics.
- Dialogue enhancement rejects unusable audio decisions and restores the previous playback route if the replacement fails.
- The first Left or Right press opens a scene-thumbnail seek preview; OK seeks and Back cancels. Instant D-pad seeking remains optional.
- The hold-menu deletes movies, episodes and whole shows after confirmation, with Plex Watchlist cleanup and explicit partial-failure reporting.
- About, support and privacy notices use fork links and current data disclosures.

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
