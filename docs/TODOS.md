# TODOS

## Audio language preferences

- [x] Set a clear preferred audio language with robust track-language matching.
- [x] Remember manual audio-language choices per show or movie without changing the global preference.
- [x] Verify and deploy audio-language preferences and title memory through the established Marquee lane.

## Intro and credits controls

- [x] Add intro and credits auto-skip preferences with configurable countdowns.
- [x] Provide Skip intro and Watch intro buttons with default skip focus.
- [x] Provide credits countdown, Watch credits cancellation and persistent manual next-episode action.
- [x] Explain missing Plex intro or credits markers without blocking playback.
- [x] Verify and deploy intro and credits skipping through the established Marquee lane.

## Denon volume precision and controls assessment

- [x] Assess Denon control capabilities and whether master volume supports configurable whole-number steps.
- [x] Display actual half-step receiver volume consistently in the native TV UI when whole-step AVR operation is unavailable.
- [x] Verify focused volume behavior and document the full AVR-settings integration requirements.

## Subtitle-safe volume popup

- [x] Make bottom-positioned volume popup avoid subtitles using the existing native layout path.
- [x] Verify subtitle-safe popup placement with focused native renders and preserve deployment notes.
- [x] Coordinate other TV sessions and put the TV in standby after the update and brief check.

Marquee publishes only changed rendered subtitle geometry through its existing Wayland
connection. The OS popup avoids current painted bounds without screen scanning,
media logging or polling; unknown geometry uses a conservative safe area. Full
Lucent shadow and antialias bounds must clear the caption, not only the layout body.

## SpeechDock first-word reveal

- [x] Integrate the reviewed first-word onset fix without changing authored cue bounds or estimated fallback.
- [x] Build, install and briefly verify the corrected stable app, then coordinate final standby and release.

The `b506a7ae` release is installed with all 54 files matching. Actual playback and
subtitle-safe popup acceptance completed; owned playback closed and Active Standby
was verified at 01:27 EST on 2026-11-02. The full backlog below remains retained.

## Sonarr and Radarr integration backlog

- [ ] Add clean native Sonarr/Radarr connection settings and connection checks.
- [ ] Request movies directly through Radarr.
- [ ] Request shows directly through Sonarr.
- [ ] Expose basic library, monitoring, quality-profile and download management.
- [ ] Search and select manual releases, with quality and rejection reasons visible.
- [ ] Delete movies, episodes and shows with explicit confirmation and clear file/library scope.

### Torsher reference and playback during download

