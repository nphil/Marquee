# Marquee: TV status and maintenance

## Resume here

Stable 0.10.0 is installed on `192.168.1.242` (webOS 10.2.1, platform `o22n2`). The existing
installer and payload deployment retained `com.beb.plxnative` and its storage identity; all 54
deployed files matched. Runtime build `37d8d0f4` includes banner prewarming and enhanced-audio startup
recovery. The versioned IPK and `marquee-latest.ipk` are staged in `/data/home/Marquee-release/`, with
SHA-256 `c9e95defb396b62062fd7405551606c135d4da318a694fb28ddfe420162b7812`.

The initial focused feature run passed 93 checks; four privacy identifier/preview-isolation checks
also passed. ARM ELF and package checks passed for the installed build. The stored account opened
Profiles, Home, Settings and About. Native keyboard input and Bazarr 1.6.2 connectivity were observed;
the key remained masked. The movie hold-menu displayed Delete; no deletion was submitted. Instant
D-pad seeking is Off for the active profile. The Apothecary Diaries S1E5 resumed at 193 seconds with
dialogue enhancement enabled: the selected output was H264/Opus, its actual Opus header supplied two
channels, and decoded video advanced. Arrow-key seeking displayed a Plex scene still; Back cancelled
the preview. Playback stopped cleanly, the video-plane list became empty, and the UI retained the
120 Hz runtime setting. Actual media deletion was not tested.

## Proven local paths

- Reuse `/data/home/Marquee-subs`, its vendor outputs and Cargo targets, and
  `/tmp/subs-work/arm-build.sh`; local prerequisites and deployment caveats are in
  `/data/home/Marquee-BUILD-NOTES.md`. Preserve stable/release/SDK/bfd-remap flags on make commands.
- Reserve and release the TV from the same build worktree with `tools/tv-lock.sh`.
  `make ... run` uses the stored account without embedding a Plex token.
- Capture this webOS 10 set with `python3 /tmp/lt/shot.py NAME 1920 1080`; the repository capture
  script uses an older service and macOS tooling. `/tmp/marquee-ui-kb.py` sends native remote key
  events through the existing SmartRemoteRCU device; `/tmp/lt/kb.py` is the SSAP remote helper.

## Frame pacing and hardware

Active Marquee navigation measured about 60 display flips/s at the compositor’s default divider,
and 119.7/s at runtime `refresh 120`, with an 8.3 ms 99th-percentile interval during steady motion.
SDL already requests VSync with swap interval 1; the compositor pacer uses actual display-flip phase.
The banner fix prewarms incoming text through the existing owned queue without presenting. Its
automatic-transition sample had only 8.3 ms intervals; a six-second manual-navigation sample still
contained one 16.7 ms interval near the motion-to-idle boundary. This is not proof of zero stutter.
Idle-to-active gaps and lifetime-average flip counts are not animation FPS. The client frame callback
is acknowledged at commit; its FPS counter does not prove unique pictures reached the display.

Automatic foreground-only 120 Hz is not installed. The single approved R2 platform attempt stopped
in read-only preflight before mutation; R2 and its consumed approval must not be replayed. Its
observed dispatcher SHA-256 `7631005064da542f26f644ae1b636597e729884c4288d996c027b60185f366cf` matches
the accepted wrapper-package verification record and the review-only dispatcher reference.
The corrected packet is `/data/home/tmp/lumatv-f5-renderer-packet-r3/renderer-entry-r3.py`, SHA-256
`e3f1642c10f513f53e0e05406af2cb3b8dad41b82a2c2578b18e87de2f42e666`. Its only entry change is that
one dispatcher pin; actual local `--inspect` returned `ok: true`. One corrected Face ID request
was subsequently permitted but remains unsent; this is not a standing grant or authority to change
the packet. The TV lease is released for human use; no controls or restarts until a fresh window.

The read-only TV baseline matched all six dispatcher dependencies and all 35 renderer members.
R3 still rejects the observed supervisor argv `/usr/bin/surface-manager -platform eglfs_webos`
and nonempty `LD_PRELOAD`/`LD_LIBRARY_PATH`; loader-environment provenance is unestablished. Home
retains HDMI2 TV/avconnector records, including six resources. The no-media guard permits only
empty `vtclient` records, not empty records of any type. Home foreground does not establish idle.
Space and remaining runtime predicates are unestablished. Do not weaken guards, close input
resources or replay the broader wake installer to force this update. Reuse an already-proven
renderer-only route within the approved scope. No platform mutation occurred; runtime 120 Hz
remains temporary rather than a persistent application preference.

Rendering already uses GLES and reused textures; two bounded workers fetch and decode posters.
A measured late frame combined native GPU work with a preceding 4.1 ms hidden-shell GPU job;
the shell job alone is not established as the cause. Do not change callback policy or force GPU
synchronization to diagnose it: the existing scheduler trace and compositor ring are sufficient first.

The native video engine supplies rational source rates to LG’s separate video plane. Preserve that
path rather than writing panel-rate controls. The fastest OSD divider is approximately 120 Hz in
the 60-family and 100 Hz in LG’s 50-family; `refresh 120` does not change the panel’s video clock.

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
- Cold enhanced startup waits nonblockingly for the selected demux output metadata. OpusHead carries
  channels, mapping and pre-skip; original media channels must not be guessed for transcoded output.
  A refused cold enhanced start retries plain playback once at the retained position. Deferred unloads
  retain that recovery owner; a refused plain start becomes Failed even before native Ready. Seven
  focused cold-start/Opus checks passed; the successful enhanced path was exercised on the TV.
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
