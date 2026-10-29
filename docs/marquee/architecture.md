# Marquee architecture

Plan for rebuilding PlxNative as Marquee on one television: an LG C4 (OLED65C4PUA, webOS 10.2.1, chip
O22A3, four Cortex-A76 at 1.4 GHz, Mali-G52 MP3, 2 GB RAM). Nothing here has run; the change that adds
this file touches no code and no television. The Lucent redesign (screens, motion, mockups) is
`docs/marquee/design/`; this document covers what runs where, how the parts talk, what formats play
by which path, and in what order the work ships.

Evidence tags: `TV` is a measurement on a television (this project's or LumaTV's), `REPO` is read in code
or docs, `[INFERENCE]` is neither. `lumatv:NAME.md` is `docs/research/NAME.md` in the LumaTV repository
(its forge `main`) and `lumatv:/path` is from that repository's root; unprefixed paths are in this
repository. Line counts are production lines, counted to the first `#[cfg(test)]` of each file (a rough
figure); tests are about two thirds of the code base.

## 1. The decision

1. **Two programs.** `marquee` (armv7, SAM's `main`) is the broker: LG's Starfish pipeline, luna-service2,
   the SAM lifecycle. `marquee-ui` (aarch64, on the Rockchip Mali blob through LumaTV's `gfx64` runtime)
   is everything else: Plex, planning, demux, subtitles, software decode, drawing, input.
2. **The cut uses two seams that already exist.** `tv::sink::VideoSink` (32 Starfish-shaped verbs) and
   the `tv::Port` table (25 entries). `ci/module-layers.ini` already fences the webOS port to five modules
   (`webos keymanager system player::ffi port`); the 32-bit side is what that fence encloses.
3. **Video never passes through the UI process.** LG's pipeline decodes onto a display-engine video window
   below the UI plane. The UI exports the window id, the broker names it in the Load payload, the
   compositor punches the hole.
4. **The UI is LumaTV's Rust stack** (LumaShow toolkit and renderer, Lucent tokens), first as a Wayland
   client with the engine in its own process, later hosted in LumaTV's compositor.
5. **120 fps belongs to LumaTV's compositor.** LG's compositor paces clients at 60 and no app can change it.
6. **Format breadth is built in the 64-bit process**: more demuxers, in-app stream rewrites (Dolby Vision
   7 to 8.1 exists; AC-3/E-AC-3 re-encode is new), software decode for 1080p-and-below SDR, Plex
   transcode last.
7. **About 100k production lines carry over, about 115k of view and loop code are rewritten, about 4k
   move to the broker, and about 6k of HTTPS transport are redone on rustls.**
8. **Eleven slices, four format slices and the tier A work**, each leaving an installable app. The 32-bit
   PlxNative UI stays as a fallback child until the Rust UI has parity.
9. **One measurement decides the split**: whether Starfish in one process can import a window exported by
   another (section 10, M1).

```
  SAM (LG) -- launch, relaunch, close --+
                                        v
 +--------------------------+   rings, socketpair    +-----------------------------+
 | marquee (armv7, 32-bit)  |<---------------------->| marquee-ui (aarch64)        |
 | SAM main, broker         |  commands, events,     | Plex, planner, demux,       |
 | libplayerAPIs, libpf     |  status block          | rewrite, software decode,   |
 | luna-service2            |                        | libass, renderer, input     |
 +------------+-------------+                        +--------------+--------------+
              | Wayland: the pipeline imports                       | Wayland: window, exported
              | the exported video window                           | video element, keys, pointer
              v                                                     v
        +---------------------------------------------------------------------+
        | compositor: LG's surface-manager, or LumaTV's                       |
        +---------------------------------------------------------------------+
          decoded video on display-engine windows below, the UI plane over them
```

Out of scope: other televisions (upstream PlxNative serves them), music, photos and live TV (PlxNative's
own scope), HDMI inputs, and DRM (Plex personal media carries none).

## 2. Where PlxNative stands

**Process.** One 32-bit ARM process, started by SAM in LG's jail under a per-app uid
(`lumatv:cardapp-result.md`). `src/main.c` boots, `plex_run` runs the Rust application. The UI is an SDL2
window (LG's fork, 2.0.14) over Wayland with GLES2 on LG's 32-bit libmali r46p0, drawn by an
immediate-mode toolkit (`plx_ui`, `plx_gfx`) whose screens are deterministic machines stepped by one frame
algorithm (`docs/ui-restructure-spec-v4.md`, `docs/sans-io.md`).

**Playback.** `PMS GET -> ff.rs (bundled FFmpeg 9.0, custom AVIO, http or https) -> access-unit queues
(aq.rs) -> pump -> StarfishMediaAPIs::Feed`, Load mode `BUFFERSTREAM`
(`rust-modules/media/src/player/CLAUDE.md`). The pipeline sees elementary streams, so the container never
reaches LG's code. On webOS 5 and later the plane binds through an exported Wayland window: the app asks
the compositor for `_Window_Id_<n>`, puts it in the Load payload as `option.windowId`, and libplayerAPIs
(which pulls `liblsm-connector` and `libwayland-webos-client` into the caller) imports it by name
(`docs/webos5-port.md` 3.3). Every Starfish call is made by the frame thread under a compiler-enforced
`MainThread` token, except `Load` (a worker) and the real stop's `Unload` (an `sf-unload` worker; 110-150 ms
on the dev set). Subtitles are drawn by the app: libass for text and ASS, FFmpeg bitmap decoders for PGS
and VobSub (`docs/ass-subtitles.md`).

**What is 32-bit-bound, and why.**

| Item | Why it cannot move to 64-bit | Where |
|---|---|---|
| Starfish and libpf | LG ships 32-bit only; the C++ symbols are called by mangled name; webOS 5+ loads `liblsm-connector` into the caller | `src/starfish.c`, `player/ffi.rs` |
| luna-service2 and glib | 32-bit libraries; the hub keys a role to `main`'s exact path; carries SAM `registerApp`, configd, settings, db8, keymanager3, toasts | `webos.rs`, `keymanager.rs`, `plxnative-storage` |
| LG's SDL fork | window, exported window, key events (the struct is shifted four bytes) | `system.rs`, `app/events.rs` |
| LG's libmali r46p0 | the only GL driver LG ships; 64-bit processes use Rockchip's g24p0 blob through `gfx64` | `plx_gfx` |
| The set's libcurl | SONAME moves between releases, so it is dlopen'd; a 64-bit process cannot load it | `net.rs`, `curlio.rs` |
| Bundled FFmpeg and libass | built for armv7 `cortex-a9` (Rust with NEON off) so one binary runs on every supported set | `ci/build-ffmpeg.sh` |

The 4 GB address-space limit does not bind at 2 GB RAM. The gain from 64-bit is the driver path and NEON
codecs, not scalar code: LumaTV measured the same Rust benchmark built both ways on this set as level at
byte hashing, 8-10 % faster at sorting and parsing and 31 % faster at float loops in 64-bit, and 2000
small GL draws a frame at 15.3-17.5 ms on the 64-bit blob against 23.9-27.8 ms on LG's 32-bit driver
(`lumatv:/docs/PLAN.md`, Rendering stack).

**What makes the split cheap.** The webOS port is one table plus one trait, enforced by CI. Machines never
perform IO: their work leaves as typed effects that runners execute and every result returns as an
addressed event (`docs/sans-io.md`). A broker connection is one more runner. The July 2026 review's
structural findings (no failure channel, route globals) are closed in the current code (`FailureKind`,
`PlaybackSession`).

**Alternatives rejected.**

| Alternative | Why not |
|---|---|
| Keep everything 32-bit, change only the renderer | LG's 32-bit driver measured 55.5-55.8 fps on a trivial window where the 64-bit path with contiguous buffers held 60.2-60.8, and the 64-bit blob spends about 36 % less CPU per draw call (15.3-17.5 ms against 23.9-27.8 ms for 2000 draws); LumaTV's whole UI stack is 64-bit Rust |
| A 64-bit player through other APIs | No 64-bit media API exists. `libNDL_directmedia`, Starfish and the GStreamer plugins are 32-bit; the V4L2 nodes (`lgsoc-vdec-*`) are root-only plumbing that skips secure memory, HDR metadata and plane routing [INFERENCE] (`lumatv:opennow-result.md`, media API table) |
| NDL DirectMedia instead of Starfish | The NDL elementary-stream player has no clock (it presents on arrival) and its v2 header offers PCM, MP3 and Opus audio only and no Dolby Vision or Atmos declaration; it suits cloud gaming, not a movie player with pause, seek and audio sync |
| Cut at the access-unit queue (broker owns the pump) | `pump.rs` and `engine.rs` name `route::` 667 times (claims, flights, ABR holds, recovery ladder); a pump in the broker needs the whole planner state across the boundary |
| Marquee as a surface inside LumaTV's shell | Marquee is an app with its own lifecycle, video and keys; LumaTV's shell hosts system layers, not apps |

## 3. Process split and IPC

### 3.1 The programs

| Program | Arch | Runs | Owns |
|---|---|---|---|
| `marquee` | armv7 hard-float, C and Rust | SAM's `main`; one process | SAM registration (`registerApp`, `relaunch`, `close`), launching `marquee-ui` through `lumatv:/gfx64/launch.sh`, the Starfish sink and its callbacks, every luna call (configd, settings, db8, keymanager3, notifications, application manager), the device probe, sound-output reads, crash tracer, watchdogs, the rings |
| `marquee-ui` | aarch64 glibc 2.36 bundle | child of `marquee` | Plex layer and stores, planner, ABR, demux, rewrite, software decode, libass, drawing, input, Wayland window and exported video element, HTTPS |
| `plxnative-storage` (kept as is) | armv7 | luna service started by an activation hint | db8 and keymanager session storage; the UI reaches it over its own private socket, the broker sends the activation hint |

`main` must be the broker because a native app's luna role is generated at install time for `main`'s exact
path (`lumatv:gfn-app-result.md`: the feeder is `main` for the same reason). `marquee` is also the
launcher, following the proven card-app shape: it parses SAM's launch JSON, starts the 64-bit child on an
inherited descriptor, registers with SAM, forwards `relaunch` and `close` (`handlesRelaunch` true, as the
card app, so a relaunch reaches the launcher instead of restarting the process), and escalates an
unanswered close to SIGTERM after 4 s and SIGKILL 2 s later (`lumatv:cardapp-result.md`, 34 checks passed
on the set). The broker holds no Plex token, opens no network connection, draws nothing and links no FFmpeg.

**Launch.** SAM starts `marquee` -> device probe, SAM registration, rings and socketpair made -> `launch.sh`
preflight (64-bit kernel, `/dev/mali0`, a buffer allocator) -> `marquee-ui`. A UI that fails to start
inside 5 s runs the fallback: the 32-bit PlxNative UI as a child on the same channel (`launch.sh
--fallback`, `LUMATV_GFX_PATH=32`). Preload launches (`reason: preload`) map nothing and draw nothing until
the first relaunch.

**Failure.** The UI treats a broker silent for 3 s as lost (status-block tick) and shows a typed failure;
the broker treats a hung-up socket or a UI silent for 5 s as a dead UI, unloads the pipeline and restarts it
with LumaTV's shell policy (backoff 2-32 s, give up after 5 starts in 10 minutes, then the legacy UI).
`marquee` has PlxNative's crash tracer (`src/crashtrace.c`) and a `SIGTERM` handler that `_exit`s even from
inside a swap waiting on a frame callback (the card app's rule; LG's driver gives up on a missing callback
after 1 s and crashes the app at close, `lumatv:/docs/research/compositor/slot-2026-10-29-r17/apps`).
Splitting the broker into a supervisor and a re-exec'd child (same executable path, so the same role)
isolates faults in LG's libraries and is held back until M1 shows the role covers it.

**Platform.** The SAM jail's `/dev` has `mali0`, `dma_buf_unified`, `vdec`, `vo`, `video*`, `snd` and no
`dri/card0` or `dma_buf_te`: a jailed 64-bit client gets ordinary-page buffers (the 56 fps class), not
contiguous CMA ones. No port, no listening socket and no filesystem rendezvous is used between the two
programs; the channels are inherited descriptors.

### 3.2 The cut

`VideoSink` has 32 verbs. Fifteen remain in the broker for webOS 10 (`load ready is_load_completed play
pause set_volume flush push_eos seek feed unload callback_gate_retire callback_intercepts destroy
quarantine`), five window verbs move to the UI because the UI owns the surface (`window_mode window_id
create_window place_window destroy_window`), and twelve are dropped: the ACB bind verbs (`plane_*`, eight),
the webOS-4 segment seek (`set_time_to_decode set_content_info send_segment`) and the webOS-3 string ABI
(`feed_abi_is_cow`). Marquee targets webOS 10.2.1 only; upstream remains the answer for other sets.

Of the 25 `tv::Port` entries, 12 are luna and device calls that go to the broker (`probe_device
start_capability_probe repair_sandbox seal open remove system_locale sound_output go_home poll_home
deliver_toast pump_bus`), 12 are window and frame-probe calls that become UI-local Wayland code
(`bind_window grab_surface release_surface arm_opaque_region opaque_route clear_opaque_region
frame_probe_*`), and one is the sink. The UI side of the table is a new port crate that fills the same
entries with IPC calls; nothing above `plx_platform` changes. This is upstream's own open step L15b
(`docs/module-layers.md`), taken with a process boundary and without the ACB bind sequence, which webOS 10
skips.

`engine.rs` and `pump.rs` stay in the UI process. The one real change is the feed path. Today
`feed_stream` calls `Feed` up to 120 times a tick and reads each reply (`'O'` taken, `'B'` buffer full),
and `max_fed_*_pts`, which priming trusts, advances only on `'O'`. Over the contract `feed` is pipelined
(section 3.3). `sink()` appears 70 times (54 in `engine.rs`, 16 in `pump.rs`); each call that reads a
verb's return value becomes issue-then-event where the verb can outlast a frame, and keeps a synchronous
shim bounded at 2 ms where the TV shows it answers within that (M3).

### 3.3 The contract

All channels are created by the broker and inherited by the UI.

| Channel | Kind | Direction | Carries |
|---|---|---|---|
| Command ring | shared memory (memfd), single producer, single consumer, 16 MiB | UI to broker | everything that must stay ordered with access units: `Feed`, `Verb`, `Open`, `Release`; AU bytes inline |
| Event ring | shared memory, 256 KiB | broker to UI | `Callback`, `VerbDone`, `LoadReturned`, `Released`, `Lifecycle`, `Log`, `Fatal` |
| Status block | shared memory, one writer, versioned snapshot | broker to UI | accepted high-water pts per lane, buffer-full flag, stage, loaded flag (the answers of `ready` and `is_load_completed`), callback-intercept count, displayed and dropped counters, the 5 Hz position, broker tick |
| Control socket | `socketpair(SOCK_SEQPACKET)`, frames of at most 64 KiB | both | `Hello`/`Welcome`, `Platform` calls and replies, ring wake-ups (a pair of eventfds) |

**Commands (UI to broker, in the ring).**

| Record | Fields | Broker does |
|---|---|---|
| `Open` | epoch, Load payload (at most 32 KiB; `windowId` and the Dolby Vision and Atmos nodes already spliced by the UI), PCM lead plan | `Load(payload, epoch)` on its load thread |
| `Feed` | epoch, lane (video or audio), es flag, key flag, pts ns, length, bytes | `Feed(p, size, pts, es)`; holds an AU the pipeline refuses and stops draining |
| `Verb` | seq, epoch, op: `Play Pause SetVolume(0-100) Flush PushEos Seek(ms)` | calls it, answers `VerbDone{seq, rv}`; a `Seek` answered 0 is a refusal and sends the session to Reload, as today |
| `Release` | epoch, quarantine flag | `Unload` on a worker, callback gate retire, `destroy` or quarantine, answers `Released{epoch, ms}` |

**Events (broker to UI).** `Callback{epoch, type, num, text}` is every `sf_on_event` the firmware raises
(load completed, position tick, sink counters 46/47 or 48/49, `type=18` errors, `UNLOADCOMPLETED`),
forwarded raw: the decode of those numbers (`sf_on_event`, `sink_counter_kind`) stays in the UI process.
Also `LoadReturned{epoch, rv, ms}`; `Lifecycle{Foreground, Background, Relaunch(params), Close(reason)}`;
`Log{line}` (one scrubber in the UI process; `scrub_local` rules apply to broker lines too); `Fatal{kind}`.

**Platform (control socket).** `Hello{proto, build}` answers `Welcome{proto, build, webOS release,
codename, model, board, the codec table JSON, GStreamer decoder ranks, audio-stack fingerprint,
Dolby-Vision answer, sound output, locale, window mode}`; the rest are the 12 broker-bound `Port` entries
and the storage activation hint. There is no generic luna tunnel: a request cannot name a service, a
method or a db8 kind, the rule the storage helper's wire already follows (`wire.rs`: "no request can choose
a DB8 owner, kind, ID, or arbitrary method").

**Rules.**

- **Epochs.** Every record carries the epoch of its `Open`. A record, callback or AU with another epoch is
  dropped by its receiver. This is the callback-gate model the engine already has (late firmware callbacks
  carry only an object address).
- **Order.** One producer writes verbs and AUs into one ring, so a `Flush` or `Seek` is ordered against the
  AUs around it by construction. The broker applies records in order.
- **Backpressure.** The UI's `feed` returns `'O'` when the record fits the ring and `'B'` when it does not;
  the broker holds an AU the pipeline answered `'B'` to and stops consuming, so a full pipeline fills the
  ring and reaches the pump as the same `'B'`. The pump's fed high-water for priming is the status block's
  accepted pts, not the `'O'`. The ring is sized above the pipeline's own video source buffer (8 MiB,
  `srcBufferLevelVideo.maximum`) so one 4K IDR never sticks.
- **No waiting on the frame thread.** The frame thread reads the status block and the event ring and
  writes the command ring. Platform calls run on runner threads (the app's existing rule: frames never
  wait on luna or storage).
- **Versioning.** `PROTO` is checked in `Hello`; both programs ship in one package. Unknown fields are
  refused (`deny_unknown_fields`); a mismatch stops the UI with an update-needed error.
- **Cost.** 100 Mbit/s is 12.5 MB/s; at 60-250 AUs a second the copy and wake-ups are under 1 % of a core
  [INFERENCE, M3]. LumaTV's GFN design plans a socket-only transport on the same arithmetic (its throughput
  on the set is unverified, `lumatv:gfn-app-result.md`); the ring is chosen so the pump never makes a
  syscall per AU.