Reference: [Torsher](https://github.com/torsher-app/torsher). Its published features include ranked
releases, audio/language preferences, series following, playback during download, read-ahead
buffering, temporary storage with Keep, and title-level progress across release changes.

- [ ] Review Torsher’s TV-first request, ranked-release, language/audio, follow-series and future-movie workflows for the existing Sonarr/Radarr integration.
- [ ] Design a Torsher-inspired play-while-downloading backend compatible with Unraid and the existing download client, without replacing Sonarr/Radarr/Plex.
- [ ] Play supported downloads before completion with prioritized fetching, bounded read-ahead, truthful buffering/progress, and seeking limited to available or fetchable ranges.
- [ ] Add temporary-cache/Keep/cancel handling, safe completed-download import, and title-level resume across release changes without duplicate files or broken seeding.
- [ ] Review Torsher’s AGPL-3.0 license and dependencies before copying or adapting upstream code.

- [ ] Audit Torsher for applicable Marquee-wide startup, poster/image loading, scrolling, memory/cache, networking and playback-buffering improvements; retain only measured benefits using the proven TV performance paths.
- [ ] Review the rest of Torsher for worthwhile future Marquee features, including household profiles/PINs, language preferences, setup checks, notifications, backup/restore/export and device management; produce a prioritized adopt/adapt/skip shortlist with platform fit and licensing constraints.

The Torsher review extends beyond Sonarr/Radarr to Marquee as a whole. Performance patterns are
candidates, not demonstrated improvements: browser-specific techniques need native-platform
equivalents, measured benefit and no regression before adoption. Additional features remain
ranked proposals rather than an instruction to clone the complete application.

Torsher bundles its own torrent engine based on anacrolix/torrent. Sonarr and Radarr manage
acquisition and import; they do not themselves supply a playable partial-file stream. Backend,
download-client and webOS format/seek compatibility need verification before implementation.
Upstream code is AGPL-3.0; feature inspiration is separate from copying its implementation.

These integration requests remain a separate backlog; the scaling and volume-popup fixes do not implement them.

## Marquee work list

### Measure

- [x] Measure Marquee rendering and TV presentation frame rates
- [x] Identify the sources of visible UI hitches
- [x] Collect the already-authorized read-only compositor ring and six-second scheduler trace
- [x] Engage the Native120Pacing slow-role helper for the remaining smooth-120 goal
- [x] Review existing graphics and hardware-layer notes before choosing a pacing fix
- [ ] Establish the authoritative main-display resolution, refresh and HDR source during video playback
  Measured display-sync ~120 Hz and source metadata work; physical output raster/final HDR and exact119.88 vs120.00 remain unverified. No additional display experiment in this surgical update.
- [x] Coordinate the shared display-output contract and aesthetic quick-settings info pill with LumaTV
- [x] Map the existing poster/image cache, playback-return residency and fast-scroll prefetch behavior

### Improve

- [ ] Remove measured UI frame stalls
  Two saved16.7ms intervals lack causal app-buffer/commit/draw/fence/flip attribution; zero-stutter acceptance remains unproven. Requires a separately scoped measurement window.
- [x] Enable 120 fps UI if the TV presentation path supports it
- [x] Remove remaining legacy Plx Native branding and old project links from the app UI
- [x] Fix cold-start dialogue enhancement and preserve playback on enhancement failure
- [x] Fix hitching while Home banners transition and settle
- [x] Match video cadence without extra flashing and restore the fastest supported UI refresh
- [x] Use the TV GPU and background CPU workers without blocking UI frames
- [x] Show confirmed Bazarr search stages without inventing measurable progress
- [x] Report Bazarr no-results and provider, connection or download failures clearly
- [x] Track real subtitle download, installation and activation progress through completion
- [x] Audit Stats for Nerds metric sources and logging behavior
- [x] Reorganize Stats for Nerds into an intuitive Lucent overlay
- [x] Keep Stats for Nerds diagnostics accurate and tied to the logging setting
- [x] Coordinate any necessary 120 Hz TV OS and menu changes with the LumaTV session
- [x] Review and integrate the isolated SpeechDock audio-informed subtitle alignment source return
- [x] Implement authoritative main-display output in Marquee Stats without confusing source or overlay rates
- [x] Fix poster pop-in using the existing bounded preload, pinning and eviction paths
- [x] Fix the stuck loading spinner after marking a title watched from the long-press menu
- [x] Review LumaTV’s native-menu 120 fps source with the existing sole Slow-role helper
- [x] Resolve actionable offline SpeechDock integration gaps without waiting for TV access
- [x] Verify subtitle offsets are applied once and consistently across SpeechDock requests and rendering
- [x] Fix default aspect-ratio preservation for non-standard-resolution video
- [x] Expose the existing video scaling choices in the player controls

### Verify and deploy

- [x] Run focused TV performance checks and deploy improvements
- [x] Record verified performance limits and proven measurement paths
- [x] Send compositor-side evidence and release the TV window to the LumaTV session
- [x] Deploy and smoke-check the subtitle progress changes through the proven Marquee lane
- [x] Recommend useful next tasks and safe parallel work from the existing project backlog
- [x] Deploy and smoke-check the updated Stats for Nerds overlay through the proven Marquee lane
- [x] Report the current Marquee source, build and runtime ownership to SpeechDock
- [x] Reserve the alignment-owned player, subtitle settings and client seams until integration handoff
- [x] Send the explicit isolated alignment-source handoff to SpeechDock without TV authority
- [x] Record the source-only menu pacing and graphics packet for the sole Slow worker
- [x] Verify main-display telemetry during playback with overlays and deploy the Stats output change through the proven app lane
- [x] Exercise long-playback-return and rapid-scroll image residency through the existing native smoke path
- [x] Verify mark-watched completion clears the loading state without extra Back presses
- [ ] Activate and verify production SpeechDock alignment through the existing native settings flow
  SpeechDock settings and alignment client are installed. Real TV consumption of verified aligned word times is unproven; untimestamped HTTP200 receipts do not prove it.
- [x] Synchronize the verified c5199 app payload to the standby TV without waking or launching it
- [x] Confirm Marquee’s cached file-only lane ownership from local completed-command evidence
- [x] Recover and live-relay the existing cached-lane mutex path and safe file-only recipe
- [x] Record and relay the verified standby-only file-lease release while preserving the failed quiet-screen guard
- [x] Build and package the three SpeechDock client fixes through the proven cached ARM lane
- [x] Synchronize the verified 2b12924e app payload to the standby TV without waking or launching it
- [x] Report the actual SpeechDock transcription-model comparison and best subtitle model
- [x] Build and deploy the targeted aspect-ratio and scaling-menu fix through the existing cached lane

### Targeted volume-popup fix

- [x] Locate the missing DTS/Dolby logo path and its playback-safe update mechanism
- [x] Fix the audio-format logo without changing playback or audio settings
- [x] Apply the popup-only update with the now-authorized UI refresh and verify the live logo
- [x] Restore the complete previous Todo list from saved records after the volume-popup update

## Player / trailer preview

### Grade `hero_scrim_a`'s legibility contract against synopsis text over live trailer video

**What:** Add a row to `widgets::hero_scrim_a`'s anchor table for "synopsis-weight (`LABEL` 26px)
text over a bound video plane at `field=1.35`" and grade it against a real device capture, real
trailer content. *(The `landing_hero::PROMOTED_FIELD` half of this item is GONE as of 2026-09-17:
full-trailer mode draws no page chrome at all, so the wedge eases to 0.0 and the constant was
deleted.)*

**Why:** `docs/trailer-ux-plan.md` §2.3 keeps the detail hero's synopsis visible during background
trailer autoplay for the first time — previously no prose was ever drawn over playing video, only
the title. Per this project's own DESIGN.md, "still artwork has a knowable worst case... video does
not," so this is a real legibility question with no existing answer. It's deferred out of the
trailer-UX PR (a scope decision, not an oversight) because it needs a TV and real footage to mean
anything — a host test can only check the arithmetic is internally consistent, not that it's
actually legible.

**Context:** Full detail and the exact anchor-table row to add are already written out in
`docs/trailer-ux-plan.md` §2.3. Land this as its own
device pass immediately after the trailer-UX PR ships, using `tools/capture-screen.sh` on real
trailer content per §5's "Fast-follow" list.

**Effort:** S
**Priority:** P1
**Depends on:** The trailer-UX plan (`docs/trailer-ux-plan.md`) shipping first.

### Re-measure RSS headroom to see if `CYCLE_BUDGET` (14 Loads/process) can go higher

**What:** Redo the device memory-headroom measurement `player/preview.rs`'s own module doc bases
`CYCLE_BUDGET` on (currently ~958 KiB headroom / `sf_load`'s fixed 64 KiB slot ≈ 14), on current
builds and devices, to see whether the ceiling can safely move.

**Why:** A faster autoplay trigger (whichever mechanism the trailer-UX plan's §2.1 investigation
lands on) makes this fixed budget the binding constraint on how long a browsing session gets
previews before they silently stop — at roughly double the trigger rate, a session could exhaust 14
cycles in about half the cumulative dwell time it takes today. The ceiling itself hasn't been
revisited since it was first measured.

**Context:** This is real device memory profiling, not a quick constant bump — inflating the number
without re-measuring would be exactly the kind of unverified platform claim this project's AGENTS.md
warns against. See `docs/trailer-ux-plan.md` §2.1 for the current arithmetic and why this is called
out as a follow-up rather than folded into that plan.

**Effort:** M
**Priority:** P2
**Depends on:** None — independent of the trailer-UX PR, but informed by its dwell-mechanism
outcome.

### Device verification pass for the trailer UX feature (fps scene, real dwell pass, basic capture)

**What:** Everything in `docs/trailer-ux-plan.md` §5's "Device (blocking)" list that a host test
cannot answer: an `fps:`-style scene exercising the logo's move+shrink transform (floor + worst/
coldopen ceiling checks), a real-device browsing pass at the shipped 2.0s dwell to sanity-check
false-trigger/budget-consumption behavior empirically, and a basic `tools/capture-screen.sh` still
of each of the four states (idle, dwelling, background autoplay, full trailer) confirming nothing
is grossly broken (wrong position, wrong alpha, focus landing somewhere invisible).

**Why:** The code landed with full host-test coverage (`make check` green) but zero device
verification — there is no host runtime for GLES composition, text rasterization, or real trailer
video, so none of this can be confirmed from a desk. This is the ordinary "does it actually look
and feel right" pass every UI change needs before it's trusted, independent of the deeper
legibility grading in the anchor-table TODO above.

**Context:** `docs/trailer-ux-plan.md` §5 has the exact scene design (reuse `plxnative-navosc`'s
headless-trigger pattern, per `K_DISC_UNFURL`'s own disc-unfurl scene as the nearest analogue).

**Effort:** M
**Priority:** P1
**Depends on:** The trailer-UX plan shipping first (done, 2026-09-15).

### Investigate decoupling trailer fetch-start from the visible reveal

**What:** Determine whether `player::preview::Machine::start` can be triggered on a SHORTER
"fetch-commit" threshold than today's single dwell timer, while the screen holds its full Idle
presentation for a separate, longer minimum reveal delay — so felt latency
(currently `dwell + Load time`, serial) becomes closer to `max(fetch-commit, reveal, Load time)`.

**Why:** This was the outside-voice-recommended alternative to a blanket `DWELL_S` cut (which is
what shipped instead, as the documented fallback — see `player/preview.rs`'s `DWELL_S` doc
comment). It was not implemented because it needs real device data to answer the open question it
raises on paper: does a shorter fetch-commit threshold meaningfully reduce false starts, or does it
just spend the same 14-Load `CYCLE_BUDGET` faster on items a viewer glances past and moves on from
(via the existing `PreviewStop`/`abandon` cancellation path, which only fires AFTER a Load is
already admitted and budget-spent)?

**Context:** Full mechanism sketch, the exact risk, and the fallback-replacement contract (nothing
else in the trailer UI depends on which mechanism sets `view.picture`) are in
`docs/trailer-ux-plan.md` §2.1 and the `DWELL_S` doc comment in `player/preview.rs`.

**Effort:** L (real investigation + a device pass to validate whichever answer it finds)
**Priority:** P3
**Depends on:** None, but do the RSS re-measurement TODO above first if this raises Load frequency
further — the two compound.

### Move natural-trailer-completion detection into `player::preview::Machine`

**What:** Add a one-shot `just_finished`/rk flag to `player::preview::Machine`/`View`, set at the
`Machine::eos` transition (`preview.rs`, `after_pump`'s `Settle::Ended` arm) before `Machine::stopped` wipes the
session's `key`, consumed and cleared by `DetailScreen` each tick.

**Why:** `docs/trailer-ux-plan.md` §8.2's play-once suppression (shipped) reconstructs "did the
trailer finish naturally, vs. was it interrupted by the viewer scrolling/leaving" at the SCREEN
layer, via a `hero`/`scrolled_off`/`preview_promoted` heuristic (`preview_tick`'s `hero_active`
combined with `preview_promoted`). This is the shape an outside-voice review pass caught a real
ordering bug in during that PR's eng review — `preview_promoted` only clears at the very last line
of `preview_tick`, so a naive check near the top of the function read last-tick's value and missed
EOS-during-full-trailer-mode entirely. The shipped fix patches the heuristic correctly (and adds a
`preview_started_for` capture to avoid attributing "played" to the wrong item if one swaps
underneath a live full-trailer session), but the underlying fragility is structural: `Machine`
already knows unambiguously which rk just finished, at the one place (`Machine::eos`) that has that
fact before anything else can race it. A `Machine`-owned one-shot flag would make this class of bug
structurally impossible rather than merely fixed, and would give any FUTURE consumer (not just
`DetailScreen`) the same unambiguous signal for free.

**Pros:** Centralizes "did this trailer actually finish" at its true source of truth. Eliminates the
whole ordering-bug/attribution-risk class this review found, rather than just patching today's two
instances of it. Reusable by any future screen or feature that needs trailer-completion signal.

**Cons:** Touches `player::preview`, a shared module other code paths depend on (larger blast radius
than the screen-local fix that shipped). No second consumer exists today to justify the refactor on
its own merits — this is a "do it when it starts hurting twice" deferral, not an urgent one.

**Effort:** M
**Priority:** P3
**Depends on:** `docs/trailer-ux-plan.md` §8 shipping first (the screen-local fix this would
replace).
