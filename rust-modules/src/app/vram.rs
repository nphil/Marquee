//! **Video memory while hidden.** A Marquee that has drawn and then lost the screen keeps its
//! window's buffers (three, one screen each) and the driver's context; a freshly preloaded one
//! keeps only the context. Once the DID background edge (`0x104`) has arrived the window's
//! `wl_egl_window` is resized to a small surface and swapped three times, so the driver drops the
//! full-size buffers; the DID foreground edge (`0x106`) puts the full size back before the first
//! frame. The SDL window and the GL context are never destroyed (`gfx::egl::release_for_exit`
//! documents what a destroy races with), and the textures and strings are released by
//! [`super::park::release`] on the same edge.
//!
//! Only ever hidden: a frame iteration that may present never shrinks, so a video in front is
//! untouched. `/tmp/plxnative-keepvram` (dev builds) keeps the buffers.
//!
//! [`Hold`] is the state machine and holds no handle, so the edges, fast toggles and the kill
//! switch are tested on the host; [`Window`] is the one place that touches Wayland.
use std::os::raw::c_void;

/// What an iteration owes the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    None,
    /// The app has been hidden by a DID background edge.
    Shrink,
    /// The app was started hidden and nothing has shown it ([`Hold::step`]).
    ShrinkQuiet,
    Restore,
}

/// A failed restore is tried again on the next iterations, this many times in all: a window left
/// small is a black screen.
const RESTORE_TRIES: u8 = 5;

/// How long an app that has had no lifecycle event and no event of any kind is taken for a hidden
/// start. A system keep-alive launch logged no lifecycle event at all (2026-11-24: not even a
/// foreground edge), so a hidden start is told from a shown one only by that silence.
const QUIET_MS: u32 = 5_000;

#[derive(Default)]
pub(crate) struct Hold {
    /// A frame iteration has run in the foreground: the surface has had buffers. A preloaded app
    /// that was never drawn has none, and a swap would be the first thing it ever presents.
    drawn: bool,
    /// The DID background edge arrived and no foreground edge since.
    did_background: bool,
    /// The window is small and a restore is owed.
    shrunk: bool,
    /// Shrinking is given up for this process (it failed, or the platform has no surface).
    off: bool,
    restore_tries: u8,
    /// A lifecycle event (`0x103`..`0x106`) has arrived, or a hidden start was woken: the app has
    /// been shown, and silence no longer means hidden.
    shown: bool,
    /// The small window is a hidden start's, not a DID background's.
    quiet: bool,
    /// Something arrived since a hidden start was shrunk.
    woke: bool,
    /// When the silence began: the first iteration, or the last event or launch.
    silent_since: Option<u32>,
    /// Launch payloads counted so far ([`Hold::launches`]).
    launches: u64,
}

impl Hold {
    /// An event reached the loop at `now` (ms). DID edges drive the background shrink (the WILL
    /// edge may be followed by a return that never leaves the screen); any event at all breaks a
    /// hidden start's silence and wakes a shrunk one.
    pub(crate) fn note(&mut self, event: u32, now: u32) {
        match event {
            0x104 => self.did_background = true,
            0x106 => self.did_background = false,
            _ => {}
        }
        if (0x103..=0x106).contains(&event) {
            self.shown = true;
        }
        self.woke = true;
        self.silent_since = Some(now);
    }

    /// The count of launch payloads offered so far; a change is a relaunch, which is the system
    /// showing the app.
    pub(crate) fn launches(&mut self, count: u64, now: u32) {
        if count != std::mem::replace(&mut self.launches, count) {
            self.woke = true;
            self.silent_since = Some(now);
        }
    }

    /// This iteration's work. `background`: the loop is parked (the window may not present);
    /// `enabled`: the kill switch is not armed and the run is live (not a recording). Answer it
    /// with [`Hold::applied`].
    pub(crate) fn step(&mut self, background: bool, enabled: bool, now: u32) -> Step {
        if !background {
            self.drawn = true;
            self.did_background = false;
            let since = *self.silent_since.get_or_insert(now);
            if self.shrunk {
                return if self.quiet && !self.woke { Step::None } else { Step::Restore };
            }
            let silent = now.wrapping_sub(since) >= QUIET_MS;
            return if silent && enabled && !self.off && !self.shown { Step::ShrinkQuiet } else { Step::None };
        }
        if self.did_background && self.drawn && enabled && !self.off && !self.shrunk {
            Step::Shrink
        } else {
            Step::None
        }
    }