- **Tests.** A fake broker implements the contract with `ffi_host.rs`'s synthetic-clock sink, so the real
  demux, queues and pump run in `cargo test` through the rings. The same conformance suite runs against
  the real broker on the set.

### 3.4 The video plane

Hardware: two video windows (`NUM_WIN_MAX 2`: MAIN and SUB0) sit below one UI layer; OSD0 is 1920x1080 and
scaled x2 by the display engine; "OSD1" is the same hardware as OSD0 (the driver ignores its enable while
OSD0 is on); the third plane is a 128x128 cursor; the 2D blitter is not a layer
(`lumatv:scanout-research.md`, Layers). Video windows carry opaque pixels with no alpha. The UI shows
video through a transparent hole.

**Baseline (V1).** The UI exports the element before its first commit (`wl_webos_foreign.export_element`,
type 0, the sequence of `lumatv:/vplane/src/vpl_wl.c` and the GFN app), keeps the id, places it with
`set_exported_window(src, dst)`, and hands the id to the broker inside `Open`. LG's own flow lets either
the app or its pipeline do the import (`lumatv:compositor-c2-result.md`, section 7); which one does it for
LG's apps was not recorded. For a native app on webOS 5+ the import happens inside the caller, and it has
only been seen with exporter and importer in one process (`docs/webos5-port.md` 3.3,
`docs/webos10-lab-report.md` 2.2: sixteen Loads, twenty-two placements, all `rv=1`). Cross-process import
is unproven; LumaTV's GFN app depends on the same assumption and lists it as its probe step 9
(`lumatv:gfn-app-result.md`). It is M1.

