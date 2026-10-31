# Marquee: TV status and maintenance

## Resume here

Stable 0.10.0 is installed on `192.168.1.242` (webOS 10.2.1, platform `o22n2`). The existing
installer and payload deployment retained `com.beb.plxnative` and its storage identity; all 54
deployed files matched. The runtime code is from `7531f3c0`, with package metadata from `572946d9`.
The versioned IPK and `marquee-latest.ipk` are staged in `/data/home/Marquee-release/`.

The focused host run passed 93 checks; ARM ELF and package checks passed. On the TV, the stored
account opened Profiles and Home, remote navigation reached Settings, native keyboard text reached
the Bazarr address draft, and Test connection reported Bazarr 1.6.2 connected. The key remained masked.
Instant D-pad seeking is Off for the active profile. Playback and actual media deletion were not tested.

## Proven local paths

- Reuse `/data/home/Marquee-subs`, its vendor outputs and Cargo targets, and
  `/tmp/subs-work/arm-build.sh`; local prerequisites and deployment caveats are in
  `/data/home/Marquee-BUILD-NOTES.md`. Preserve stable/release/SDK/bfd-remap flags on make commands.
- Reserve and release the TV from the same build worktree with `tools/tv-lock.sh`.
  `make ... run` uses the stored account without embedding a Plex token.
- Capture this webOS 10 set with `python3 /tmp/lt/shot.py NAME 1920 1080`; the repository capture
  script uses an older service and macOS tooling. `/tmp/lt/kb.py` is the proven SSAP remote helper.

## Input and private configuration

- Native SDL text commits must reach owned Settings, not only Search. Addresses and keys use literal
  text editing, not Search prediction replacement; printable text must stay out of diagnostics.
- On this firmware, the first Back dismisses the OS keyboard; the next Back cancels the active
  draft and restores the saved value. Native typing and that two-step cancellation were observed.
- `tools/import-bazarr-link.sh` stages a one-shot file. It must be mode `0600` and owned by the
  running app’s effective UID, not the root SSH account. Resolve the exact app process through
  `/proc/*/exe`; its path is inside `/var/palm/jail/<id>/`. Do not hard-code the UID or relax checks.
  Successful import saves through the protected session service and deletes the file.

## Playback and media constraints

- Enhancement uses one native playback engine: successful route changes reload it; failed trials
  restore the prior route and position. The recovery is not seamless or a second decoder.
- Scene stills use authenticated Plex Part-index endpoints, not the photo transcoder. The image
  cache is memory-only; missing generated indexes show no still. The request deadline does not
  interrupt synchronous DNS resolution.
- Delete uses the existing hold-menu and an exact-scope confirmation. Membership changes follow
  confirmed deletion only, including retained Search, Person and Collection screens. Watchlist
  removal uses Discover identities, not local rating keys; uncertain legacy mappings are reported.

## Speech timing and render provenance

Reveal as spoken estimates word timing from cue text and duration; authored ASS karaoke keeps its
timings. No server audio alignment or timing-result endpoint exists. The 2026-10-31 live Unraid
inventory included Bazarr but no Whisper/SubGen alignment container; GPU hardware and spare capacity
were not established. The proposed offline alignment path is in `design-subtitles.md`.

The existing subtitle/Bazarr images in `docs/marquee/renders/` are desktop simulator frames under
Xvfb, not TV captures. The shared table still uses value read-outs rather than mock sliders/swatches.
