# Marquee: TV status and maintenance

## Resume here

Stable 0.10.0 is installed on `192.168.1.242` (webOS 10.2.1, platform `o22n2`). Runtime source
`fce9888a` adds confirmed subtitle activation and separate Stats logging to the earlier banner and
enhanced-audio fixes. The stable `com.beb.plxnative` storage identity is retained; all 54 deployed
files match. The versioned IPK and `marquee-latest.ipk` in `/data/home/Marquee-release/` are
20,970,244 bytes, SHA-256 `0cb296dffd9652981d593b45692980944add0d94da40b4157e2dfe46df979afb`.
The previous package is retained as `marquee-0.10.0-37d8d0f4-rollback.ipk`.

Native Bazarr search returned English provider results. The player Stats read-out showed the
23.976 fps file rate separately from GL swaps and unverified physical refresh; detailed logging
remained Off. New subtitle downloading and actual media deletion were not exercised on the TV.
The app ended on Home without the Stats overlay and with zero video planes; the TV lease was
released and LumaTV notified. No system, refresh, input or power setting changed in this window.
Captures are `/tmp/lt/marquee-fce-*.png`; scrubbed logs are `/tmp/marquee-fce-scrubbed/`.

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
  script uses an older service and macOS tooling. `/tmp/marquee-ui-kb.py` lives on the TV and sends
  native SmartRemoteRCU events: invoke `/usr/bin/python3 /tmp/marquee-ui-kb.py` through `/tmp/lt/tv.py`.

## Frame pacing and hardware

Active Marquee navigation measured about 60 display flips/s at the compositor’s default divider,
and 119.7/s at runtime `refresh 120`, with an 8.3 ms 99th-percentile interval during steady motion.
SDL requests VSync with swap interval 1 but does not check whether the request succeeded. Effective
client swap blocking remains unverified; the compositor pacer uses actual display-flip phase.
The banner fix prewarms incoming text through the existing owned queue without presenting. Its
automatic-transition sample had only 8.3 ms intervals; a six-second manual-navigation sample still
contained one 16.7 ms interval near the motion-to-idle boundary. This is not proof of zero stutter.
Idle-to-active gaps and lifetime-average flip counts are not animation FPS. The client frame callback
is acknowledged at commit; its FPS counter does not prove unique pictures reached the display.

The 2026-11-01 read-only window captured 6.003157 seconds with zero scheduler overruns: 718 draws,
717 flips and two 16.7 ms flip intervals among continuous 8.3 ms output. Compositor render p50/p90/max
was 422/460/2900 microseconds; draw-to-GPU-fence was 5.4/5.6/8.0 ms. The pre-capture divider read
120 Hz and one video plane existed. No mode, input, power, deployment or restart command was sent.
The ring and trace are `/tmp/marquee-readonly-20261101-ring.json` and
`/tmp/marquee-readonly-20261101-sched.gz`; the lease was released and LumaTV notified.

Both long intervals had zero late/lost/missed counters; those counters assess landed-frame timing,
not every display refresh. One gap overlaps distributed runnable-off-CPU time, the other a poll
sleep with an unresolved wake source. Neither proves a particular GPU job or callback caused it.
The next causal capture needs matching app-buffer, commit and flip identities. Do not infer GPU
durations from overlapping Mali slot starts/ends, change callback policy or promise zero stutter.

Automatic foreground-only 120 Hz is not installed. The single approved R2 platform attempt stopped
in read-only preflight before mutation; R2 and its consumed approval must not be replayed. Its
observed dispatcher SHA-256 `7631005064da542f26f644ae1b636597e729884c4288d996c027b60185f366cf` matches
the accepted wrapper-package verification record and the review-only dispatcher reference.
The corrected packet is `/data/home/tmp/lumatv-f5-renderer-packet-r3/renderer-entry-r3.py`, SHA-256
`e3f1642c10f513f53e0e05406af2cb3b8dad41b82a2c2578b18e87de2f42e666`. Its only entry change is that
one dispatcher pin; actual local `--inspect` returned `ok: true`. One corrected Face ID request
was subsequently permitted but remains unsent; this is not a standing grant or authority to change
the packet. The later coordinated read-only capture completed and the TV lease was released again.
No platform request is pending; the disconnected HomeLabber gate and live compatibility guards remain.

