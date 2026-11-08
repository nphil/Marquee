# Marquee: guide for agents

Marquee is a fork of PlxNative (`GLinnik21/plx-native`), a native Plex client for rooted LG
webOS TVs, maintained for one household. `AGENTS.md` and `docs/agent-reference.md` remain the
architecture, build and verification reference; this file covers what the fork adds or does
differently, the machine it is developed on and the services it talks to. Where the two disagree on
workflow (pull requests, releases, the TV), this file wins.

## Repository and workflow

- Remotes: `origin` is `nphil/Marquee` on GitHub; `upstream` is `GLinnik21/plx-native`. Marquee is
  GitHub-only: there is no forge mirror and its CI does not run on the Proxmox runner (owner
  decision, 2026-11-03).
- No pull requests. A change goes on a short-lived branch, `gh workflow run ci.yml` and
  `gh workflow run simulators.yml` are dispatched on it, and when both are green on the same SHA
  `main` is fast-forwarded to it and the branch deleted. Pushes to `main` run CI again.
- The full test suite is too heavy for the shared build host (below), so GitHub CI is the gate;
  locally only the tests a change touches are run.
- Author-only workflows are disabled in the repository settings: Build bench, CI metrics, Issue
  labels, Nightly, Deploy landing page, Release candidate, Site video. Release publishes on a `v*`
  tag.
- Upstream changes are merged by hand after the owner reviews them; there is no automatic sync.
  Merge in a separate worktree and keep every fork behaviour.
- User-visible changes get a line under "Unreleased" in `docs/marquee/changelog.md`.
- Commit messages are short and third person: what changed and why.

## Name and identity

The display name comes from one place: `"title"` in `pkg/appinfo.json`. `rust-modules/platform/
build.rs` exports it as `PLX_DISPLAY_NAME`, which becomes `plx_platform::i18n::APP_NAME`
(`@APP_NAME@` in locale strings) and `plx_plex::identity::PRODUCT` (`X-Plex-Product`). Renaming
the app is that field plus new artwork from `tools/mkicons.py`. The webOS app id stays
`com.beb.plxnative`: it keys the install directory, storage and the TV's launch points, so changing
it would orphan the installed app's data.

## Fork documents

- `docs/marquee/changelog.md`: fork changes.
- `docs/marquee/design-subtitles.md`: subtitle look, text handling, "Wrong subtitles?".
- `docs/marquee/subtitles-ui-status.md`: TV acceptance history and proof limits.
- `docs/marquee/ports.md`: Android TV, Android tablet and iPhone requirements. The iPhone app is a
  separate clean-room repository (`nphil/marquee-ios`) with its own owner session.
- Outside the repository: `/data/home/Marquee-BUILD-NOTES.md` (what is installed on the TV, the
  build lane, hashes) and `/data/home/Marquee-release/` (IPKs, including a rollback).

## Build host

Development happens on a shared Unraid container (16 cores, often at load 25 to 50, `/data` around
90 % full) used by many agent sessions at once.

- Run only the affected tests: `nice -n 19 cargo test -j4 -p <crate> <filter>`, with output
  redirected to a file in `$CODY_SCRATCH`. Engine restarts kill piped output and background jobs, so
  a log file is the only reliable result.
- Reuse `rust-modules/target` in `/data/home/Marquee`; never create a target directory per attempt.
  Delete scratch builds when done.
- Container packages can disappear after a host restart. The ARM build needs `build-essential
  cmake pkg-config file bzip2 qemu-user-static libc6-arm64-cross`. The NDK's `ld.bfd` is an x86
  shim that runs the real aarch64 linker through `qemu-aarch64-static` with
  `/usr/aarch64-linux-gnu`, so a missing package fails the link with "exec qemu: No such file or
  directory".
- Some tests cannot run as root; `plx_base::skip_as_root!` skips them with a reason.

## TV lane

The TV is `192.168.1.242`. `/data/home/Marquee-subs` is the build worktree: detached, and it keeps
the warm `target-release` caches, so never build in the source checkout or share its target
directory.

```sh
cd /data/home/Marquee-subs && git fetch -q && git checkout -q --detach <main sha>
export WEBOS_SDK=/tmp/webos-ndk/arm-webos-linux-gnueabi_sdk-buildroot TV=192.168.1.242
export PLX_TV_LOCK_AGENT=<name unique to this agent>
R="--remap-path-prefix=$HOME=/build --remap-path-prefix=$HOME/.cargo=/cargo \
--remap-path-prefix=$HOME/.rustup=/rustup -C link-arg=-fuse-ld=bfd"
make -j16 FLAVOR=stable RELEASE=1 RUST_REMAP="$R"
make ipk FLAVOR=stable RELEASE=1 RUST_REMAP="$R"
./ci/check-elf.sh && python3 ci/check-package.py       # both must pass
tools/tv-lock.sh require --as <who> --why "<reason>"
make FLAVOR=stable RELEASE=1 RUST_REMAP="$R" deploy     # ends "verify-deploy: N files match"
tools/tv-lock.sh release
```