**V2, if V1 fails.** The broker owns a 16 px window of type `_WEBOS_WINDOW_TYPE_LUMATV_VIDEO` that exports
the element, as the camera and wallpaper work does (`lumatv:hw-planes-result.md`); the UI window sits above
it. This exists only under LumaTV's compositor. Under LG's compositor the hole lives under the exporting
window (`Eos.PunchThrough`, `lumatv:videoplane-saver-result.md`), so V1 is the only shape there
[INFERENCE], and a failed M1 would confine the Rust UI to LumaTV's compositor with the legacy UI staying
the LG-compositor path.

**V3, last resort.** Single process (slice 2 stays the product).

The UI owns letterboxing, zoom and crop (the placement rectangles), hides the element on background, and
destroys it after the broker reports `Released`. A new exported window is made per Load, as today.

### 3.5 Compositor tiers

| Tier | Compositor | UI path | Rate | Status |
|---|---|---|---|---|
| A | LumaTV, engine hosted in the compositor | `marquee-ui` drives layers over `compositor-protocol`; the compositor draws them into the scan-out page | 120 (100 under 25/50 fps content) | engine-in-compositor built and host-tested, never run on the set; layer scenes for apps ("plugin level 2") not built |
| B | LumaTV | Marquee window, in-process engine, direct scan-out when sole and opaque | 120 only when scan-out of its buffer works, else at most 60 | Home's own window scans out (compositor thread under 1.1 ms); Rockchip-blob buffers in the jail are unproven |
| C | LG's surface-manager | same window path | at most 60: callbacks peak at 60.8/s; the spike held 54.3 fps with 4.8-5.4 % of frames over 20 ms, 0.4 % without its flip pass | spike measured, `lumatv:rustui-result.md` |
| D | no 64-bit path | legacy 32-bit UI | 60 | exists today |

LumaTV's compositor is not live persistently: go-live is paused (compositor frame timing parked by Nitin;
PlxNative's close crash fix `112d9cc` awaits the set). Tier C is therefore the default environment for
months, and slices 5-9 target it first. Tiers B and C are one code path: the UI reads the compositor's
frame callbacks and keys its pacer to the flips it measures, as LumaShow's pacer does.

Marquee does not need LumaTV installed: tier C and the legacy fallback use only LG's stack and Marquee's own
64-bit runtime. Tiers A and B need LumaTV's compositor.

## 4. UI at 120 fps

### 4.1 Renderer

| Option | For | Against |
|---|---|---|
| PlxNative's `plx_gfx` + `plx_ui` on 64-bit | complete and behaviour known | immediate-mode: redraws what it does not cache; no Lucent; glass was budgeted on the dev set's Mali-T820 MP2 (`docs/glass-hardware-budget.md`); nothing measured at 120 Hz |
| **LumaShow `ui` + renderer, Lucent tokens** | retained layers, 16 px damage tiles, glyph-run text from one atlas, glass tiers, motion tracks on the compositor clock, host harness; a full redraw of LumaHome measured 5.1-6.2 ms GPU on this Mali (`lumatv:rustui-result.md`); the same stack as LumaTV's shell, so one Lucent | views must be rewritten; damage-only redraw measured on the host only |
| Slint, Flutter, Qt 6 | mature | one more pass per frame as a window client; Slint's licence excludes embedded; Flutter needs an engine build; Qt rejected by Nitin (`lumatv:rust-shell-plan.md` 0) |

Chosen: the second. Marquee's screens are pure models `update(Input, now) -> Vec<Effect>`,
`view() -> View`, `due_at()` over the kit's focus, flow and pane parts (`lumatv:rust-shell-kit.md`), the
same shape as PlxNative's machines, so stores and planner connect through the existing effect and event
lanes. The renderer is used through `ui::Scene` only; whether the engine runs in the app (tiers B, C) or
in the compositor (tier A) is a host choice behind that interface, the arrangement LumaShow built and the
spike used (`lumatv:/rustui/render/src/host.rs`).

### 4.2 Budget

