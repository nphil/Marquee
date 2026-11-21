//! **The loop at rest while another app has the screen.** Between the background edge and the
//! foreground edge (`WindowActivity`) the loop runs no frame: no page tick (so no timer, no hero
//! page turn, no hub, Requests or Continue Watching refresh, no Settings look-ahead or
//! seek-preview), no art request, no draw, upload or present, no heartbeat or memory sample. Each
//! parked turn takes the events that arrived, does the main-thread work the background edge left
//! owed (`run::parked_turn`) and blocks for up to [`PARK_WAIT_MS`] for the next event.
//!
//! Measured on the TV before (2026-11-20): a backgrounded Home woke the loop 60 times a second,
//! turned the hero 55 times and cost a GeForce NOW session about 680 s of main-thread CPU.
//!
//! The iteration that receives the background edge still runs whole, so the tree is suspended and
//! the keyboard dismissed before the first parked turn.

/// The longest a parked turn waits for an event: the most the loop wakes is once a second.
pub(crate) const PARK_WAIT_MS: u32 = 1000;

/// What one loop iteration is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Turn {
    /// An ordinary frame.
    Frame,
    /// The first frame after a park: the paused work starts again.
    Resume,
    /// The first parked turn.
    Enter,
    /// A later parked turn.
    Park,
}

#[derive(Default)]
pub(crate) struct Park {
    parked: bool,
}

impl Park {
    /// This iteration's turn. `background`: the window is not the app's and nothing else drives
    /// frames (a recording, a replay or a controlled run never parks).
    pub(crate) fn turn(&mut self, background: bool) -> Turn {
        let was = std::mem::replace(&mut self.parked, background);
        match (was, background) {
            (false, true) => Turn::Enter,
            (true, true) => Turn::Park,
            (true, false) => Turn::Resume,
            (false, false) => Turn::Frame,
        }
    }
}

/// **What the background edge gives back** (GL thread, on [`Turn::Enter`]): the art and strings
/// the last frame did not draw, the Requests answers kept on disk, the seek stills and their
/// decoder, and the freed heap. A game streamed over the app wants the memory: a parked Home held
/// 206 MB resident with 167 MB of the TV in swap (2026-11-20). Each comes back as it is drawn or
/// opened again; a parked playback's own state is left alone.
pub(crate) fn release() {
    use plx_gfx::gfx::tex_ledger;
    let (rss, gpu) = (rss_kib(), tex_ledger::totals().1);
    plx_ui::tex::release_hidden(&mut super::adapters::poster::GfxUploader);
    plx_gfx::text::release_hidden();
    super::bridge::release_held_answers();
    plx_media::seekprev::release();
    // SAFETY: glibc's malloc_trim only returns free heap pages to the system.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
    let textures = gpu.saturating_sub(tex_ledger::totals().1) >> 20;
    let freed = rss.saturating_sub(rss_kib()) >> 10;
    plx_base::eventlog::log(&format!("background: released ~{freed} MB (textures {textures} MB)"));
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| super::adapters::poster::proc_kib(&s, "VmRSS:"))
        .unwrap_or(0)
}
