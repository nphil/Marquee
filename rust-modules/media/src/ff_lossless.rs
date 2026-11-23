//! **DTS-HD fed whole to the receiver.** The television's own DTS decoder takes the DTS stream the
//! Load names `DTS`; until now the demuxer cut every packet down to its core (`ff::dts_core_packet`),
//! so a DTS-HD Master Audio, High Resolution or DTS:X track reached the receiver as lossy DTS. When
//! the sound chain proves the receiver takes the bitstream over eARC ([`plx_platform::tv::earc::Chain::passes_dts_hd`]),
//! the packet is fed as the file holds it and the television passes the extension substreams on
//! (the stock Plex app does the same on this set). TrueHD has no input on the pipeline and takes its
//! own path ([`crate::truehd`]); nothing here is TrueHD's.
//!
//! **Not yet measured on the set.** The fallback is the old feed: a refused
//! open before the first picture takes the core back for the rest of the request ([`veto`], the
//! pump's `retry_core_audio`), and a debug build's `nolosslessaudio` trigger turns it off.
use std::sync::atomic::{AtomicBool, Ordering};

/// The request has taken the whole feed back.
static VETO: AtomicBool = AtomicBool::new(false);
/// This session's demuxer feeds DTS whole.
static WHOLE: AtomicBool = AtomicBool::new(false);

/// A new play request starts: whatever the last one vetoed is forgotten.
pub(crate) fn new_request() {
    VETO.store(false, Ordering::Release);
    WHOLE.store(false, Ordering::Release);
}

/// A refused open fed whole DTS: the rest of this request feeds the core, as before.
pub(crate) fn veto() {
    VETO.store(true, Ordering::Release);
    WHOLE.store(false, Ordering::Release);
}

/// What the plan may count on, without a bus call: the last read of the chain passes DTS-HD and
/// nothing has taken the feed back. [`wanted`] is the open's own, fresh answer.
pub(crate) fn planned() -> bool {
    !VETO.load(Ordering::Acquire)
        && !plx_base::devtrig::flag("nolosslessaudio")
        && plx_platform::tv::earc::cached().is_some_and(|c| c.passes_dts_hd())
}

/// The demuxer says what this session feeds ([`wanted`]'s answer).
pub(crate) fn publish(whole: bool) {
    WHOLE.store(whole, Ordering::Release);
}

/// The session feeds DTS whole right now.
pub(crate) fn fed_whole() -> bool {
    WHOLE.load(Ordering::Acquire)
}

/// Whether this session should feed DTS whole: the chain passes it, the request has not taken it
/// back, and a debug build has not turned it off. Blocks (one bus read, cached): the demuxer's open.
pub(crate) fn wanted() -> bool {
    if VETO.load(Ordering::Acquire) || plx_base::devtrig::flag("nolosslessaudio") {
        return false;
    }
    plx_platform::tv::earc::chain().is_some_and(|c| c.passes_dts_hd())
}

/// A DTS packet the TV's decoder can take as it is: a 16-bit big-endian core sync word first, the
/// extension substreams after the core. The other three sync spellings (little-endian and the 14-bit
/// pair) are converted by `dts_core_packet`, which keeps only the core, so they stay core feeds.
pub(crate) fn feeds_whole(packet: &[u8]) -> bool {
    packet.starts_with(&[0x7f, 0xfe, 0x80, 0x01])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_big_endian_core_packet_is_fed_whole() {
        assert!(feeds_whole(&[0x7f, 0xfe, 0x80, 0x01, 0, 0]));
        // little-endian and 14-bit cores, an extension-only packet and nothing
        assert!(!feeds_whole(&[0xfe, 0x7f, 0x01, 0x80]));
        assert!(!feeds_whole(&[0x1f, 0xff, 0xe8, 0x00]));
        assert!(!feeds_whole(&[0x64, 0x58, 0x20, 0x25]));
        assert!(!feeds_whole(&[]));
    }

    #[test]
    fn a_veto_lasts_until_the_next_request() {
        let _guard = plx_base::testlock::serial();
        new_request();
        publish(true);
        assert!(fed_whole());
        veto();
        assert!(!fed_whole() && !wanted(), "the old feed, for the rest of the request");
        new_request();
        assert!(!fed_whole());
    }
}