The later offline candidate is `/data/home/tmp/lumatv-f5-renderer-packet-r4`. R4 has new argv, loader
environment and ELF-provenance checks and is not approved or installed. The retained HDMI profile
is classified as occupied and needs a separate explicit input/audio-interruption scope. The unused
R3 permission and the app-only update window do not authorize R4.

The newer UI-feature compositor candidate is
`/data/home/tmp/lumatv-foreground-pacing-20261031/lumatv-compositor`, SHA-256
`904c8e93a985aa25feb07afac8fdde2f0796b67317ac431ef86277e4edbcb6c0`. This uninstalled build is
superseded: window-type changes and restores from minimized to `Maximized`/`Default` omit policy
recomputation. A repaired build, replacement artifact hash and independent review are required
before installation. The intended policy selects nominal 120/divider 1 for exact foreground
`com.beb.plxnative`; other foreground
restores the startup `--refresh` base, default 60. Numeric `ctl refresh 60/120` overrides remain
sticky; `ctl refresh auto` resumes foreground policy, while `ctl refresh reset` only resets
measurements. This artifact is distinct from the previously pinned legacy payload: independent
review and joint maintenance agreement remain required, with no implicit package substitution.
Divider 1 follows LG’s 100 or 119.88/120 family; independent physical 120 over the 100 family and
optical seamlessness remain unestablished.

The read-only TV baseline matched all six dispatcher dependencies and all 35 renderer members.
R3 still rejects the observed supervisor argv `/usr/bin/surface-manager -platform eglfs_webos`
and nonempty `LD_PRELOAD`/`LD_LIBRARY_PATH`. Historical LG startup uses those argv flags; exact
loader-environment provenance is still unestablished. Home
retains HDMI2 TV/avconnector records, including six resources. The no-media guard permits only
empty `vtclient` records, not empty records of any type. Home foreground does not establish idle.
Space and remaining runtime predicates are unestablished. Do not weaken guards, close input
resources or replay the broader wake installer. The successful R8 record
`/tmp/lumatv-historical-stage.TyUJ9M4D/wake-maintenance-evidence/events.jsonl:23-54`
proves the renderer steps inside `wake-maintenance.py:renderer_cutover(ctx)`, not a standalone
host launcher. It installed baseline `6fe8bae`, not candidate `f5c864`. Reuse those proven steps
within a compatible scoped entry; the broader wake/trial/coldboot workflow is outside this task.
No platform mutation occurred; runtime 120 Hz remains temporary rather than persistent.

Rendering already uses GLES and reused textures; two bounded workers fetch and decode posters.
A measured late frame combined native GPU work with a preceding 4.1 ms hidden-shell GPU job;
the shell job alone is not established as the cause. Do not change callback policy or force GPU
synchronization. The bounded ring and scheduler trace do not yet link app buffers to display flips.

The native Qt-host default-GPU-wallpaper probe reported unchanged settled counters over 14–18 s:
Home frameSwapped 109→109, system 8→8 and nested frameTick 61→61, with hostActive false and
idleHold true. Quickpanel and keyboard navigation remained usable. This does not establish the
TV GPU-slot cause or behavior for video wallpapers and all configurations; hidden-shell production
code remains unchanged.

The existing C4 graphics research is under
`/tmp/lumatv-historical-stage.TyUJ9M4D/docs/research/`. In `scanout-research.md:11–22,84–103`,
OSD1 aliases OSD0 rather than providing a second simultaneously blended graphics layer; LG video
windows are not general RGBA overlays. Its default-off `--scanout direct` path can omit a GL copy
for a compatible sole fullscreen client, with composition required when other layers need the screen.
That archived report leaves on-set display validation pending. `rustui-result.md:65–118` records
the older Rust-client flip-copy comparison; the upside-down no-copy diagnostic is not proof of a
viable display path. Check the current baseline before treating these as new optimizations; neither
report identifies the current two frame gaps, and LG retains native video-plane ownership.

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
timings. The installed app does not use a server alignment or timing-result endpoint. The 2026-10-31
live Unraid inventory included Bazarr but no Whisper/SubGen alignment container; GPU hardware and spare
capacity were not established. The proposed offline alignment path is in `design-subtitles.md`.
The separate speech-alignment effort is not included in this app update; its eventual merge must
preserve the current sidecar lifecycle and ABI 6 renderer-readiness contract.

The existing subtitle/Bazarr images in `docs/marquee/renders/` are desktop simulator frames under
Xvfb, not TV captures. The shared table still uses value read-outs rather than mock sliders/swatches.
