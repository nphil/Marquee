# Install Plex on LG webOS TV with Marquee

Marquee is an unofficial, open-source Plex app for LG webOS TVs. It is not yet available in the LG Content Store, but you can install it on a regular LG webOS TV using LG's Developer Mode and a computer. **No root is required.** If something goes wrong, see [troubleshooting](troubleshooting.md).

The upstream Homebrew catalogue distributes PlxNative, not Marquee. Install this fork from its
[releases](https://github.com/nphil/Marquee/releases) using the steps below.

## Before you start

You will need:

- an LG TV running **webOS 4.0 or newer** (or webOS 3.4.3, tested on one set); check the [compatibility notes](../README.md#will-it-work-on-my-television), since playback support varies by model;
- a Plex account with access to a **Plex Media Server**, either your own or one shared with you;
- a Mac, Windows PC, or Linux computer for the setup described here;
- the TV and computer on the **same local network**.

The route is **set up the TV → connect from your computer → install Marquee**. No LG SDK or IDE is required.

> **Important:** Developer Mode needs periodic renewal. If it expires and LG disables Developer Mode, apps installed through it are removed. [How to keep the apps installed](#important-developer-mode-expires).

## 1. Enable Developer Mode on the TV

1. **On your computer**, visit [LG's developer website](https://webostv.developer.lge.com/) and select **Sign In**. Sign in or create an account, completing any account verification LG requests.
2. **On your TV**, open **LG Content Store / Apps**, search for **Developer Mode**, and install it.
3. Open the **Developer Mode** app and sign in with that LG account.
4. Turn **Dev Mode Status** on. The TV will restart.
5. Open **Developer Mode** again, confirm **Dev Mode Status** is on, and turn **Key Server** on.
6. Keep this screen open. You will need the TV's **IP address** and the **Passphrase** displayed here to connect from your computer. The IP address is also available in the TV's network settings.

[LG's official guide](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app) has screenshots of these controls. **Do not continue into its CLI or webOS Studio setup** for this installation; use Dev Manager in the next step instead.

**Ready to continue:** Developer Mode is on, Key Server is on, and you have the TV's IP address and Passphrase.

## 2. Connect your computer to the TV

**webOS Dev Manager is the app on your computer.** It connects to the Developer Mode app on your TV and installs packages for you.

1. [Download the latest webOS Dev Manager](https://github.com/webosbrew/dev-manager-desktop/releases/latest). In **Assets**, choose the installer for your computer: `.dmg` for macOS, `.msi` for Windows, or the appropriate Linux package. The project's [download table](https://github.com/webosbrew/dev-manager-desktop#download) explains the architecture choices. Do not download **Source code**.
2. Install and open Dev Manager. Start its setup wizard to add a TV and select **Use Developer Mode**. Do not select the Homebrew Channel SSH option; that is for an existing rooted setup.
3. The wizard also lists the TV preparation steps. Since you completed them above, mark them complete and continue to the connection form.
4. Fill in the connection details:

   | Field | What to enter |
   |---|---|
   | **Device Name** | A name you choose, such as `living-room-tv`. |
   | **Address** | Your **TV's local IP address**, not your Plex server's address. |
   | **Passphrase / authentication info** | The **Passphrase shown in the TV's Developer Mode app**, with the same capitalisation. This is **not your LG account password**. |

   Keep the Developer Mode defaults: **Username** `prisoner`, **Port** `9922`, and **Authentication** `Dev Mode`. The wizard sets these for you.
5. Finish adding the TV. In Dev Manager, open **Apps** and check that the **Installed** tab can load the TV's apps.

**Ready to continue:** Dev Manager can display the TV's installed apps. If it cannot connect, see [Connection problems](#connection-problems) before downloading Marquee.

## 3. Install Marquee

1. On your computer, open [Marquee releases](https://github.com/nphil/Marquee/releases) and select a release.
2. Under **Assets**, download **`com.beb.plxnative_X.Y.Z_arm.ipk`**. `X.Y.Z` is the release's version number. The `.ipk` is the TV app, not something to open on your computer; the manifest and source archives are not installers.
3. For a manual download, [check the package against the release checksum](#verifying-the-package) before installing it.
4. In Dev Manager, select your TV, open **Apps**, and click **Install**. Choose the downloaded `.ipk`.
5. Wait for installation to finish and check that **Marquee** appears under **Installed**.

Continue to [step 4](#4-open-marquee-and-sign-in).

For future updates, repeat this with the newer `.ipk`. Install over the existing app rather than uninstalling it first.


## 4. Open Marquee and sign in

Open **Marquee** from the TV's app launcher. Scan the on-screen QR code and sign in to the Plex account that has access to your server or shared libraries. Choose your Plex Home profile if prompted.

**Installation is complete.** You can now use Marquee from the TV launcher. The remaining setup responsibility is keeping Developer Mode active, as described below.

Marquee is free and open source. If it's useful to you, you can [support it on Ko-fi](https://ko-fi.com/0xbeb).

## Important: Developer Mode expires

For installations made through LG Developer Mode, including Homebrew Channel installed that way:

1. While the TV is online, open the **Developer Mode** app and check **Remain Session**.
2. **Before the remaining time runs out**, select **EXTEND**.
3. Check that the remaining time has increased. Repeat before the next expiry; a calendar reminder can help.

LG does not let you extend an already-expired session. It documents that restarting the TV after expiry disables Developer Mode and removes apps installed through it. Re-enable Developer Mode and reinstall the apps if this happens.

Do not turn off Developer Mode after installation. **Key Server is different:** it is needed when initially adding the TV to Dev Manager, not for everyday playback, and it may turn off after a restart.

See [LG's session guidance](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app#extending-developer-mode-time) and [webOSbrew's Developer Mode notes](https://www.webosbrew.org/devmode/).

## Connection problems

**Dev Manager cannot reach the TV:** check that the TV is on, both devices are on the same local network, and you entered the TV's current IP address. Guest Wi-Fi, device isolation, or a VPN can prevent a local connection.

**Adding the TV fails at authentication:** reopen Developer Mode on the TV, enable **Key Server**, and copy the current **Passphrase** exactly. In Dev Manager, use **Use Developer Mode**, not the rooted-TV SSH option or password authentication.

**The apps disappeared:** check **Dev Mode Status** and **Remain Session**. If Developer Mode has been disabled, enable it again and reinstall the apps using the same route.

**Marquee opens but your libraries are missing:** make sure you signed in to a Plex account with access to a server and that the server is reachable. Marquee does not include a media library or set up Plex Media Server for you.

For connection help, see [webOSbrew's troubleshooting guide](https://www.webosbrew.org/devmode/#troubleshooting). To [report a Marquee problem](https://github.com/nphil/Marquee/issues), include the TV model, webOS version, Marquee version, installation method and exact error. Do not post passwords, passphrases, Plex tokens or screenshots containing them.

If Marquee itself is installed and opens but something inside it doesn't work — sign-in, playback, or anything else — see [Troubleshooting](troubleshooting.md).

---

## Checking what you downloaded

The installation walkthrough ends above. The sections below are a technical reference for package verification, release provenance, and what the app reads, writes, and connects to.

The app runs in LG's normal sandbox: an unprivileged uid, chrooted, under the stock jail profile. `appinfo.json` declares two ACGs, `database.operation` and `securitykey.operation` — both are the storage helper's, for the local DB8 keystore and the platform key manager it uses to keep your sign-in off plain disk; neither reaches the network or anything outside this app's own data.

Check the selected [Marquee release](https://github.com/nphil/Marquee/releases) for its package and release information.

### Which file to download

A release may attach the following files. The `.ipk` is the application.

| File | What it is |
|---|---|
| `com.beb.plxnative_X.Y.Z_arm.ipk` | The app. |
| `com.beb.plxnative.manifest.json` | Package metadata, when supplied. |
| `ipk.sha256` | The checksum, for `sha256sum -c`. |
| `ffmpeg-<version>.tar.xz` | The pristine upstream FFmpeg source, published because we are obliged to. |
| `build-ffmpeg.sh` | The complete configure invocation that produced the bundled FFmpeg libraries. |

### Verifying the package

Nothing in this distribution chain is code-signed, so the SHA-256 published with the release is what tells you that the file you downloaded is the file that was published there.

Download `ipk.sha256` from the same release as the `.ipk` and open a terminal in the download folder. Replace `X.Y.Z` with the version you downloaded. On macOS and Windows, compare the printed hash with the one in `ipk.sha256`; on Linux, the check below should report `OK`. **Do not install the package if the hashes do not match.**

```sh
shasum -a 256 com.beb.plxnative_X.Y.Z_arm.ipk              # macOS
sha256sum -c ipk.sha256                                    # Linux, with the checksum asset beside it
certutil -hashfile com.beb.plxnative_X.Y.Z_arm.ipk SHA256 # Windows
```

[Return to installation](#3-install-marquee) after checking the file.

#### Rebuilding it to compare

Two builds of one commit on one machine produce a byte-identical `.ipk`. It is **not** reproducible across machines yet — the bundled FFmpeg records the toolchain paths it was built against — so a hash from your own rebuild will differ, and that is not evidence of tampering. Each audit's **Reproducibility evidence** section shows exactly which paths a given package carries.

Check the selected release for its source and verification information.

## What the app does on your television

This section is invariant across releases. Where a release changes one of these behaviours, its release note says so and its audit measures it.

### What it writes

Both entries below are restricted to the app, but not at the same mode. The three logs are mode `0640` — owner read/write, the app's own group read, nothing for anyone else — so that on a television that is not rooted, Dev Manager should be able to fetch them and you can attach them to a bug report. The signed-in session is mode `0600`, owner only.

- `/tmp/plxnative-events.log`, `/tmp/plxnative-stderr.log` and `/tmp/plxnative-crash.log` (mode `0640`) — the first two are truncated each launch, while the crash log is append-only so it survives a restart. Because the mode includes group read, another app installed on the same television that runs in Marquee's group can read these three files too; the event log is scrubbed as described next, and so is the panic text the app appends to the crash log, but the crash tracer's own records (a faulting address, registers, one memory-map line) and the stderr log (whatever aborts and the television's own libraries print) are not. Every event-log line is scrubbed **before it is written**: tokens, header and query credentials, hostnames (including `plex.direct` names that encode your LAN address), bare addresses, Plex GUIDs, search queries, and your server and profile names are rewritten. Media titles, search terms, and subtitle text are never written at all. What remains includes ratingKeys — server-local item numbers used to diagnose playback bugs. Someone with access to the same server could map one back to an item, so still think before posting a log publicly. [`PRIVACY.md`](https://github.com/GLinnik21/plx-native/blob/main/PRIVACY.md) is the full contract.
- Your signed-in session (mode `0600`), as `<id>-auth.json` under `/media/developer` or `/media/internal` — one access token per server your account can reach. Marquee capability-probes the documented `com.webos.service.keymanager3` service (TV 24+) and uses its AES-GCM operation when LS2 policy permits it. The older `com.palm.keymanager` AES-CFB service is deliberately not used because it cannot authenticate ciphertext. webOS TV 4.10.2 has neither usable service, so the compatible result there is an atomically replaced, app-owned mode-0600 file. A protected file is never silently downgraded during a temporary service failure. The probe is an ordinary application-service call and the fallback needs no root service or root-only HAL API; store entitlement for Key Manager is still capability-tested at runtime rather than assumed from the OS version.

The app does not persist the last screen: an authenticated cold launch starts on Home. Upgrades remove the retired `<id>-lastplace.json` bookmark written by older builds.

A crash writes no core file.

### What it reads outside its own directory

The television's codec table at `/etc/umediaserver/device_codec_capability_config.json`, and its firmware identity at `/var/run/nyx/os_info.json` and `/var/run/nyx/device_info.json`. All three are published by the platform, read once at boot, and never written.

### What it reaches

`plex.tv` and `discover.provider.plex.tv` over TLS, and the Plex Media Servers your account can reach — your own and any shared with you — over HTTPS whenever a token is present, except the home-network exception below.

Stable builds refuse token-bearing plaintext HTTP, with one exception that is yours to make: when a server answers only unencrypted on your home network — every secure route to it, Plex's relay included, failed — the app asks **Connect without encryption?**: on the sign-in screen, on the Home or library read-out that reports the failure, or when you switch the server on in Settings → **Unencrypted connections**. It is offered only for a numeric private address plex.tv lists as local and on the same network as the television, never for a remote address or the relay, and never for a server set to require secure connections. The answer is remembered per server for the Plex account that gave it, so another account signing in is asked again. The permission it grants is never stored: it holds only while each fresh check of that server still reaches the same address, and it ends when you sign in or out, switch to a profile that does not use that server at that address, or bring the app back from the background — the app cannot see the television change network, so those are the points where it proves eligibility again. While it is in use the app keeps retrying HTTPS and switches back as soon as a secure route verifies, and Settings → **Unencrypted connections** turns it off at once. Developer-trigger builds may enable plaintext for a local lab and log that exception.

**Only if enabled**, the app can send crash reports and usage analytics. The included [privacy notice](../PRIVACY.md) is inherited from upstream and does not identify Marquee's data controller or support contact.

Nothing is sent anywhere else. A build carries an endpoint only if one was compiled into it, so `strings` on the binary answers the question directly, and each release audit reports what it found there.

### What listens

Nothing. A release build compiles out the whole `/tmp` trigger surface, the remote-control FIFO, and the TCP capture listener that exist in a development build. Each audit measures this on the shipped bytes rather than asserting it.

## Scope

Movies and TV shows from a Plex Media Server your account can reach. No music, no photos, no live TV, no DVR.

There is deliberately nowhere on the television to type a server address — configure servers on a phone or PC, and the app offers what your Plex account already knows about.

## The bundled FFmpeg

The package contains three FFmpeg shared libraries — `libavformat-plx.so.63`, `libavcodec-plx.so.63`, and `libavutil-plx.so.61` — built from **FFmpeg 9.0**, unmodified, and licensed **LGPL-2.1-or-later**.

They contain demuxers, parsers, bitstream filters, subtitle decoders, and audio decoders only: video is decoded by the television's own hardware, and so is audio except FLAC, ALAC and LPCM, and the DTS core where Force Direct Play or a refused open asks the app for it, which are decoded in the app and fed to the television as PCM, and any audio while a playback speed other than 1x is chosen, which the app decodes and time-stretches.

The complete corresponding source accompanies every release, as LGPL-2.1 §6 requires and not as a courtesy: `ffmpeg-<version>.tar.xz` (the tarball `ci/dependencies.json` pins) is the pristine upstream tarball with no patches applied, and `build-ffmpeg.sh` is the complete configure invocation that produced the libraries.

It is built with `--disable-everything` plus an explicit component list, and **without** `--enable-gpl`, `--enable-version3`, or `--enable-nonfree`, so no GPL or non-free FFmpeg component is present. Each audit quotes the configure string recorded inside `libavutil` itself, which is the primary evidence for that.

They are ordinary shared libraries, `dlopen`ed by absolute path out of the app's own directory under exactly those names, so they can neither shadow nor be shadowed by the television's own FFmpeg — and a build of your own with the same names replaces ours.

Full licence text travels inside the package, in `THIRD-PARTY-NOTICES.md` and `licenses/`.

The bundled build is configured with `--disable-network` and `file` as its only protocol, so it cannot open a URL at all; everything it demuxes arrives through the app's own transport.
