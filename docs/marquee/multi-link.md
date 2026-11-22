# Multi Link

A direct-play film can be fetched over more than one of the TV's links to the Plex server's
network: the built-in 100 Mbit/s wired port, Wi-Fi and a USB Ethernet adapter. LumaTV's Multi
Link plugin prepares the links and says which exist. Marquee's playback engine proves each one
and decides which carry. Plex's other requests (metadata, timeline, subtitles, images) stay on
the default route.

This page describes what the code does under its tests, which run it against a simulated network
(`rust-modules/media/src/multilink/tests.rs`). What the TV has shown, and what it has not, is at
the end.

## When it runs

Only for a direct-play library part of known size, open at the address it was asked for. A
redirected open, a conversion, a segmented stream and a server reached with a pinned key are
read as before.

The helper publishes one state on loopback
(`GET http://127.0.0.1:18731/v1/state/network.multi-link`). A worker beside the demuxer asks for
it every 10 s while there is none, every 2 s while it offers nothing and four times a second
while it offers links. The state
counts only while it is enabled, active, of schema 1 and no older than it vouches for (2 s at
most), and only its IPv4 addresses on the wired port's subnet are links.

With no such state the reader does nothing: no request is made over any link, the default
reader does all the work, and the film starts no later than without the plugin.

## Proof

A link carries nothing until 64 KiB fetched over a connection bound to its address equals, byte
for byte, what the default reader delivered for the same range, under an answer that names the
same file: a `206` for the range asked, the size the default reader opened, and an unchanged
strong `ETag` (else `Last-Modified`). The wired port is always proven, as the place a failed
range goes. Of the other links the most promising is proven, and a further one only when the
proven ones do not cover the film. Redirects are never followed. An answer that is not the file
ends Multi Link for that film.

## Which link carries

The stream moves from the default reader to the links once a link other than the wired port is
proven. The file ahead of the demuxer is cut into ranges of about 150 ms at the fetching link's
rate (128 KiB to 1 MiB), each fetched over one link and read out in file order.

A link that has not carried yet shows its rate over its first eight ranges. The fastest then
carries alone while it covers the film, which is 115% of the film's mean bitrate. The others
join when it does not. They also join while less than 8 s of film is queued, until 16 s are,
unless the fastest alone brings four times the film's bitrate.

While another link carries too, the wired port is given ranges at no more than 70 Mbit/s, so the
TV's own traffic and its remote controls keep the rest. Where the wired port alone at its full
rate carries more than the others beside that share, it carries alone.

Every turn reads each connection until it is empty, so while the queues fill the wired port
brings its share and every other link what it carries. libcurl 7.53 hands over one TLS record
(16 KiB) of a connection a call, and one call a turn held a link to what the turns brought.

## Memory

What the links have fetched or asked for ahead of the demuxer is part of the read-ahead reserve,
not beside it: 8 MiB of the reserve's allowance, given up by the video queue. The allowance
(half of the available memory above 224 MiB, 192 MiB at most) stays the one limit on what a film
holds beyond its queues' base sizes. When memory is short there is no allowance and no room, and
the default reader takes the stream back. A film that Auto quality supervises takes no
read-ahead for its queues, and its links keep their 8 MiB.

## When a link fails

A request unanswered after 1 s, an answer that stalls for 500 ms or is cut short, and a link
that changes address or leaves the helper's state each cost the link its ranges. Another link
finishes them from the byte they reached, within a second. The failed link is proven again after
2 s, doubling to 30 s.

Only what a link's connections bring while they are read counts against it. A range is held
against its plan from its first byte, not from when it was asked: a connection made anew spends
that wait on its handshake. A range that falls behind while its link still brings bytes moves to
another link, and the link is planned at the slower rate. A range whose link has gone quiet
moves too when the demuxer would wait for it, but its request stays out: the link fails if
500 ms pass without a byte and carries on if one comes. Time the demuxer spends away from the
connections is held against no link, and neither is a link that nothing is asked of.

With no proven link left, or a helper state that stops vouching for itself, the default reader
reopens at the read position and the film goes on over the wired port.

## Log

Lines in `/tmp/plxnative-events.log`:

| Line | Meaning |
| --- | --- |
| `multilink: helper state usable\|missing\|stale\|disabled\|inactive\|malformed\|unknown schema` | The helper's state changed. `missing` is the plugin not installed or not running. |
| `multilink: not used for this file: <why>` | The film is not eligible. |
| `multilink: <link> proven` | The link's proof matched. |
| `multilink: <link> rejected: <why>` | The link's proof did not match: it carries nothing for this film. |
| `multilink: <link> not proven (<why>), again in N s` | The proof did not finish. |
| `multilink: carrying from byte N` | The links took the stream from the default reader. |
| `multilink: using <link> N Mbit/s (held to 70), <link> N Mbit/s; failovers N` | The links that carry now, at their measured rates. |
| `multilink: fill N Mbit/s (<link> N, <link> N) reserve N→N ms in N ms` | The queues were filled, after a start, a seek or a stretch that ran them down: what the links brought together and each alone, the least and the last media queued, and how long it took. A fill under a second has no line. |
| `multilink: <link> failed (<why>), failovers N` | A carrying link lost its ranges. |
| `multilink: <link> is slow, its range goes to another link` | A range was moved before its link failed. The link is planned slower, or heard for 500 ms if it has gone quiet. |
| `multilink: <link> set aside at N Mbit/s` | The wired port carries alone at its full rate. |
| `multilink: <link> may carry again` | A link proven again after a failure is trusted again. |
| `multilink: <link> is no longer offered` | The helper dropped the link. |
| `multilink: handing back at byte N: <why>` | The default reader takes the stream back. |
| `multilink: off for this file: <why>` | An answer was not the file. |
| `feed: read-ahead allowance video=NMiB audio=NMiB ranges=NMiB span=Ns` | The reserve's allowance. `ranges=0MiB` leaves the links no room. |

## On the TV

One run, at `ca33f3bdf` (`/data/home/tmp/marquee-multilink-20261122/RESULTS.txt`): Wi-Fi and the
wired port carried an 89 Mbit/s film over TLS for seven minutes and two seeks, with no answer
refused and no hand-back. The answers to the Wi-Fi-bound connections arrived, their return
traffic leaving over the wired port. The links brought 145 Mbit/s together at best over 10 s,
and failed over nine times in four and a half minutes, each link proven again within seconds.

Not tried there:

- What changed after that run: a late answer, a pause and the demuxer's absence costing no link,
  and each connection read until it is empty.
- A fill at 200 Mbit/s and more. One thread decrypts every connection beside the demuxer's own
  work.
- The server keeping its connections over many range requests, and what a connection made anew
  costs after a seek.
- The wired port's 70 Mbit/s share against the remote controls' response time while another link
  carries at its whole rate.
- The cost of the default reader's reopen after a hand-back.
- A USB Ethernet adapter.
