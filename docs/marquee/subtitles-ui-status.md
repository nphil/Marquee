# Marquee: TV status and maintenance

## Acceptance history

The installed build and its latest on-screen checks are in `/data/home/Marquee-BUILD-NOTES.md`
("Installed now"). The sections below run up to 2026-11-02.

### Subtitle-safe volume popup

Stable 0.10.0 source `b506a7ae` is installed, including the first-word reveal fix.
The executable is `40f426110c961365e5bfb69eb635ee0343d0948cf744718a710645f225bb5f13`;
all 54 deployed files match. Stable ARM ELF, package and link-input checks pass.
Actual TV acceptance covers the owner profile, Home, episode playback, subtitles
and pause/resume. Verified acoustic first-word timing on the TV remains unproven;
the 17 focused/native ASS onset, fade and backward-seek checks are separate evidence.

Caption compositor `6dab7dcd` and shadow-aware native UI `21255466` are installed.
Painted subtitle bounds accompany the same Wayland surface commit as the picture;
unchanged bounds send no message. There is no screen scan, media capture, polling
loop or additional commit. Current-frame and queued-page guards remain enabled.
The renderer sync patch preserves the shared painted extent; `host.rs` stays local.

Actual TV playback shows the bottom Dolby Audio popup, its full shadow and
`PCM · Dolby Surround` above subtitles at `{x:594,y:938,w:740,h:55}`, with no overlap.
Six focused placement/native GL checks cover normal, deep and flat materials and
changing caption geometry. Runtime UI identity matches the canonical child’s
executable mapping, installed inode and SHA256, rather than the explicit loader.

Proof: `/data/home/tmp/marquee-caption-visible-proof/`, including
`ui-install-after-handoff.json`, `screen.json` and `captionpopup40f.png`; the 54-file
receipt is in `/data/home/tmp/marquee-caption-final-proof/app-deploy.log`.
At 01:27 EST on 2026-11-02, owned playback was closed and Active Standby verified,
failures were 0, temporary OLED pixel brightness 10 was restored to the original 84,
and the local mirror and owned remote token were absent. Intentional receiver
unmute was preserved; the TV window was explicitly returned to the other sessions.

### Audio and skip-settings update

Stable 0.10.0 source `c2f01107` is installed, with all 54 payload hashes matching.
The running release executable matches SHA-256
`2ee10cb5add80f979cf050d5172d6d10af1020de92a8fe5301b85d4ad8e88c62`.
Actual TV preferences retain English audio, audio memory On, manual intro skipping
and a five-second credits countdown. Audio memory completed an On/Off/On round trip;
both timer pickers expose 3/5/10/15/30 seconds. Marker-boundary player cancellation
has focused local regression coverage, not a new TV playback acceptance.
Final power Active, foreground Marquee Video & playback; own lease explicitly released.
Proof: `/data/home/tmp/marquee-audio-skip-proof/installed-final.json` and adjacent PNGs.

### Receiver volume precision and broader controls

