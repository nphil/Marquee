# Ports

Planned ports beyond webOS: Android TV, Android tablets and iPhone. This file collects the
requirements fixed for them.

## iPhone

The iPhone app is a separate, clean-room codebase: `nphil/marquee-ios` (MIT, SwiftUI, iOS 27). It
shares behaviour with the TV app, not code; its parity list is `docs/PARITY.md` in that repo.

### LumaTV remote

LumaTV control lives inside the iPhone app; there is no standalone remote app. Remote input comes
from the Swift package `LumaRemoteKit` (`nphil/LumaRemoteKit`), which Marquee embeds and which
has no Marquee imports, so the remote can later become its own app as a thin shell around the same
views. The kit owns the protocol, pairing and stored keys (its own Keychain group and settings
namespace), discovery over mDNS (`_lumatv-remote._tcp`), wake, the air mouse, the trackpad, the
keyboard bridge and the remote's SwiftUI views (`LumaRemoteView`, `PairingView`, `TVPicker`),
tinted by the host. Marquee builds no input code of its own and reaches the kit through a small
host interface: the Remote screen, the URL hand-off and optional hooks such as "play this on the
TV".

- **Remote screen:** a full-screen Remote opened from a remote button beside the profile button on
  Home (the tab bar holds Home, Library, Live TV, Watchlist and Search). It shows `LumaRemoteView`.
- **Pairing:** a code shown on the TV plus OK on the TV's physical remote, then a per-device key,
  revocable from LumaTV's `phone-remote` plugin page. Marquee owns the `lumaremote://` link and
  scans the TV's QR code itself.
- **Air mouse:** hold to point by default (the pointer moves while a thumb rests on the pointer
  pad), with always-on as a Remote setting.
- **Typing:** the phone's keyboard types into the focused TV field, including YouTube, Netflix and
  Prime Video.
- **Volume buttons:** while the Remote screen is open, the phone's volume buttons drive the TV or
  receiver volume.
- **Install:** sideloaded like Stashy (free signing, re-signed every 7 days).

The protocol and its decisions are in `/data/home/lumaglass/docs/research/phone-remote.md`
(LumaTV repo, "Decisions", 7d0801d0). Added latency is a target under 20 ms on Wi-Fi, to be
measured. iOS builds run on GitHub Actions macOS runners; the kit keeps its own CI with
`swift test`.
