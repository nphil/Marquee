# Ports

Planned ports beyond webOS: Android TV, Android tablets and iPhone. The port plan (shared Rust
core versus platform playback, input, system integration and packaging, with milestones) is not
written yet. This file collects the requirements already fixed for it.

## iPhone: LumaTV remote

The iPhone app embeds LumaTV's phone remote, for example as a Remote tab or sheet. Remote input
comes from the shared Swift package `LumaRemoteKit`, which has no Marquee dependency and also
ships as a standalone remote app. Marquee builds no input code of its own for this. The remote is
input only and does not change what the media player does.

The kit provides gyro air-mouse aiming, a trackpad mode with scrolling, typing into the focused
TV field as text, the remote keys, LumaTV actions (including Marquee playback controls),
discovery over mDNS (`_lumatv-remote._tcp`) and wake from standby. Pairing uses a code shown on
the TV, then a per-device key, revocable from the TV. The TV side is the LumaTV `phone-remote`
plugin, off by default.

The protocol, message formats and text-entry routing are defined in
`/data/home/lumaglass/docs/research/phone-remote.md` (LumaTV repo); kit code waits for that
document. Added latency is a target under 20 ms on Wi-Fi, to be measured.

There is no Swift toolchain on the Unraid build host, so iOS builds run on GitHub Actions macOS
runners, following Stashy's `.github/workflows/ios-build.yml`. The iOS project consumes
`LumaRemoteKit` as a Swift package (local path or SPM) built the same way.
