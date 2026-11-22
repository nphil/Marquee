# Multi Link

A direct-play film can be fetched over more than one of the TV's links to the Plex server's
network: the built-in 100 Mbit/s wired port, Wi-Fi and a USB Ethernet adapter. LumaTV's Multi
Link plugin prepares the links and says which exist. Marquee's playback engine proves each one
and decides which carry. Plex's other requests (metadata, timeline, subtitles, images) stay on
the default route.

Not tried on the TV: the plugin is not installed there. This page describes what the code does
under its tests, which run it against a simulated network
(`rust-modules/media/src/multilink/tests.rs`).

## When it runs

Only for a direct-play library part of known size, open at the address it was asked for. A
redirected open, a conversion and a segmented stream are read as before.

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
| `multilink: <link> failed (<why>), failovers N` | A carrying link lost its ranges. |
| `multilink: <link> is slow, its range goes to another link` | A range was moved before its link failed. |
| `multilink: <link> set aside at N Mbit/s` | The wired port carries alone at its full rate. |
| `multilink: <link> may carry again` | A link proven again after a failure is trusted again. |
| `multilink: <link> is no longer offered` | The helper dropped the link. |
| `multilink: handing back at byte N: <why>` | The default reader takes the stream back. |
| `multilink: off for this file: <why>` | An answer was not the file. |
| `feed: read-ahead allowance video=NMiB audio=NMiB ranges=NMiB span=Ns` | The reserve's allowance. `ranges=0MiB` leaves the links no room. |

## Not tried on the TV

- A connection bound to a second address reaching the server and its answer coming back, on the
  TV's libcurl 7.53.
- The server keeping one validator and its connections over many range requests.
- The wired port's 70 Mbit/s share against the remote controls' response time while another link
  carries.
- TLS on a bound connection. A server reached with a pinned key is not eligible.
- The cost of the default reader's reopen after a hand-back.