The recorded Denon AVR-S760H has no documented whole-unit master-volume step
setting. Its [Volume manual](https://manuals.denon.com/avrs760h/NA/EN/GFNFSYwrfimgdw.php)
distinguishes display Scale from Limit, Power On Level and Mute Level; Scale does
not change half-unit increments. The popup and read-only Sound volume row rounded
those readings; Quick Settings already displayed fractional `volumeText`.

Earlier native UI `bd31db80` established half-unit Sound and popup readings at
100% and 200% text. Actual TV readings and a volume-key round trip preserved
47.5 exactly; Dolby Audio and PCM · Dolby Surround remained visible. Evidence:
`/data/home/tmp/marquee-denon-half-volume-proof/`.

Receiver UI `0a8467e6770f2483edb24bab892f09e78e41284fc19d093d66c3900a71e97537`
and helper `94ebaded8807b691a63875261c007b2d2ffdf8ea045a72f714ab901b6f2af504`
are installed. Authenticated typed settings writes require a current session and
new matching receiver readback; queued is not applied. The actual AVR-S760H
advertises six main inputs plus qualified power, dialogue and Eco controls.
Audyssey, Zone 2 and other controls remain disabled where current prerequisites
are unqualified; sound-mode selection requires a recognized current signal.
Hardware setting writes remain unexercised. Manual receiver volume was untouched.

The owner-approved canonical bundle registration preserved core `6dab7dcd`,
SM and smart-home bytes and regenerated the native manifest. The final helper-only
discovery correction preserved that manifest. Actual native Receiver rendering
and fresh readback are retained in `/data/home/tmp/lumatv-denon-live-20261102-mvzhuzzf/`.
Owned Settings closed, final Active Standby confirmed, stage removed and own
mutex released with local mirror and remote owner absent.

### Native volume popup

PCM with Dolby Surround or DTS Neural:X processing displays the original processing-brand glyph
and an explicit PCM carrier label. Encoded Dolby/DTS inputs retain their existing glyphs; plain
PCM, direct/stereo modes, unknown input and mute do not acquire an invented encoded format.

Earlier UI `346f43accb3fb6312b9e7c575d57972b8e5efb5ce6dca12f1bb2962a2085c0dc`
established the badge behavior; the half-step update above supersedes it.
That update refreshed only the UI client through existing `ctl shell restart`; the compositor and
Marquee executable were unchanged. Actual Marquee playback shows the Dolby Audio glyph with
`PCM · Dolby Surround`. A native volume-key pair restored the exact starting level, 55.5; mute
remained false. Final power Active, foreground Marquee, native UI ready. Own mutex was released
with local mirror and remote owner absent. Proof: `/data/home/tmp/marquee-volume-popup-proof/`.

UI-client restart tears down native UI subscriptions and focus, then recomputes Home and refresh
policy. It is not intrinsically playback- or refresh-neutral; live maintenance requires explicit
permission for possible playback interruption. This movie capture does not establish neutrality.

The unchanged `/tmp/lt/tv.py` transport opens a fresh SSH connection for each command; it avoids
the observed channel closure after reusing a connection for SFTP and execution. Native popup
smoke reuses the Smart Remote input functions from `docs/research/perf/kb.py`, without tracing.
Helper receiver reads are supported; volume POSTs require their supported native-client authority.

### Picture scaling

Native placement and authored subtitle geometry share the decoded raster and pixel aspect ratio.
Fit is the default; a 1920×800 picture occupies `(0,140,1920,800)` in the 1920×1080 player canvas.
More → Video scaling changes the window without restarting or unpausing playback. Original size
is relative to that player canvas, not certified physical-panel 1:1. Fill/crop and Zoom use paired
source/destination clipping; crop choices are offered only when the native sink supports them.
ACB placement uses explicit windows with `fullScreen=false` before the separate PLAYING transition.

Stable 0.10.0 source `757261c9` is installed on `192.168.1.242`; the existing cached-lane deploy
matched all 54 file hashes. Release executable SHA-256:
`581e23c3d19789adb80dfa14c3a7feec53daa193f0ae220725340b74c254407b`.
Native Never After Dark playback shows fitted proportions and black bars. More → Video scaling
offers Fit, Fill/crop, Original size, Zoom 125% and Stretch. Fill/crop changed the paused picture
at 37:45; Fit was restored and playback advanced to 37:48. Native proof:
`/tmp/lt/marquee-scaling-{modes,fill,final}.png`. Final power Active, foreground Marquee, receiver
untouched; own mutex released with local mirror absent and remote owner absent. No platform,
picture, power or refresh changes accompanied this app-only update. Stable storage identity
remains `com.beb.plxnative`.

### Earlier SpeechDock setup

Previous payload `2b12924e` used the same stable identity and existing in-place deployment path.

The staged package is `/data/home/Marquee-release/marquee-0.10.0-2b12924e.ipk`, 21,092,004 bytes,
SHA-256 `057b4fa017cab90e3afd184670cc70437708cbd9b843ee5a87915fae1d1308bd`. Canonical/versioned
and `marquee-latest.ipk` packages were not promoted from the last on-screen-accepted `fce9888a`:
20,970,244 bytes, SHA-256 `0cb296dffd9652981d593b45692980944add0d94da40b4157e2dfe46df979afb`.
The earlier `marquee-0.10.0-37d8d0f4-rollback.ipk` is also retained.

The 2026-11-01 12:32 EST handoff records successful root `Active Standby` JSON before and after
the copy. The same app-only lease was released, with the local mirror absent and its remote token
gone. No launch, wake, power, picture, refresh, compositor or tracing command accompanied the deploy.
The later owner-opened stable 0.10.0 app window verified native keyboard entry, durable SpeechDock
Address save and the same value after reopening: `https://speechdock.nateshome.net`. Reveal changed
from Off to Word by word; detailed stats logging stayed Off. Brief enhanced playback advanced and
seeking displayed a scene still. In-app Stats showed approximately 120 Hz measured display sync,
23.976 fps source metadata and 1916×952 coded pipeline video; GL swaps are not delivered-frame FPS.
Configured English-subtitled Community playback displayed a partial cue. Its Info label said Direct
Play, but the actual route and service-aligned timing were not independently captured. A saved
Address or visible word reveal is not proof of an acoustic result. The ordinary-records check found
18 HTTP 200 receipts, but no timestamps or outcomes to attribute them to TV playback. No diagnostics
consent was enabled. Final screen was Marquee Home, power
Active, receiver unchanged. Fresh own lease release verified local mirror absent and own token
gone. Evidence: `/data/home/tmp/marquee-live-setup-20261101.json`; native screenshots are listed
there. No device job or lease remains.

An earlier passive root observation during Netflix left playback, input, screenshots and settings
untouched. Exact CPU-summed `osd_irq` advanced 8,233,968 to 8,234,065 over 1.00272638 monotonic
seconds: 97 interrupts, approximately 96.74 Hz in that one sample, with unchanged CPU topology and
no reset. This is display sync, not video FPS, unique flips or certified physical raster refresh.
`videooutput/getStatus` reported one connected MAIN for Netflix at 3840×2160 / 23.9760246 fps.
The foreground app matched, but an authoritative foreground-context join was unavailable; these
remain reported video-source fields, not final output dimensions. Final HDR and physical raster
remain unknown. Root mode 0444 on `/proc/interrupts` does not prove jailed app readability.
Power remained Active and the verified own lease was released. Sanitized proof:
`/data/home/tmp/marquee-main-output-passive-20261101.json`. The installed LumaTV reader/pill is
private native UI implementation, not a new external-app API or a reason to add a second pill.

The deployed Stats change samples native `osd_irq` counts off the UI thread only while the overlay
is visible, showing approximate Hz under `measured display sync`. Video-source rate and returned
GL swaps remain separate; reset, unreadable and stale counters do not produce live readings.
No live panel raster or final output HDR field is inferred from video metadata, placed video
rectangles or static capabilities. LumaTV owns the separate native Quick Settings info pill.

A 23.976 fps source can use five evenly repeated refreshes at 119.88 Hz. The rounded `~120 Hz`
display cannot distinguish that from exactly 120.00 Hz or certify physical-panel cadence matching.
Do not infer a broken rate switch, a verified switch, or 120 fps video from that rounded reading.

The updated artwork cache retains the last drawn viewport through playback and return within the
existing 64 source slots, render-scaled 44 MiB texture budget and 8 MiB decode-admission threshold.
Current drawn demand takes upload priority. Watched-state read-back gives the visible detail owner
the shared metadata slot and publishes terminal failures while retaining loaded data. Focused
consumer/cache and native desktop return/reversal checks passed; household TV reproduction is
unobserved.

Detailed stats logging retains its own explicit session-only app opt-in, off at boot. LumaTV’s
supported read-only `GET /v1/logging` reports saved `base.logging.enabled` for optional remote UDP
forwarding only; `active` means the worker is configured, not receiver delivery. LumaTV’s bounded
local collection is independent. Neither these flags nor Stats visibility authorize Marquee capture;
no supported external-app capture/submission contract exists. Private collector sockets/files are
not an app integration API.

The earlier `fce9888a` feature run passed 93 focused checks; four privacy identifier/preview-isolation checks
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

LumaTV's final native-platform handoff reports persistent enabled/live/UI-ready and zero failures,
with compositor SHA-256 `d19adcfec0d2edefc41de322b7a7f6c23e805362c5f78fe6ddf76aa213701da9`,
UI `b622a6764c1cd23658ed49f3fe5162efcce334345a09624c0c8552d8ef3ddd06` and surface manager
`f9625a8a57d9a226647c94065cc06c4ee11072bbd428935e6ba01cc0c7b3134d`. Its reported control
target was 120 Hz, with an earlier approximately 119.9 Hz measurement, not a zero-stutter guarantee.
Focused recovery checks passed; the full old gate exited 1, followed by a narrow checker fix and
43 passing checks, not a full rerun. Native Home proof is
`/data/home/tmp/lumatv-ui-proof.68u1vel0/native-final.png`. Marquee typing, Stats, main-output
telemetry and SpeechDock activation were not accepted in that platform window.

The following R2/R3/R4 notes are historical legacy-path evidence, not remaining installation steps
or authority to replay a packet. The later native pair has the separate pins above.

The legacy f5 foreground-only 120 Hz candidate was not installed. Its approved R2 attempt stopped
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
The approximately 4.134 ms compositor submission-to-completion span is not exclusive hidden-shell
GPU execution or proof of its causality. Preserve callback policy without forced GPU synchronization.
The bounded ring and scheduler trace do not yet link app buffers to display flips.

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
  A cold enhanced start that is refused (an invalid Opus header included) or whose source fails to
  open retries plain playback once at the retained position; any other failed open takes the
  failed-open ladder. Deferred unloads retain that recovery owner; a refused plain start becomes
  Failed even before native Ready. Seven focused cold-start/Opus checks passed; the successful
  enhanced path was exercised on the TV.
- Scene stills use authenticated Plex Part-index endpoints, not the photo transcoder. The image
  cache is memory-only; missing generated indexes show no still. The request deadline does not
  interrupt synchronous DNS resolution.
- Delete uses the existing hold-menu and an exact-scope confirmation. Membership changes follow
  confirmed deletion only, including retained Search, Person and Collection screens. Watchlist
  removal uses Discover identities, not local rating keys; uncertain legacy mappings are reported.

## Speech timing and render provenance

The earlier `2b12924e` client integrated the isolated SpeechDock source return
`1b0e4e5c3ea32f38f94525aca7a89457f9dec963` and renderer/source fences. The native Address
remains saved; the latest bounded check below restored the original Off reveal preference.
Positive acoustic timing on the TV remains unverified; estimates and authored ASS karaoke remain intact.

SpeechDock session `01a1943c-9834-7106-872e-e017bd1c394b` owns the CPU backend. Client source
ownership has returned to Marquee; `/data/home/Marquee-speechdock-alignment` remains frozen. Its
private `CARGO_HOME` is `/data/home/tmp/marquee-speechdock-alignment-cargo` and
`CARGO_TARGET_DIR` is `/data/home/tmp/marquee-speechdock-alignment-target`, separate from the
cached `/data/home/Marquee-subs` deployment lane. Integration follows the main-only convention.

The client uses protocol 1 `POST /api/alignment/windows`, off the playback thread. SpeechDock
verifies English audio independently with a local quantized CPU speech model; word boundaries use
a 20 ms grid within the authored cues. Production activation requires a trusted
`SPDOCK_ALIGNMENT_PLEX_URL`, private `SPDOCK_ALIGNMENT_PLEX_TOKEN_FILE` and read-only
`SPDOCK_ALIGNMENT_ROOTS` mappings. An empty session Address disables alignment; a trusted origin
enables requests for a profile using Word or Line reveal, subject to direct-play, English-subtitle
and renderer checks. Unavailable or unverified audio retains explicitly estimated timing. There
is no separate SpeechDock Enable switch or Test connection control.

The deployed `2b12924e` corrections intersect rendered sidecar visibility with unchanged authored
bounds before applying the offset once, omit authored cues longer than 15 seconds, and clamp startup
windows to zero. Earlier requests could reject an entire batch because ASS rounded 1009 ms to
1000 ms, admit an ineligible long cue shortened for display, or send nothing before 15 seconds.
The ARM release passed binary/package checks. Native Address/reveal setup is verified; live acoustic provenance is not.

The validated Address is published only after durable session save and restored at boot. Settings
shows Address/Disabled and validation or save errors, not connection or aligned/estimated status.
The renderer distinguishes verified word timing internally; request failures retain estimates.

The source update adds a functional player Info line: `Word timing: SpeechDock · N words applied`,
`Estimated` or `Not in use`. It reads only the accepted current-cue renderer snapshot, with
source/revision, playback generation, exact request geometry, frame validity and cue-bound fences.
Seek, fallback, source changes and busy selection invalidate the snapshot. Labels are cached
until the reported state changes; no polling, capture or private caption data is added.
The readout is installed from `d609f6a6`; all 54 payload files match and the running executable
SHA-256 is `4ffd7d6b5fbb6107eea0691c1397acbaf3217e2327d1f94aeadadaf6fbae82d3`.
The bounded TV check showed `Estimated` and cue-gap `Not in use` with Word by word, selected
English external subtitles and the saved SpeechDock origin. A real seek returned `Estimated`.
No current cue showed SpeechDock words applied; positive acoustic consumption remains unverified.
The temporary reveal change was restored to the original Off preference, owned playback closed
and actual Active Standby read back before the verified lease release. Receiver volume was left
unchanged. Backend success or a nonnull source revision is not consumption evidence.

Historical SpeechDock 0.2.0 evidence used the trusted PMS origin, private token file and read-only
movie/TV roots. That production image had bounded acoustic-positive proof:
authoritative English cue 583 returned one aligned word, UTF-8 bytes 0–4, at
2,478,100–2,478,220 ms on the native 20 ms grid, inside authored bounds 2,477,380–2,478,380 ms.
The source revision is `c50d1543…` and the model SHA-256 is `cd5040c1…`.

Coverage remains limited: one of 19 sampled film cues aligned; 18 stayed estimated. This is not
broad film-accuracy certification, and no guards or authored bounds were relaxed. A non-null
`source_revision` can accompany estimated cues and is not proof of aligned words; estimated cues
must clear earlier alignment. Changed-source validation returns `estimated`, `words: []` and a null
source revision. The private pair is `actual-plex-single-{request,response}-583.json` under
`/data/home/tmp/speechdock-alignment-production`. In that earlier setup the native Address was saved
and Word by word enabled. The backend owner reported one 1000-line container-log tail at 15:14 EST:
18 `POST /api/alignment/windows` receipts, all HTTP 200. Every returned line lacked timestamps,
and matching receipts contained no aligned-versus-estimated classification. Attribution to the
15:03–15:12 TV playback window/client and the acoustic outcome remain unobserved. HTTP success
is not aligned/consumed proof. No bodies, diagnostic capture, new inference or further polls were used.

All 19 sampled cues passed source verification: 17 failed independent exact greedy lexical
equality, one failed the acoustic score, and one aligned. Repeated-word rejection accounts for
none of these samples. Retained two-second versus four-second context evidence produced the same
16-character prefix of a 20-character target from both same-support 99-frame crops; extra context
alone did not resolve acceptance. These are backend coverage limits, not connection failures or
evidence that playback on the TV is required to diagnose them.

For retained cue 951, the recorded 75-second clip starts at movie second 4890. Its effective cue
rebases to 13.660–15.660 seconds; the same-target qualified CPU fixture uses 14–16 seconds and
places the final word at 15.500–15.900, outside the real cue end by 240 ms. The sample-specific
+340 ms mapping is diagnostic only, not a global offset. Full-clip ASR contains the target but
disagrees on token times, so those timestamps are not timing ground truth. This does not explain
the remaining 16 lexical rejections or the weak-score rejection.

The corrected real Rust sidecar parse → renderer readiness → request → HTTP → CPU backend →
sidecar consumer path passed locally: offsets 0 and +300 ms each delivered three acoustic words;
−300 ms remained estimated with no borrowed words. Requests retained the 1009 ms authored start,
excluded the authored-long cue and used a valid 0–30-second startup window without HTTP 422.
Proof: `/data/home/tmp/speechdock-client-cpu-proof.json`. This is bounded fixture proof, not broad
film accuracy or TV acceptance.

ABI 6 epoch-fenced renderer readiness, truthful player activation, visual modes and authored ASS
karaoke remain required. Embedded alignment capture and publication require the exact displayed
source ID and revision to be renderer-ready. Estimated cues remove earlier aligned words; a missing
or changed verified backend source revision invalidates retained timings, while an unchanged
revision can retain valid out-of-window words. Requests are cancelled or ignored when their owning
media/audio/subtitle selection changes. Backend deployment and TV verification require a separate
explicit handoff; no new HomeLabber requests are authorized.

The existing subtitle/Bazarr images in `docs/marquee/renders/` are desktop simulator frames under
Xvfb, not TV captures. The shared table still uses value read-outs rather than mock sliders/swatches.