    /// The outcome of the step just returned. `done`: a shrink left the window small (a restore
    /// is owed); a restore needs nothing more.
    pub(crate) fn applied(&mut self, step: Step, done: bool) {
        match step {
            Step::None => {}
            Step::Shrink | Step::ShrinkQuiet if done => {
                self.shrunk = true;
                self.quiet = step == Step::ShrinkQuiet;
                self.woke = false;
                self.restore_tries = 0;
            }
            Step::Shrink | Step::ShrinkQuiet => self.off = true,
            Step::Restore if done => {
                // A hidden start that woke has been shown: it is never taken for hidden again.
                self.shown |= self.quiet;
                self.shrunk = false;
                self.quiet = false;
            }
            Step::Restore => {
                self.restore_tries += 1;
                if self.restore_tries >= RESTORE_TRIES {
                    self.shrunk = false;
                    self.off = true;
                }
            }
        }
    }
}

/// The window's buffers.
pub(crate) trait Backing {
    /// Make the surface small and let the driver drop its buffers. True when it is small now.
    fn shrink(&mut self) -> bool;
    /// Put the full size back. True when nothing is owed any more.
    fn restore(&mut self) -> bool;
}

/// One iteration's work for `hold` on `backing`; the step it took.
pub(crate) fn run(hold: &mut Hold, background: bool, enabled: bool, now: u32, backing: &mut dyn Backing) -> Step {
    let step = hold.step(background, enabled, now);
    let done = match step {
        Step::None => return step,
        Step::Shrink | Step::ShrinkQuiet => backing.shrink(),
        Step::Restore => backing.restore(),
    };
    hold.applied(step, done);
    step
}

plx_base::devtrig::latched_flag!(
    /// `/tmp/plxnative-keepvram`: keep the window's buffers while hidden, as before.
    pub(crate) fn keep_buffers = "keepvram";
);

/// The loop's call, on the GL thread, once per iteration before the park or frame work. `live`:
/// not a recording, a replay or a controlled run.
pub(crate) fn apply(hold: &mut Hold, win: *mut c_void, background: bool, live: bool) {
    let now = super::clock::now();
    hold.launches(plx_platform::tv::launch::arrivals(), now);
    let step = run(hold, background, live && !keep_buffers(), now, &mut Window(win));
    if step == Step::Restore {
        // A hidden start that is shown has no other reason to draw its first frame.
        plx_machine::idle::invalidate();
    }
}

/// The SDL window's `wl_egl_window`, on the television. The simulator has none, so reads nothing.
struct Window(#[cfg_attr(feature = "hostsim", allow(dead_code))] *mut c_void);

#[cfg(feature = "hostsim")]
impl Backing for Window {
    fn shrink(&mut self) -> bool {
        // The simulator's window has no `wl_egl_window`.
        false
    }
    fn restore(&mut self) -> bool {
        true
    }
}

#[cfg(not(feature = "hostsim"))]
impl Backing for Window {
    fn shrink(&mut self) -> bool {
        native::shrink(self.0)
    }
    fn restore(&mut self) -> bool {
        native::restore(self.0)
    }
}

/// `wl_egl_window`'s public head (wayland-egl): `intptr_t version; int width, height`.
#[cfg(any(test, not(feature = "hostsim")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Head {
    version: isize,
    w: i32,
    h: i32,
}

#[cfg(any(test, not(feature = "hostsim")))]
impl Head {
    /// Read the head at `p`. Only for a pointer [`plausible`] has not refused yet, so this is
    /// the one read made on a value SDL did not promise is an `egl_window`.
    unsafe fn read(p: *const u8) -> Self {
        let word = std::mem::size_of::<isize>();
        Self {
            version: (p as *const isize).read_unaligned(),
            w: (p.add(word) as *const i32).read_unaligned(),
            h: (p.add(word + 4) as *const i32).read_unaligned(),
        }
    }

    /// A version Mesa and Mali stamp (1 to 8) and a screen-sized extent: anything else is some
    /// other object in `SDL_SysWMinfo`'s union.
    fn plausible(&self) -> bool {
        (1..=8).contains(&self.version) && (1..=16384).contains(&self.w) && (1..=16384).contains(&self.h)
    }
}

