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

The unshipped Stats change samples native `osd_irq` counts off the UI thread only while the overlay
is visible, showing approximate Hz under `measured display sync`. Video-source rate and returned
GL swaps remain separate; reset, unreadable and stale counters do not produce live readings.
TV app-jail access and actual playback verification remain pending; no live panel raster or final
output HDR field is inferred from video metadata, placed video rectangles or static capabilities.

The same unshipped source retains the last drawn artwork viewport through playback and return,
within the existing 64 source slots, render-scaled 44 MiB texture budget and 8 MiB decode-admission
threshold. Current drawn demand takes upload priority. Watched-state read-back gives the visible
detail owner the shared metadata slot and publishes terminal failures while retaining loaded data.
Focused local consumer/cache regressions pass; actual household reproduction remains unobserved.

Captures are `/tmp/lt/marquee-fce-*.png`; scrubbed logs are `/tmp/marquee-fce-scrubbed/`.

Detailed stats logging retains its own explicit session-only app opt-in, off at boot. LumaTV’s
supported read-only `GET /v1/logging` reports saved `base.logging.enabled` for optional remote UDP
forwarding only; `active` means the worker is configured, not receiver delivery. LumaTV’s bounded
local collection is independent. Neither these flags nor Stats visibility authorize Marquee capture;
no supported external-app capture/submission contract exists. Private collector sockets/files are
not an app integration API.

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

The 2026-11-01 saved capture spans 6.003157 seconds with zero scheduler overruns: 718 draws and
717 flips. Recorded intervals are 680 × 8.3 ms, 35 × 8.4 ms and two × 16.7 ms; p50/p95/p99 are
8.3/8.4/8.4 ms at the ring’s 0.1 ms precision. Render p50/p90/max is 422/460/2900 microseconds;
draw-to-output-fence is 5.4/5.6/8.0 ms. The complete 29.56-second ring includes ten gaps above
10 ms and is not the same window. The pre-capture divider read 120 Hz and one video plane existed.
No mode, input, power, deployment or restart command was sent; the lease was released.
Sources are `/tmp/marquee-readonly-20261101-{ring.json,sched.gz,capture.log}`; corrected parser
output is `/tmp/marquee-readonly-20261101-saved-analysis.json`.

All 719 `osd_irq` events remain continuous through both gaps: median 8.343 ms, maximum 8.357 ms.
The first gap overlaps 8.456 ms of app runnable/off-CPU time; the second has only 0.098 ms and
several poll sleeps. No joined app-buffer/COMMIT → selected draw → output fence → flip identity
exists, so these records do not justify an app-only pacing patch or a zero-stutter claim.

Identity-bearing GPU events attribute the approximately 4.134 ms interval to the compositor, not
the hidden Qt shell. Submission-to-completion spans include queueing and resubmission, not
exclusive GPU execution. The existing `analyse.py` one-job-per-slot tracker loses ownership on
overlapping submissions; its GPU-busy totals are not causal evidence for this capture. A fresh
passive identity-observation window must be separately coordinated; repeating scheduler-only
capture cannot establish the missing buffer/fence links. Preserve native COMMIT callbacks.

Automatic foreground-only 120 Hz is not installed. The single approved R2 platform attempt stopped
in read-only preflight before mutation; R2 and its consumed approval must not be replayed. Its
observed dispatcher SHA-256 `7631005064da542f26f644ae1b636597e729884c4288d996c027b60185f366cf` matches
the accepted wrapper-package verification record and the review-only dispatcher reference.
The corrected packet is `/data/home/tmp/lumatv-f5-renderer-packet-r3/renderer-entry-r3.py`, SHA-256
`e3f1642c10f513f53e0e05406af2cb3b8dad41b82a2c2578b18e87de2f42e666`. Its only entry change is that
one dispatcher pin; actual local `--inspect` returned `ok: true`. One corrected Face ID request
was subsequently permitted but remains unsent; this is not a standing grant or authority to change
the packet. The later coordinated read-only capture completed and the TV lease was released again.
The failed R2 receipt does not cover the later packet; no valid current-packet approval is established.

R4 is reported frozen offline at `/data/home/tmp/lumatv-f5-renderer-packet-r4`, PREPARED NOT RUN.
Entry SHA-256: `3e5fd3953c3c67aae80d849b0ab6187f7c3df9300d1336104b0d86614a41cd5b`.
`scope-receipt-r4.json` SHA-256:
`b006de5f91f6f88dfa5cfc06af322ffee5141ffdbfdd4296c6f0e10b16537c37`.
`execute-command.txt` SHA-256:
`c4596739ce95df9eca054f4df94ac85c0f9152536b09a7aa7b461d78be977735`.
The packet retains the renderer/R8/f5 and rollback payloads; frozen R2/R3 remain unchanged. The
retained HDMI profile is OCCUPIED and requires explicit possible input/audio-interruption scope.
Unused R3 permission and app-update authorization do not authorize R4.