| Quantity | Value | Source |
|---|---|---|
| Frame at 60 / 100 / 120 Hz | 16.67 / 10 / 8.33 ms | arithmetic |
| Usable GPU at 120 Hz | about 7.5 ms (flip lead 0.5 ms, in-fence cutoff, next frame's CPU overlapped) | `lumatv:ui120-result.md` |
| LumaTV composition pass | 4.1 ms of GPU per composed frame | `lumatv:scanout-research.md` |
| Left for the client when a frame is composed | 7.5 - 4.1 = 3.4 ms | arithmetic |
| QML slide frame under LumaTV's compositor | 11.0-13.5 ms GPU at 60 Hz (a Home frame 8.1-9.0 ms), predicted to miss 120. Round 14 measured the bursts failing (quick 32, picker 35 intervals over 10 ms) with the compositor's own pipeline at p99 4.85 ms and no late frame: the Qt client's cadence, not GPU time, was the limit | `lumatv:ui120-result.md` |
| Mali busy | 10 % idle Home, 30 % at a 60 fps UI, so about 60 % at 120 for the same frames | `lumatv:hardware-census.md` |
| Idle shell frame under the compositor | pipeline p99 4.75 ms at 120 Hz, 0 late, 0 lost | `ui120-result.md` (round 14) |

### 4.3 Living with video

1. **The video costs the UI nothing.** Decoded frames sit in display-engine memory (137 MB of secure
   regions) and are blended below the OSD; the GPU cannot sample them, so there is no blur behind a HUD
   over video, only tinted glass (`lumatv:/rustui/ui/src/kit/pane.rs`, `readingAlpha`).
2. **The hole.** The compositor clears the exported rectangle of the UI plane to transparent before the
   owning window's own pixels, so a HUD drawn in that window shows over the video. On LumaTV's compositor
   the pages are XR24, which the driver treats as constant alpha (`global_alpha_en 1`), so a hole would show
   opaque black until `--page-alpha` (ARGB pages) is on. That finding is read from source and fits the
   recorded symptoms; its check (P0) has not run (`lumatv:hw-planes-result.md`). Under LG's compositor (AB24
   pages) holes work.
3. **Refresh families.** The display engine runs 120 Hz for 24/30/60/120 fps sources and 100 Hz for
   25/50/100 fps; the UI's flip rate is the engine rate divided by `fbdev_vsync_cnt_on_flip` (2 by default:
   60 Hz UI at 120). `--refresh 120` in LumaTV's compositor writes 1 (root) and restores it
   (`lumatv:framerate-result.md`, `ui120-result.md`); an app cannot write it. With it, the UI runs at 120
   over 24/30/60 fps video and 100 over 25/50 fps; under LG's compositor it runs at 60 (50 over 25/50 fps).
4. **Composition.** A sole, opaque, full-screen buffer is scanned out whole. A window with a video hole,
   a dim, a transition or a second picture is composed (4.1 ms). So Browse can be free of the pass, a
   playing HUD is composed on every frame it changes, and a still HUD costs nothing because an unchanged
   screen is not redrawn. Tier A removes the pass: the engine draws into the page and the hole is alpha 0
   in it.
5. **Memory for buffers.** CmaFree reads 4-9 MB while 4K Dolby Vision plays yet allocations succeed by
   migrating pages (30-48 ms each, `lumatv:/docs/PLAN.md`). Window buffers, atlases and the poster cache
   are allocated once at start and before the first Load, never per frame or per open.

### 4.4 Rules

- Draw nothing that did not change: damage tiles, layer caches for what does not move (cards, dock,
  wallpaper), motion as transform and opacity of cached layers.
- No allocation on a frame: the compositor's planning path was proven at 0 allocations over 200 frames
  (`lumatv:/docs/PLAN.md`, Rendering stack); a per-frame `String` or `Vec` in a view is a bug.
- Text is glyph runs from one atlas, warmed at start; no text picture is rasterised on a frame.
- Textures uploaded on a worker; a `wl_shm` upload of one 1080p frame cost the compositor 12-21 ms, so
  nothing the size of a frame is uploaded on the frame thread.
- Shaders compile once and are cached (the card app spent 1363 ms compiling, the largest start-up cost).
- Glass tier follows a forecast of late frames: live blur, cached blur, tint, never a dropped refresh
  (`lumatv:lumashow-lessons.md`).
- One focus engine and one spring; no screen animates its own focus.
- A window client starts its frame at the compositor's callback, so its CPU and GPU time add up inside one
  period unless it renders one frame ahead (the compositor's early callbacks,
  `lumatv:compositor-surfaces-result.md`). Tiers B and C budget the sum, not the GPU alone; tier A overlaps
  them by construction.