- `make ipk` has a disk preflight. On a nearly full disk `PLX_MIN_FREE_MB=0` skips it; the package
  needs about 0.4 GB.
- `deploy` copies files only. Use `make install` (a real IPK install) when `pkg/appinfo.json` or
  the icons change, because webOS reads them only at install. Its `luna-send` subscription can hang
  after the install finishes; check the installed `appinfo.json`, then run `deploy`, since an IPK
  install leaves the packaged binary in place.
- A running Marquee keeps the old binary after a deploy (`/proc/<pid>/exe` shows "(deleted)").
  Restart it with the Makefile's `CLOSE_SH`/`BOOT_SH` pattern (`luna-send -i … closeByAppId`,
  `fuser -k`, `luna-send -i … launch`; plain `luna-send -n` does not take effect), then check the
  new pid's executable hash.
- Record each install in `/data/home/Marquee-BUILD-NOTES.md`: main SHA, executable SHA-256, file
  count and what was checked on screen. Only code on `origin/main` goes on the TV.

### Sharing the TV

The LumaTV OS session (same Cody install, folder `/data/home/lgtv`) builds the TV's shell and uses
the same screen. Before anything visible (launching apps, playback, IPK installs), check both
`tools/tv-lock.sh status` and that session's current status, and do not start while it is measuring.
Tell it when the TV is released. Never change boot-direct, OLED care, picture settings, compositor
retime or receiver settings. When the owner is watching something, a file-only `deploy` with
Marquee closed is fine; launching or closing apps is not.

Playback checks mute the receiver first (Home Assistant `media_player.living_room_avr`) and restore
its state afterwards. Leave the TV in the state the owner asked for, usually on at LumaTV Home on
his profile.

## Home services

- **Plex Media Server** runs in the `plex` container on Unraid (`ssh unraid`), port 32400. Its token
  is in `Preferences.xml`; read it into a variable and never print it. `allowMediaDeletion` is on and
  the TV signs in as the owner account.
- **Plex-Auto-Languages** runs beside it and rewrites stream selections. A subtitle the server has
  selected is therefore not the viewer's choice.
- **Bazarr** at `https://bazarr.nateshome.net` downloads `<base>.en.srt` translations. The TV gets
  its link through `tools/import-bazarr-link.sh`, which writes a 0600 file owned by the app's jail
  uid; the key never goes into the repository or logs.
- **SpeechDock** (owned by the "Speech 2 Text Stuff" session) writes `<base>.en.cc.srt` transcripts
  of English dubs. Plex lists them as hearing-impaired English. Its word-alignment endpoint
  `POST /api/alignment/windows` accepts them like any English sidecar.

## Fork behaviour worth knowing before editing

- **Subtitles follow the audio** (`media/src/route/plan.rs` `follow_audio`,
  `decision_memory.rs` `follow_audio_with_auto_subtitle`). English audio picks the dub transcript
  (hearing-impaired flag, `.cc`/`.sdh` sidecar, or a title naming SDH, CC, Dub or Dubtitle); other
  audio picks the full translation, a downloaded one over a built-in one. An audio switch re-picks.
  Only a remembered subtitle that actually decided the pick, or the viewer's own pick this playback,
  stops the follow; remembered audio never does. Off and "On-screen text only" are respected.
- **Duplicate subtitles are hidden** (`data/src/metadata/sub_layout.rs` `downloaded_twin`). A
  built-in track is hidden when a drawable downloaded sidecar has the same language, region and
  kind. Any pick that lands on a hidden track moves to its twin, so the tick is always on a visible
  row.
- **Plain words in the UI.** Menus, badges and settings never show SDH, CC, Forced, codec, format
  or release names. Labels read "English", "English (dub)", "English (on-screen text)", "Subtitles",
  "Captions", "Audio description". Only the Tracks info panel keeps technical names. Every string
  goes through `locales/` in all 11 languages, and the text-fit tests catch long translations.
- **Deletion** (`plex/src/plex/delete_media.rs`). Delete is offered only on an owned server with
  `allowMediaDeletion` (`serverinfo::delete_offered`, cached and re-read in the background by
  `refresh_deletion_if_due`). 404 counts as deleted; 401/403 means not allowed; a server error or
  timeout re-checks the item three times before reporting a problem. A delete removes the item from
  every list at once through the view-state ledger, and a covered page whose item (or show or
  season) was deleted leaves when it is uncovered.