#[cfg(not(feature = "hostsim"))]
mod native {
    use super::Head;
    use plx_base::dynlib::Handle;
    use plx_base::eventlog::log;
    use std::cell::Cell;
    use std::os::raw::{c_int, c_void};

    extern "C" {
        fn SDL_GetWindowWMInfo(window: *mut c_void, info: *mut c_void) -> c_int;
        fn SDL_GetVersion(ver: *mut u8);
    }

    /// `SDL_SYSWM_WAYLAND`, and `egl_window`'s place among the pointers of the union's `wl`
    /// member (`display`, `surface`, `shell_surface`, `egl_window`), which begins at byte 8.
    const SYSWM_WAYLAND: i32 = 6;
    const EGL_WINDOW_SLOT: usize = 3;
    /// The size the surface is shrunk to: small, but a size every driver allocates.
    const SMALL: c_int = 64;
    /// The driver keeps three buffers and reallocates one per swap at the new size.
    const SWAPS: u32 = 3;

    type Resize = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int);

    thread_local! {
        /// The `wl_egl_window` and the size to give back, while it is small.
        static SAVED: Cell<Option<(usize, Head)>> = const { Cell::new(None) };
    }

    /// `wl_egl_window_resize`: in SDL's own scope, else in the libwayland-egl SDL already mapped.
    fn resize_fn() -> Option<Resize> {
        let sym = "wl_egl_window_resize";
        let p = Handle::self_handle()
            .sym(sym)
            .or_else(|| Handle::open_loaded(&["libwayland-egl.so.1", "libwayland-egl.so"]).and_then(|(h, _)| h.sym(sym)))
            .filter(|p| !p.is_null())?;
        // SAFETY: `wl_egl_window_resize(struct wl_egl_window *, int, int, int, int)`.
        Some(unsafe { std::mem::transmute::<*mut c_void, Resize>(p) })
    }

    /// The window's `wl_egl_window` and its head, or why not.
    unsafe fn egl_window(win: *mut c_void) -> Result<(*mut c_void, Head), &'static str> {
        let mut info = [0u8; 512]; // the television's SDL writes more than the header declares
        SDL_GetVersion(info.as_mut_ptr());
        if SDL_GetWindowWMInfo(win, info.as_mut_ptr().cast()) == 0 {
            return Err("SDL_GetWindowWMInfo failed");
        }
        if i32::from_ne_bytes(info[4..8].try_into().unwrap()) != SYSWM_WAYLAND {
            return Err("not a Wayland window");
        }
        let p = (info.as_ptr().add(8) as *const *mut c_void).add(EGL_WINDOW_SLOT).read_unaligned();
        if p.is_null() || p as usize % std::mem::align_of::<isize>() != 0 {
            return Err("no egl_window in SDL_SysWMinfo");
        }
        let head = Head::read(p as *const u8);
        if !head.plausible() {
            return Err("egl_window slot does not hold a wl_egl_window");
        }
        Ok((p, head))
    }

    /// Resize to [`SMALL`] and swap once, so the driver reallocates for the new size.
    pub(super) fn shrink(win: *mut c_void) -> bool {
        // SAFETY: GL thread, the window SDL made; the resize entry point is the library's.
        unsafe {
            let (p, head) = match egl_window(win) {
                Ok(found) => found,
                Err(why) => {
                    log(&format!("vram: kept ({why})"));
                    return false;
                }
            };
            let Some(resize) = resize_fn() else {
                log("vram: kept (no wl_egl_window_resize)");
                return false;
            };
            resize(p, SMALL, SMALL, 0, 0);
            SAVED.set(Some((p as usize, head)));
            for _ in 0..SWAPS {
                super::super::SDL_GL_SwapWindow(win);
            }
            // What the window says after the swaps: SDL resizing it back would show here.
            let after = Head::read(p as *const u8);
            log(&format!(
                "vram: shrunk {}x{} to {SMALL}x{SMALL} ({SWAPS} swaps; header now {}x{})",
                head.w, head.h, after.w, after.h
            ));
            true
        }
    }

    /// Give the full size back, ahead of the first frame.
    pub(super) fn restore(win: *mut c_void) -> bool {
        let Some((saved, head)) = SAVED.take() else { return true };
        // SAFETY: as `shrink`; the window is found again. SDL may have replaced its
        // `wl_egl_window` while hidden (a new pointer at the full size, which needs nothing);
        // whichever window is current and still small is the one given its size back.
        unsafe {
            match egl_window(win) {
                Ok((p, now)) if now.w == SMALL && now.h == SMALL => {
                    let Some(resize) = resize_fn() else {
                        SAVED.set(Some((saved, head)));
                        return false;
                    };
                    resize(p, head.w, head.h, 0, 0);
                    log(&format!("vram: restored {}x{} on window {:p} (shrunk {:#x})", head.w, head.h, p, saved));
                }
                Ok((p, now)) => log(&format!(
                    "vram: restore not needed (window {:p} is {}x{}; shrunk {:#x})",
                    p, now.w, now.h, saved
                )),
                Err(why) => {
                    SAVED.set(Some((saved, head)));
                    log(&format!("vram: restore waits ({why})"));
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        shrinks: u32,
        restores: u32,
        shrink_ok: bool,
        restore_ok: bool,
    }

    impl Fake {
        fn working() -> Self {
            Self { shrink_ok: true, restore_ok: true, ..Self::default() }
        }
    }

    impl Backing for Fake {
        fn shrink(&mut self) -> bool {
            self.shrinks += 1;
            self.shrink_ok
        }
        fn restore(&mut self) -> bool {
            self.restores += 1;
            self.restore_ok
        }
    }

    /// A hold that has drawn once, as every shrinkable one has.
    fn drawn() -> (Hold, Fake) {
        let mut hold = Hold::default();
        let mut fake = Fake::working();
        run(&mut hold, false, true, 0, &mut fake);
        (hold, fake)
    }

    /// One iteration at `t` ms.
    fn at(hold: &mut Hold, fake: &mut Fake, background: bool, t: u32) -> Step {
        run(hold, background, true, t, fake)
    }

    #[test]
    fn a_did_background_shrinks_once_and_the_foreground_restores_once() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x103, 1);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 0, "the WILL edge alone is no reason to give the buffers up");
        hold.note(0x104, 2);
        for _ in 0..3 {
            at(&mut hold, &mut fake, true, 2);
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 0));
        hold.note(0x105, 3);
        at(&mut hold, &mut fake, true, 3);
        assert_eq!(fake.restores, 0, "the WILL foreground edge does not authorize a frame");
        hold.note(0x106, 4);
        for _ in 0..3 {
            at(&mut hold, &mut fake, false, 4);
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 1));
    }

    #[test]
    fn a_return_before_the_loop_parks_never_shrinks() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104, 1);
        hold.note(0x106, 1);
        at(&mut hold, &mut fake, false, 1);
        assert_eq!((fake.shrinks, fake.restores), (0, 0));
    }

    #[test]
    fn repeated_toggles_pair_every_shrink_with_a_restore() {
        let (mut hold, mut fake) = drawn();
        for round in 1..=4 {
            hold.note(0x104, 1);
            at(&mut hold, &mut fake, true, 1);
            at(&mut hold, &mut fake, true, 1);
            hold.note(0x106, 1);
            at(&mut hold, &mut fake, false, 1);
            assert_eq!((fake.shrinks, fake.restores), (round, round));
        }
    }

    #[test]
    fn nothing_shrinks_in_front_of_the_viewer() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104, 1); // a stale edge while frames are presenting (a video plays in front)
        for _ in 0..3 {
            at(&mut hold, &mut fake, false, 1);
        }
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn a_parked_playback_shrinks_like_any_hidden_app() {
        // The video plane is the player's own; the UI window has nothing on it while hidden.
        let (mut hold, mut fake) = drawn();
        hold.note(0x104, 1);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 1);
    }

    #[test]
    fn an_app_that_never_drew_has_no_buffers_to_give_up() {
        let mut hold = Hold::default();
        let mut fake = Fake::working();
        hold.note(0x104, 1);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn the_kill_switch_keeps_the_buffers() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104, 1);
        run(&mut hold, true, false, 1, &mut fake);
        run(&mut hold, false, false, 60_000, &mut fake);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn a_refused_shrink_is_not_tried_again() {
        let (mut hold, mut fake) = drawn();
        fake.shrink_ok = false;
        hold.note(0x104, 1);
        for _ in 0..3 {
            at(&mut hold, &mut fake, true, 1);
        }
        hold.note(0x106, 2);
        at(&mut hold, &mut fake, false, 2);
        assert_eq!((fake.shrinks, fake.restores), (1, 0), "nothing was shrunk, so nothing is restored");
    }

    #[test]
    fn a_failed_restore_is_retried_then_given_up() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104, 1);
        at(&mut hold, &mut fake, true, 1);
        fake.restore_ok = false;
        hold.note(0x106, 2);
        for _ in 0..10 {
            at(&mut hold, &mut fake, false, 2);
        }
        assert_eq!(fake.restores, u32::from(RESTORE_TRIES));
        hold.note(0x104, 3);
        at(&mut hold, &mut fake, true, 3);
        assert_eq!(fake.shrinks, 1, "a window that would not restore is never shrunk again");
    }

    #[test]
    fn a_start_that_stays_silent_is_taken_for_hidden_and_shrunk_after_the_dwell() {
        let (mut hold, mut fake) = drawn();
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS - 1), Step::None);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS), Step::ShrinkQuiet);
        for t in 1..4 {
            assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + t), Step::None, "small, and nothing woke it");
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 0));
    }

    #[test]
    fn any_event_restarts_the_silence() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x200, 3_000);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 2_999), Step::None);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 3_000), Step::ShrinkQuiet);
    }

    #[test]
    fn a_lifecycle_event_means_the_app_was_shown_so_silence_is_not_hidden() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x106, 10);
        at(&mut hold, &mut fake, false, 60_000);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn a_hidden_start_is_restored_by_an_event_and_never_shrunk_that_way_again() {
        let (mut hold, mut fake) = drawn();
        at(&mut hold, &mut fake, false, QUIET_MS);
        hold.note(0x300, QUIET_MS + 10); // a key
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 10), Step::Restore);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 11), Step::None);
        assert_eq!(at(&mut hold, &mut fake, false, 10 * QUIET_MS), Step::None, "a shown app idles in front");
        assert_eq!((fake.shrinks, fake.restores), (1, 1));
    }

    #[test]
    fn a_relaunch_restores_a_hidden_start() {
        let (mut hold, mut fake) = drawn();
        hold.launches(0, QUIET_MS - 1);
        at(&mut hold, &mut fake, false, QUIET_MS);
        hold.launches(1, QUIET_MS + 1);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 1), Step::Restore);
    }

    #[test]
    fn a_hidden_start_that_is_then_backgrounded_and_returned_ends_full_size() {
        let (mut hold, mut fake) = drawn();
        at(&mut hold, &mut fake, false, QUIET_MS);
        hold.note(0x104, QUIET_MS + 1);
        assert_eq!(at(&mut hold, &mut fake, true, QUIET_MS + 1), Step::None, "already small");
        hold.note(0x106, QUIET_MS + 2);
        assert_eq!(at(&mut hold, &mut fake, false, QUIET_MS + 2), Step::Restore);
        assert_eq!((fake.shrinks, fake.restores), (1, 1));
    }

    #[test]
    fn a_refused_hidden_start_shrink_is_not_tried_again() {
        let (mut hold, mut fake) = drawn();
        fake.shrink_ok = false;
        for t in 0..5 {
            at(&mut hold, &mut fake, false, QUIET_MS + t);
        }
        assert_eq!(fake.shrinks, 1);
    }

    #[test]
    fn only_a_wl_egl_window_head_is_accepted() {
        let word = std::mem::size_of::<isize>();
        let mut mem = [0u8; 64];
        mem[..word].copy_from_slice(&3isize.to_ne_bytes());
        mem[word..word + 4].copy_from_slice(&1920i32.to_ne_bytes());
        mem[word + 4..word + 8].copy_from_slice(&1080i32.to_ne_bytes());
        // SAFETY: a live, aligned buffer larger than the head.
        let head = unsafe { Head::read(mem.as_ptr()) };
        assert_eq!(head, Head { version: 3, w: 1920, h: 1080 });
        assert!(head.plausible());
        assert!(!Head { version: 0x7f12_3456, ..head }.plausible(), "a proxy begins with a pointer");
        assert!(!Head { w: 0, ..head }.plausible());
        assert!(!Head { h: -1, ..head }.plausible());
    }
}