- Input comes from Wayland, not SDL: every `wl_seat` is bound (LG's compositor sends a key on the seat of
  the device that pressed it, so one bound seat hears no remote), remote codes arrive as evdev (OK 28 and
  Back 412 in the card app's run), the pointer as `wl_pointer` with hover as a cue only. The key table of
  `docs/remote-keys.md` is re-derived from evdev codes (LG's SDL fork carries that table).

### 4.5 What is realistic

| Situation | Tier C | Tier B | Tier A |
|---|---|---|---|
| Browse, scroll, focus moves | 60; 5 % of animated frames over 20 ms with the spike's flip pass, 0.4 % without it | 120 if the client buffer scans out, else at most 60 | 120 |
| Panel or dialog over Browse | 60 | one window, still scanned out | 120 |
| HUD animating over video | 60 | composed: 120 only if the client draws in 3.4 ms, else 60 | 120 |
| HUD still, video playing | no UI frames | no UI frames | no UI frames |
| Software-decoded video present (section 6.4) | 60 UI; 24p judders 2:3 | 120 UI; 24p can be 5:5 | 120 |

## 5. Start-up and memory on 2 GB

**Anchors (TV unless marked).**

| Quantity | Value | Source |
|---|---|---|
| RAM available at Home | 675-915 MB, depends on LG's preloaded apps | `lumatv:memory-result.md` |
| CMA | 692 MB total, 3-119 MB free (lent to page cache while idle) | same |
| Swap | LG's compressed f2io on eMMC, 614 MB, 300-350 MB used | same |
| PlxNative RSS (webOS 4.10 dev set) | 35 MB at boot, 119 MB browsing, about 152 MiB peak playing, two thirds of it Mali pages; texture cache cap 44 MiB; `requiredMemory` 160 | `docs/distribution.md`, `docs/poster-scheduler-design.md` |
| 64-bit Rust client on the C4 | 132-138 MB RSS (spike), 135 MB (card app) | `lumatv:rustui-result.md`, `lumatv:cardapp-result.md` |
| LumaTV compositor | 51 MB RSS | `lumatv:rust-shell-plan.md` |
| Netflix playing 4K Dolby Vision | 129-200 MB PSS | `lumatv:memory-result.md` |
| 64-bit runtime on disk | 26-37 MB (loader, glibc, Mali blob, heap shim); the card ipk is 29.4 MB | `lumatv:gfx64-result.md`, `lumatv:cardapp-result.md` |
| Card app start | in front 2.4 s after launch; first frame 1.7 s after process start; shaders 1363 ms; page built 133 ms | `lumatv:cardapp-result.md` |
| PlxNative prime time | video 708-833 ms, audio 477-827 ms; foreground to picture 3.5 s | `docs/webos10-lab-report.md` 2.6 (K24, webOS 10.3.1) |

**Budget (targets, not measurements).**

| Part | Browse | Playing | Note |
|---|---|---|---|
| `marquee-ui` | 160 MB | 200 MB | includes Mali pages; posters at most 44 MiB; libass fonts and caches while a subtitle is up |
| `marquee` | 30 MB | 40 MB | Starfish client libraries, ring mapping |
| Command ring | 16 MiB | 16 MiB | shared, counted once in PSS |
| Software decode frame pool | 0 | up to 75 MB | 12 frames of 1080p 10-bit 4:2:0 (6.2 MB each); only while that tier plays |
| Total | about 205 MB | about 255 MB (about 330 with software decode) | against 675-915 MB available |
| `requiredMemory` | measure; the shipped 160 is below the Rust client's baseline | | SAM asks LG's memory manager for this at launch |
| CMA | 3 window buffers, 24.9 MB if contiguous | same | allocated before the first Load |

**Start-up targets.**

| Step | Target | Basis |
|---|---|---|
| SAM launch to broker ready (probe, registration, rings) | 150 ms | the configd probe took 19 ms (`TV`, C4 log); the rest is process start [INFERENCE] |
| Process start to first UI frame | 1.5 s; 1.0 s with a shader binary cache | card app 1.7 s with 1363 ms of shaders; cache support of the blob unknown |
| First frame to cached Home content | 0.5 s | the page built in 133 ms; image cache and stored session |
| Play pressed to picture, LAN direct play | 3 s | prime 0.7-0.8 s plus Load; K24 resume 3.5 s |

`appinfo.json` carries `"transparent": true` (the existing key LumaTV's GFN app also sets) so the hole
shows; `splash.png` covers SAM's launch gap. The broker starts first and answers the probe before the UI
asks, so the UI never waits on luna during boot.

## 6. Playback: paths and formats

### 6.1 The decision path

The planner (`route/plan.rs`, `decision.rs`, the learned-refusal store, `devcaps`) stays and gains two
tiers. It runs in `marquee-ui` before any media byte is fetched, from the PMS `Stream` fields (codec,
profile, level, bit depth, chroma, colour transfer, `DOVI*`, frame rate, bitrate, scan type, channels,
audio profile), the broker's `Welcome` (codec table, DV answer, audio stack, sound output) and the viewer's
mode (Auto, or Force Direct Play which never converts). Every route will carry its reasons (the failure
path already reports codec classes and the refusal path in telemetry), so the player's info panel can badge
it; the badge itself is designed in `docs/marquee/design/`.

| Tier | What it is | Demux, rewrite, decode | Picture | Audio | Server load |
|---|---|---|---|---|---|
| 1 Direct play | original file, any container, elementary streams fed | UI process: FFmpeg demux, Annex-B and param-set fix-ups | LG hardware decoder, video window | LG decoder (AAC, AC-3, E-AC-3, DTS core, Opus stereo, MP3) or PCM | none |
| 2 Direct play with rewrite | tier 1 with the stream edited in the app (Stashy's "remux", which on this TV has no container half) | UI process | LG hardware decoder | as 1, or re-encoded in the app | none |
| 3 Software decode | video LG cannot decode, inside a CPU budget | UI process: FFmpeg decoders on NEON, GL upload and shader | UI plane (SDR, 1080p and below) | audio-only Load, LG decoder | none |
| 4 Plex transcode | server `directStream` remux or HLS rungs, today's ABR | PMS; UI demuxes its output | LG hardware decoder | as 1 | yes |

Order is by what the viewer loses and what the server pays: 1 loses nothing, 2 loses at most a lossless
audio track, 3 loses HDR and resolution above 1080p but leaves the server idle, 4 re-encodes. Software
decode is chosen over a server transcode only when the source is SDR, at most 1080p and inside the
measured CPU class; otherwise the server is asked. The server's own `directStream` remux (video copied,
audio converted) is the fallback of the audio rewrite, not a step before it.

**Where the old machinery sits.** Auto's Original-versus-HLS utility decision, the HLS rung controller and
its failure frontier, the open-recovery ladder (facet conversion, both-facets encode, app decode) and the
learned-refusal rows (`docs/adaptive-playback.md`) are pure machines and move unchanged. They act on
Starfish through `Pause`, `Play`, `Flush` and `Seek`; "Starfish accepted Pause" becomes the `VerbDone`
event.

### 6.2 Format matrix

**The C4's own codec table** (`/etc/umediaserver/device_codec_capability_config.json`, `TV`,
`lumatv:gfn/tv-2026-10-26/device_codec_capability_config.json`). PlxNative reads it and logs, on the C4:
`devcaps: hevc=true 4096x2176 vp9=true av1=true audio=aac,ac3,eac3,dts rows: h264=4096x2304@60
hevc=4096x2176@60 vp9=4096x2304@60 av1=4096x2304@60 vp9bits=8 (device table)`. The file's `H.265` row is
4096x2304@120 and its `HEVC` row 4096x2176@60; duplicate names merge by minimum. The table has no bit-depth
or chroma axis.

**Containers.** Because the pipeline is fed elementary streams, the container is the app's business.

| Container | In the build today | Marquee |
|---|---|---|
| Matroska, WebM | yes | yes |
| MP4, MOV, M4V, 3GP | yes (`mov`) | yes |
| MPEG-TS, M2TS | yes (`mpegts`); PMS HLS segments too | yes |
| Raw H.264 and HEVC | yes | yes |
| AVI, MPEG-PS and VOB, ASF and WMV, FLV, Ogg, RealMedia | no (`--disable-everything`; five demuxers enabled) | slice F1: build flags and parsers only |
| Disc images, BDMV, DVD folders | no | not planned |

**Video.**

| Codec and profile | C4 table | Tier today | Marquee |
|---|---|---|---|
| H.264 High, 8-bit | 4096x2304@60, 50 Mbit/s | 1 (`TV`) | 1 |
| H.264 Hi10P, 4:2:2 | no axis for either | refused or transcoded | 3 up to 1080p, else 4; whether the decoder takes Hi10P is unknown (M4) |
| HEVC Main, Main10 | 4096x2304@120 and 4096x2176@60, 50 Mbit/s | 1 (`TV`, 2024 set: 4K DV P8, 25 GB over the internet) | 1 |
| HEVC 4:2:2, 4:4:4, 12-bit | decoder output maps are tiled 4:2:0 HEVC 8/10-bit; none for 4:4:4, 4:2:2 not stated [INFERENCE] (`lumatv:opennow-result.md`) | 4 | 3 up to 1080p SDR, else 4 |
| VP9 profile 0 | 4096x2304@60 | 1 | 1 |
| VP9 profile 2 (10-bit, HDR) | `vp9bits=8` on webOS 10 | 4 | 4 (SDR 1080p may use 3) |
| AV1 | 4096x2304@60, 60 Mbit/s; YouTube loads it on this set | 1 where the table lists it | 1 |
| MPEG-2, MPEG-1, MPEG-4 ASP (DivX, Xvid), VC-1 (WMV), H.263, VP8, MJPEG, RealVideo | rows exist at 1920x1088@30, 40 Mbit/s | not in the Load vocabulary (`H264 H265 AV1 VP9`) | 3 (cheap codecs); tier 1 only if M4 shows Starfish takes them |
| VVC, 8K, wider than 4096 | VVC row 4096x2304@60 | no | 4 |

The table's `maxBitRate` is 50 Mbit/s for H.264, HEVC and VP9, 60 for AV1, 40 for the legacy codecs. A UHD
remux peaks above 50. Whether the pipeline enforces it is unmeasured (M4).

**HDR and colour.**

| Format | Today | Marquee |
|---|---|---|
| SDR, BT.709 and BT.2020 | yes | same |
| HDR10 (PQ, static metadata in-band) | yes, in-band SEI | tier 1; `TV` record on the C4 itself is missing (M7) |
| HDR10+ | SEI stripped (`TV`: 1500 messages in one session); plays as HDR10 [INFERENCE] | same |
| HLG | in-band VUI; never recorded on webOS 10 | tier 1, M7 |
| Dolby Vision P5 | declare node, direct play | same |
| Dolby Vision P8.1, 8.4 | declare node, direct play (`TV`: P8 + Atmos on a 2024 set) | same; `tv.config.supportDolbyHDRContents` is `supported` on the C4 (`TV`) |
| Dolby Vision P7 (UHD Blu-ray remux) | converted to 8.1 in-app as `dovi_tool -m 2`; an enhancement layer's detail is dropped | same |
| Dolby Vision P10 (AV1) | declared like P8; measured on a C1 only | same, M7 on the C4 |
| Dolby Vision P4, P9, others | refused, transcode | same |
| HDR through software decode | n/a | never: the UI plane is SDR; HDR sources go to tier 1 or 4 |

**Audio.**

| Codec | Pipeline path | Marquee |
|---|---|---|
| AAC (LC, HE; ADTS) | native, 6 ch | tier 1 |
| AC-3 | native, 6 ch | tier 1 |
| E-AC-3, with Atmos (JOC) | native, 8 ch; `contents.immersive:"ATMOS"`; the Denon read "Dolby Atmos - DD+" over Netflix (`TV`) | tier 1; Plex-originated Atmos to the receiver not yet read (M5) |
| DTS core | table row `DTS`, announced on the C4; refusal learning exists (#695) | tier 1 |
| DTS-HD MA, HRA, DTS:X | table rows `DTSH` and `DTSE` exist (their meaning is not documented here); the app feeds the core only | tier 1 on the core; lossless is not carried |
| TrueHD, with Atmos | no table row; no Load vocabulary entry (libpf, read on webOS 4.4.3) | decoded in the app (7.1 bed, objects dropped); Load PCM is stereo, so a surround track needs tier 2 or the server |
| Opus | Auto feeds up to stereo natively (5.1 stalls on a C1, plays on a C5) | tier 1 up to stereo |
| MP3, MP2 | native | tier 1 |
| Vorbis | not fed natively (a C1 refuses every Vorbis Load) | app decode to PCM stereo, or tier 2 |
| FLAC, ALAC, PCM | no native decoder; app decodes to PCM stereo up to 48 kHz, resampled above | tier 1 as PCM; surround goes to tier 2 |
| Multichannel lossless | PCM input is mono, stereo or dual in libpf (read on webOS 4.4.3) | tier 2 (new): decode, then AC-3 5.1 at up to 640 kbit/s; a 7.1 source folds to 5.1 |

The NDL header for webOS 5+ (webosbrew's `NDL_directmedia_types.h`) lists a `6-channel` PCM mode with a
capability callback ("current settings support multi-channel PCM"). That is the one lead for multichannel
PCM through the same pipeline and is M5. TrueHD and DTS-HD MA bitstream passthrough and multichannel PCM
also depend on the receiver and on `sound.soundOutputDigital` (`pcm`, `auto`, `passThrough`); the repo
records the receiver as an AVR-S760H over eARC (`lumatv:/docs/SURFACES.md`), and LumaTV already reads the
receiver's input-signal text (`lumatv:volume-osd-result.md`), which is the instrument for M5. Whether a
TrueHD or DTS-HD MA bitstream reaches the receiver is [UNKNOWN]; until M5 says otherwise the plan assumes
it does not.

PlxNative's rule that a source of more than two channels is never mixed down to stereo by default (its
owner's decision of 2026-10-28) carries over unless Nitin changes it (section 11).

**Subtitles.** All client-rendered; the TV's subtitle engine is unreachable in buffer-feed.

| Kind | Path | Marquee |
|---|---|---|
| SubRip, WebVTT, mov_text, SAMI, other text | libass through a synthesised header (`text_ass`), sidecar `.srt`/`.vtt` converted to one script | same, 64-bit libass |
| ASS, SSA | libass with the authored script, fonts attached to the file, karaoke, movement, signs; 22 Noto faces plus Inter | same |
| PGS | FFmpeg `pgssub` bitmap decoder, drawn as textures scaled by the subtitle canvas | same |
| VobSub, DVB | `dvdsub`, `dvbsub` bitmaps, canvas from the decoder (720x480 DVD) | same |
| Burn-in | server re-encode only | same |
| Over HDR video | OSD is SDR; white level over HDR not recorded | visual check in M7 |

libass is built with FreeType, HarfBuzz, FriBidi and libunibreak (one local patch for more font faces); the
five rebuild for aarch64 from the same pinned sources [INFERENCE].

### 6.3 Tier 2: rewrites in the app

Container work is not needed. What exists: Annex-B conversion with parameter sets at each keyframe,
`hev1` handled, AV1 temporal units with in-band sequence headers, VP9 passed whole (PQ metadata injected on
webOS 4), HDR10+ SEI removal, Dolby Vision P7 to 8.1 (RPU rewritten, enhancement NAL dropped), DTS core
extraction, PCM decode with resampling. New:

- **Surround audio re-encode.** TrueHD, DTS-HD MA, FLAC and PCM surround, Opus above stereo and Vorbis are
  decoded and re-encoded to AC-3 5.1 (448-640 kbit/s) with FFmpeg's native LGPL encoder; a 7.1 source folds
  to 5.1. E-AC-3 is an option where the encoder proves solid [INFERENCE: not checked]. Channels are kept;
  objects and lossless are lost; the server does no work; the codec's frame is 32 ms. It replaces the server
  conversion as the first answer and is a setting (Auto, Off). Build change: `--enable-encoder=ac3` and a
  channel-layout path (`swresample` is disabled today).
- **Level fix-up.** A stream that declares a level above the pipeline's (HEVC 5.2 on a 5.1 decoder) has its
  parameter-set level byte rewritten, as LumaTV's GFN probe streams do; only with an M4 result that the
  pipeline refuses the original.
- **Playability preflight.** Stashy's idea of deciding before fetching: the planner marks a route
  "doomed" from PMS metadata (for example HEVC 4:4:4) so no failed Load is spent on it.

### 6.4 Tier 3: software decode

Scope: SDR, at most 1080p, 8 or 10-bit 4:2:0 or 4:2:2, at most 30 fps (60 for the cheap codecs), progressive
or deinterlaced. HDR, 4K and 120 fps never go here. Hardware facts: three usable A76 cores at 1.4 GHz (LG
offlines CPU3 at Home), about 10-12 % of one core already used by `pqcontroller` and input, SoC 56-57 C idle
(`lumatv:hardware-census.md`). **No decode cost has been measured on this CPU**; the budget class is M6.

- Decode: FFmpeg native `h264`, `hevc`, `mpeg2video`, `mpeg4`, `vc1`, `wmv3`, `vp8`, `vp9`, frame threads
  at online cores minus one; planar output stays planar.
- Present: planar frames upload as textures on a worker, a fragment shader does the YUV matrix, range and
  scaling (and a bob or blend deinterlace), and the layer sits under the HUD in the UI plane. No `swscale`.
- Clock: an audio-only Starfish Load (`adaptiveStreaming.audioOnly`) is the master [INFERENCE: no audio-only
  Load has been run]. Its 5 Hz position ticks are interpolated with the monotonic clock and video frames
  are scheduled against it with the PCM lead the broker reports.
- Cadence: the output stays at 120 Hz with no video source, so 24p is 5:5 only when the UI flips every
  refresh (tiers A and B with `--refresh 120`); under LG's 60 Hz UI 24p shows 2:3 judder, 25 fps judders on
  either, and LG's picture engine (FRC, film cadence, sharpening) does not touch the OSD. The planner
  prefers tier 4 over tier 3 for 24 and 25 fps content under tier C.
- Guard: if the decoder cannot keep pace for N seconds the player offers the server transcode at the
  current position; it never drops into a slideshow silently.
- Raw frames into LG's pipeline (`video/x-raw` BUFFERSTREAM) would restore the video plane and the picture
  engine. Whether libpf accepts it is unknown (M6).

### 6.5 Tier 4: Plex transcode

Unchanged. PMS `/decision` and `directStream`, HLS rungs, the ABR controller, Original probes, the
transcode-seek flights and the bandwidth governor stay in `route`, `abr` and `plex`, with the fix already
in upstream for the 4K sink ceiling on webOS 10 (`docs/webos10-resource-allocation.md`).

## 7. Reuse and rewrite

| Area | Production lines | Verdict | Work |
|---|---|---|---|
| `plx_plex`, `plx_data`, `plx_session`, `plx_machine` | 19k, 25k, 4.6k, 3.4k | reuse | aarch64 build; `identity` names the product Marquee |
| `plx_base` | 7k | reuse | `dynlib` stops being used for firmware libraries |
| `media`: `route` 4.5k, `abr` 7.6k | 12k | reuse | none |
| `media`: `ff`, `aq`, `hls`, `ff_pcm`, `speed_audio`, `ff_subs*` | 6k | reuse | FFmpeg 9.0 for aarch64 with the wider component list (section 6.2); ABI check |
| `media::player` engine, pump, shared, lifecycle, recovery, report, transport | 12k | reuse with surgery | `RemoteSink`; pipelined feed; drop ACB, segment seek, era tables for old webOS |
| `media::player` ass, text_ass, sidecar, subside | 3.5k | reuse | 64-bit libass stack |
| `media::curlio`, `plx_net::net` | 2.1k, 4.1k | rewrite | rustls HTTPS source with read, seek, size, abort; Plex roots and the SPKI pin (`base::spki`); the helper in LumaTV already uses rustls 0.23 |
| `plx_net::stream` | 0.9k | reuse | none |
| `plx_platform` | 12.7k | split | devcaps parsing, i18n, image cache, storage client stay; `webos`, `keymanager`, `tv` entries go to the broker |
| `plx_telemetry` | 11.9k | decision | Sentry Native is a C build and the endpoints are upstream's (section 11) |
| `plx_ui`, `plx_appkit`, `plx_screens` | 43k, 8.6k, 28.8k | rewrite | views on the LumaShow toolkit; PlxNative's screen rules and tests are the spec |
| `plx_gfx`, `src/system.rs` | 14.6k, 0.8k | drop | replaced by the engine and a Wayland window |
| `src/app` loop, input, dispatch, adapters | 18.9k | rewrite | key map from evdev codes, pointer from `wl_pointer`, all `wl_seat`s bound |
| `src/starfish.c`, `player/ffi.rs`, `webos.rs`, `keymanager.rs`, `main.c`, `crashtrace.c`, the `tv` entries | about 4k | move | into the broker |
| `plxnative-storage` | small | keep | activation hint comes from the broker |
| Tests | about 445k | reuse where they test logic | `tests/mock_pms.py`, fixtures and flight rigs; host harness `HostSink` becomes the fake broker |

Dropped for webOS 10: ACB (`libAcbAPI`, deleted at webOS 5), `SeekMode::Segment` and its private offsets,
the webOS-3 string ABI, SDL, SDL2_ttf, nanosvg, jsmpeg (`ci/dependencies.json`).

**Repository policy.** New crates beside the existing ones (`marquee-ipc`, `marquee-broker`,
`marquee-port`, `marquee-ui`); existing crate paths are kept until slice 10 so upstream fixes merge. The UI
toolkit comes from LumaTV by pinned git revision (`lumatv:/rustui/tools/sync-lumashow.sh` copies verbatim today).
Marquee is GPL-3.0-or-later; LumaTV and LumaShow are MIT, which may be included in it, so code flows
LumaTV to Marquee and not back, and the compositor stays a separate program behind Wayland and the
`compositor-protocol` socket. `TRADEMARKS.md` excludes the PlxNative name, logo, splash and `pkg/icon*`
from the grant: Marquee ships new identity assets.

## 8. Slices

Shippable means: installable beside PlxNative under a new app id, `make check` green, the default path no
worse on the set, new paths behind a switch. Sizes are one worker working serially (S under a week, M
1-3, L 3-6, XL over 6) [INFERENCE: no velocity data for this codebase]; slices on the same line can fan
out.

| # | Slice | Scope | Ships | Size | Needs |
|---|---|---|---|---|---|
| 0 | Fork hygiene | product name, app id `org.nphil.marquee`, packaging, CI, upstream-merge policy; `marquee-ipc` crate (types, codec, versioning, fake broker, conformance suite) | a Marquee build with PlxNative's behaviour | S | nothing |
| 1 | TV proofs | probe package, M1-M5 (section 10) | decision record, no product change | M | Nitin at the set; Face ID install |
| 2 | Sink over the contract, one process | `RemoteSink`, rings, pipelined feed, async verbs, broker thread in the same process; drop ACB and segment seek | Marquee 0.1, same behaviour | L | 0 |
| 3 | Two processes | `marquee` as `main` and launcher, legacy UI as child, SAM bridge, watchdogs, cross-process window | 0.2: a hung Unload or a fault in LG's libraries no longer takes the UI process with it | M | 1 (M1), 2 |
| 4 | 64-bit core | aarch64 toolchain in CI (zig, glibc 2.36 bundle), FFmpeg and libass 64-bit, rustls transport, `marquee-port`; headless core that browses PMS and plays through the broker | CLI mode for development | L | 3 |
| 5 | Rust UI shell | window as the card app makes it (`wl_webos_shell`, `KEYS_BACK`, every seat), pointer, keys, engine in process, tokens, kit, exported element and placement, Wayland bindings from `lumatv:/compositor/crates/wl-protocols`, sign-in, profile picker, plain player | opt-in "Marquee UI (beta)" | L | 4; LumaTV toolkit |
| 6 | Browse parity | Home, Library, Detail, Search with LumaTV's keyboard, Watchlist, Person, item menus | beta | XL (fans out by screen) | 5 |
| 7 | Player parity | HUD, chapters, track menus, info panel, Skip Intro and Credits, Up Next, speed, subtitles, route badge | beta | L | 5 |
| 8 | Settings and account | settings families, profiles and PINs, privacy, legal, diagnostics | beta | M | 5 |
| 9 | Default flip | Rust UI default, legacy kept one release as fallback | 1.0 | S | 6-8 and the parity list (`docs/ux-scenario.md`) with frame gates |
| 10 | Legacy removal | delete the 32-bit UI, SDL, `plx_gfx`, `plx_ui`, ACB, firmware-library loaders | 1.1 | M | 9, one release of use |
| F1 | Demux breadth | AVI, MPEG-PS, ASF, FLV, Ogg; parsers; fixtures | more containers | M | 4 |
| F2 | Audio rewrite | AC-3 and E-AC-3 encode, channel layouts, passthrough policy from M5 | lossless surround without the server | M | 4, M5 |
| F3 | Software decode | tier 3 and its planner gate, deinterlace shader, audio-only clock, guard | legacy codecs | L | 5, M6 |
| F4 | Stream surgery extras | level-byte rewrite, preflight "doomed" list | fewer failed Loads | S | M4 |
| A | Tier A | engine hosted in LumaTV's compositor, damage-only redraw, direct scan-out with a hole, `--refresh 120` | 120 fps | M on this side | LumaTV: engine-in-compositor measured, page alpha, persistent go-live |

Slices 6, 7, 8 and F1-F4 do not depend on each other; most of the wall-clock saving is there.

**First proof steps on the set** are slice 1 (section 10): M1-M5. M6 gates F3, M7 and M8 run inside slice 5,
M9 and M11 gate tier A, M10 closes each beta. Slice 3 cannot start until M1 passes; slice 5 cannot start
until M2 passes under LG's compositor.

**TV tooling.** The set is driven with LumaTV's tools, not PlxNative's dev FIFO: its frame gate
(`lumatv:/compositor/tools/tv_gate.py`, `lumatv:/docs/research/perf/gate.py`), key injection into the
remote's evdev device (`lumatv:/docs/research/perf/kb.py`), the capture and `ctl` surfaces of its
compositor, and the receiver read-out. PlxNative's playback cases (`tests/run.py`, mock PMS) port onto
them; the host side uses LumaTV's Mesa-backed UI harness and the fake broker.

## 9. Risks

| Risk | Effect | Evidence | Mitigation |
|---|---|---|---|
| Cross-process window import fails | V1 dead; Rust UI limited to LumaTV's compositor (V2) | in-caller import only seen same-process; GFN carries the same assumption | M1 first; V2 and the legacy UI are the fallbacks |
| LumaTV's compositor never goes live | UI stays at 60; tier A never happens | go-live paused; PlxNative's close crash fix unproven; frame gate failing rounds 14 and 17 | tier C is a full product; 120 is an upgrade |
| Synchronous verbs over IPC jank the frame | dropped frames during seek, pause, prime | about 70 call sites; Starfish verbs already block the frame thread | async contract with a 2 ms shim; M3 measures verb latency |
| Jailed buffers are not contiguous | no scan-out of the UI window; composed every frame | `dma_buf_unified` pages, 56 fps class; `dma_buf_te` and `dri/card0` absent in the jail | M9 tries the exporter's contiguous request; tier A removes the client buffer |
| 2 GB, LG's memory manager | LG closes Marquee or Netflix; swap thrash | 675-915 MB available; f2io swap 300-350 MB used | budget in section 5; M10 |
| Software decode misses its budget | stutter, heat | nothing measured | M6 before F3 is scoped; guard to the server |
| Surround audio unsupported by the pipeline | lossy conversions | PCM input stereo only (webOS 4.4.3 read); TrueHD absent from the table | tier 2 re-encode; M5 may open 6-channel PCM |
| HDR and Dolby Vision on the C4 unrecorded | regression unseen | lab log is a 2024 K24 set; the C4's own logs are boot lines | M7 |
| Upstream divergence | merge pain, lost fixes | upstream moves daily | keep crate paths until slice 10; merge weekly |
| Rockchip Mali blob licence | redistributing it in a GPL package may not be allowed | the blob is fetched at build time and never committed; terms not read here | Nitin's decision (section 11) |
| Install friction | every TV round needs Face ID | LumaTV's standing rule | batch probes; one grant per round |
| Driver or firmware update breaks the 64-bit path | no UI | `mali_kbase` interface 11.40 accepts 64-bit callers today | `launch.sh` fallback to the legacy UI; firmware fingerprint |
| Close-time crash in EGL teardown | app dies on exit | PlxNative under LumaTV's compositor (r15-r17) | SIGTERM `_exit`, 8 s swap watchdog, M2 close test |

## 10. Needs a television first

Run in this order. Each needs the set and a probe package installed through Nitin's Face ID approval; no
worker installs on its own. Pass criteria are conditions on the log and the receiver.

| # | Measurement | Decides | Pass |
|---|---|---|---|
| M1 | A 32-bit process with no SDL, no GL, loads Starfish `BUFFERSTREAM` with `option.windowId` of an element exported by a 64-bit process (the GFN probe's step 9, with Starfish); direct play 1080p and 4K DV P8; 10 pause/seek/unload cycles; background and foreground; once under each compositor | V1, slices 3-5 | picture, audio sync, `placed rv=1`, no leak in `proc` after 10 cycles |
| M2 | 64-bit window under SAM's jail: transparent hole over playing video, HUD animation, close; frame gate numbers (`tv_gate.py`, `gate.py` above): p99 and frames over 20 ms at 24p and 60p; under LG's compositor and LumaTV's | tier C and B numbers; the close crash | no crash in 15 closes; gate figures recorded |
| M3 | IPC at 100 Mbit/s HEVC: CPU of both processes, feed-to-accepted latency, `BufferFull` behaviour, Unload under load; latency of each verb (`Play Pause Seek Flush SetVolume`) | ring versus socket; which sites need the event path | less than 2 % of a core for the ring; verbs answered inside 2 ms or listed |
| M4 | Sustained bitrates 50, 80, 100 Mbit/s HEVC Main10 4K24 and H.264 1080p through `BUFFERSTREAM`; H.264 Hi10P and HEVC 4:2:2 Loads; MPEG-2, VC-1, MPEG-4 ASP `codec.video` names; HEVC level 5.2 | table limits, tier 3 scope, F4 | counters from `streamQualityInfoNonFlushable`: no drops or black frames, or the first failing rate |
| M5 | Audio to the receiver: Plex E-AC-3 Atmos; DTS core; TrueHD and DTS-HD MA Loads; `pcmInfo.channelMode` `6-channel`; each with `soundOutputDigital` `pcm`, `auto`, `passThrough`; read the receiver's input-signal text per clip (muted, as the repo's rounds do) | tier 2 policy, F2 | the receiver's text per case |
| M6 | Software decode: H.264 Hi10P and HEVC 4:2:2 at 1080p24, MPEG-2 1080i, VC-1 1080p24, MPEG-4 ASP 720p, three threads: CPU of all cores, dropped frames, temperature; 1080p planar upload and shader cost at 60 and 120 Hz; audio-only Load clock jitter; `video/x-raw` Load | tier 3 scope, F3 | frames on time at the stated class |
| M7 | HDR on the C4: HDR10, HLG, DV P5, P8.1, P8.4, P7 to 8.1, AV1 P10, VP9 profile 2; picture-mode switching; subtitle white level over HDR | the HDR column | each case plays with the right panel badge |
| M8 | Wayland input: every seat bound, colour and media keys, Magic Remote hover, click, wheel; the evdev code table | slice 5 key map | all keys of `docs/remote-keys.md` reach the UI |
| M9 | Contiguous buffers in the jail (`DMA_BUF_UNIFIED_ALLOC_CONT`), `--scanout direct` on a Marquee window, page alpha with a hole | tier B's 120, tier A scope | direct frames counted; hole transparent |
| M10 | Cold and warm start; RSS and PSS per process; CmaFree before and during 4K DV; whether SAM or the memory manager closes other apps for a given `requiredMemory` | section 5 budget | targets met |
| M11 | UI rate follow: `--refresh 120` over 24p, 30p, 60p; 100 Hz over 25p and 50p; pacer lock time | tier A and B claims | rate within 1 % of the family |

## 11. Decisions for Nitin

Defaults taken where nothing blocks the work are marked; the rest need him.

| # | Decision | Recommendation |
|---|---|---|
| 1 | Target only the C4 on webOS 10.2.1; drop ACB and the webOS 3-9 paths | yes (default taken) |
| 2 | App identity: `org.nphil.marquee`, own package and repo manifest, installed beside PlxNative | yes (default taken) |
| 3 | UI toolkit: LumaShow's, rewriting PlxNative's views, not a 64-bit port of PlxNative's | yes |
| 4 | Accept 60 fps under LG's compositor and call 120 a LumaTV-compositor feature | yes |
| 5 | Ship the 64-bit runtime inside Marquee's package (self-contained, +30 MB) versus requiring LumaTV's | inside the package, after the Mali blob's redistribution terms are checked; no public release before that |
| 6 | Telemetry (Sentry, PostHog): strip, or point at his own endpoints | strip for now; local crash logs only |
| 7 | Software decode scope: SDR at most 1080p | yes; no HDR or 4K ever |
| 8 | In-app AC-3 re-encode of lossless surround, default Auto | yes, after M5; no downmix to stereo by default (PlxNative's rule) |
| 9 | Keep the legacy 32-bit UI as the fallback child until slice 9, then one more release | yes |
| 10 | Process model: `main` is one process (broker and launcher) versus a supervisor plus re-exec'd broker | one process first (default taken); split if M1 shows the role covers it |
| 11 | Every package install and every release goes through his Face ID and his OK | unchanged |

## Appendix: sources

PlxNative: `docs/agent-reference.md` (runtime architecture, linking, key files),
`rust-modules/media/src/player/CLAUDE.md`, `src/starfish.c`, `rust-modules/platform/src/tv/sink.rs` and
`tv.rs`, `ci/module-layers.ini`, `docs/module-layers.md` (L15, L15b), `docs/sans-io.md`,
`docs/async-model-decision.md`, `docs/adaptive-playback.md`, `docs/dolby-vision.md`,
`docs/webos5-port.md`, `docs/webos10-lab-report.md`, `docs/ass-subtitles.md`, `docs/remote-keys.md`,
`ci/build-ffmpeg.sh`, `ci/dependencies.json`, `rust-modules/platform/src/devcaps.rs`.

LumaTV: `docs/PLAN.md` (Rendering stack), `docs/SURFACES.md`, and in `docs/research/`: `rust-shell-plan.md`,
`rust-shell-kit.md`, `rustui-result.md`, `ui120-result.md`, `gfx64-result.md`, `cardapp-result.md`,
`gfn-app-result.md`, `opennow-result.md`, `scanout-research.md`, `hw-planes-result.md`,
`layer-deep-result.md`, `framerate-result.md`, `memory-result.md`, `hardware-census.md`,
`videoplane-saver-result.md`, `volume-osd-result.md`, `compositor-c2-result.md`,
`compositor-surfaces-result.md`, `compositor-gate9-result.md`, `lumashow-lessons.md`,
`compositor/slot-2026-10-28-r14/appcompat/`, `compositor/slot-2026-10-29-r17/apps`,
`gfn/tv-2026-10-26/device_codec_capability_config.json`.

Stashy (separate repository): `docs/ENGINEERING_NOTES.md` section 5 (direct play, then on-device remux,
then server), `docs/ROADMAP.md` (playability report, seek by re-init).