The final reported legacy compositor candidate is
`/data/home/tmp/lumatv-foreground-pacing-20261031/lumatv-compositor-legacy`, SHA-256
`f5c864ae1e869da3e2f60af14faca77ef51664254c8043652dff84a2fbf3463a`. LumaTV reports clear
independent local review after protocol-type/title, input-panel and minimize/restore boundary fixes.
Hardware installation and acceptance remain pending; the old UI-feature `904c8`/`9762` artifacts
are superseded and must not be installed.

Exact foreground `com.beb.plxnative` selects nominal 120/divider 1; other foreground restores the
startup `--refresh` base, default 60. Numeric `ctl refresh 60/120` overrides remain sticky;
`ctl refresh auto` resumes policy and `ctl refresh reset` is measurement-only. Maintenance hand-back
must verify the normal base is restored outside Marquee, not leave a global manual 120 setting.
Source routing selects the newest mapped, non-minimized nongroup card; overlay, volume, toast,
keyboard, ShellSystem and cover windows do not replace it. Menus over Marquee inherit its nominal
120 target; over Home they inherit startup base 60. This is not universal 120 Hz animation.
Qt Shell has a nominal 16 ms basic timer and selected frame-driven paths; Rust UI’s initial
60 Hz scene hint is not a proven global cap. Divider 1 follows LG’s 100 or 119.88/120 family;
independent physical 120 over the 100 family and optical seamlessness remain unestablished.

A fresh agreed TV window and separately authorized exact compositor/Rust-bundle restart with
pinned rollback through the proven renderer transport are required. App-update permission does
not cover this maintenance, and no approved payload may be substituted implicitly. No TV lease
is held and no new HomeLabber request will be sent.

Frozen local proofs do not establish live TV state. Fresh vendor-header, hash, complete loader-
environment and inode checks, plus hardware acceptance, remain unobserved. Local settlement and
rollback stand-ins do not establish device recovery. Reuse the historical `renderer-fragment.py`
cutover, not the enclosing wake-policy/trial/cold-boot workflow. The separate movie picture-mode
arrow/selection fix is outside frozen R4.

The platform owner reports no first request for the final packet and no evidence of a matching
pending or execution-unknown action. Missing optional `approval_list_requests` does not prevent
a first request. The operative blocker is Nitin’s explicit no-new-HomeLabber-requests instruction
in the platform chat; coordination messages do not lift it. Only new direct authorization from
Nitin can permit the exact maintenance request. Existing wake-read grants must not be reused or
shared for installation.

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

The current source-only packet is `/tmp/lumatv-r28/docs/research/framerate-result.md:9–21`.
Its primary map covers scanout, complex Home load and native commit-callback constraints; AFBC
import is not proof of scanout compatibility. Reuse the existing LumaTV
`perf/{ringsum.py,frametl.py,frametl_parse.py,analyse.py}` with the saved capture pair above.

`Native120PacingRoleRetry` completed the local watched-spinner and bounded artwork-residency fixes.
`Native120SavedTraceCausality` completed the saved-trace analysis; no Slow worker remains active.
Normal role routing must finish its configured fallback chain before a worker is declared unavailable.

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

The installed build estimates word timing from cue text and duration; authored ASS karaoke keeps
its timings. The isolated SpeechDock source return `1b0e4e5c3ea32f38f94525aca7a89457f9dec963` is
integrated into main but is not yet installed or activated on the TV.

SpeechDock session `01a1943c-9834-7106-872e-e017bd1c394b` owns the CPU backend. Client source
ownership has returned to Marquee; `/data/home/Marquee-speechdock-alignment` remains frozen. Its
private `CARGO_HOME` is `/data/home/tmp/marquee-speechdock-alignment-cargo` and
`CARGO_TARGET_DIR` is `/data/home/tmp/marquee-speechdock-alignment-target`, separate from the
cached `/data/home/Marquee-subs` deployment lane. Integration follows the main-only convention.

The client uses protocol 1 `POST /api/alignment/windows`, off the playback thread. SpeechDock
verifies English audio independently with a local quantized CPU speech model; word boundaries use
a 20 ms grid within the authored cues. Production activation requires a trusted
`SPDOCK_ALIGNMENT_PLEX_URL`, private `SPDOCK_ALIGNMENT_PLEX_TOKEN_FILE` and read-only
`SPDOCK_ALIGNMENT_ROOTS` mappings. The profile integration defaults Off; missing or unverified
audio retains explicitly estimated timing rather than a claim of audio alignment.

ABI 6 epoch-fenced renderer readiness, truthful player activation, visual modes and authored ASS
karaoke remain required. Embedded alignment capture and publication require the exact displayed
source ID and revision to be renderer-ready. Estimated cues remove earlier aligned words; a missing
or changed verified backend source revision invalidates retained timings, while an unchanged
revision can retain valid out-of-window words. Requests are cancelled or ignored when their owning
media/audio/subtitle selection changes. Backend deployment and TV verification require a separate
explicit handoff; no new HomeLabber requests are authorized.

The existing subtitle/Bazarr images in `docs/marquee/renders/` are desktop simulator frames under
Xvfb, not TV captures. The shared table still uses value read-outs rather than mock sliders/swatches.
