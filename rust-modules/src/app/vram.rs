//! **Video memory while hidden.** A Marquee that has drawn and then lost the screen keeps its
//! window's buffers (three, one screen each) and the driver's context; a freshly preloaded one
//! keeps only the context. Once the DID background edge (`0x104`) has arrived the window's
//! `wl_egl_window` is resized to a small surface and drawn and swapped three times, so the driver drops the
//! full-size buffers; the DID foreground edge (`0x106`) puts the full size back before the first
//! frame. The SDL window and the GL context are never destroyed (`gfx::egl::release_for_exit`
//! documents what a destroy races with), and the textures and strings are released by
//! [`super::park::release`] on the same edge.
//!
//! A launch the system started hidden (a keep-alive preload) never gets a lifecycle edge, so the
//! compositor's own word is read: `wl_webos_shell_surface.exposed` carries the visible rectangles
//! (none when covered or minimized, sent on change only) and `state_changed` carries fullscreen or
//! minimized. A window that has been told neither is hidden, shrunk after its first iteration, and
//! given its size back on the first non-empty `exposed` or fullscreen ([`shell`]).
//!
//! Only ever hidden: a window the compositor has not said is in front never presents full size,
//! and a frame iteration that may present never shrinks one that is. `/tmp/plxnative-keepvram`
//! (dev builds) keeps the buffers.
//!
//! [`Hold`] is the state machine and holds no handle, so the edges, fast toggles and the kill
//! switch are tested on the host; [`Window`] is the one place that touches Wayland.
use std::os::raw::c_void;

/// What an iteration owes the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    None,
    Shrink,
    Restore,
}

/// A failed restore is tried again on the next iterations, this many times in all: a window left
/// small is a black screen.
const RESTORE_TRIES: u8 = 5;

/// How long after the shell hook is installed a window that has been told nothing is believed
/// hidden. The first `exposed` of a window in front follows its first buffer, which is a few
/// frames after the hook; a window never in front is told nothing at all.
const GRACE_MS: u32 = 2_000;

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
    /// When the shell hook began to report; `None` where the compositor cannot be heard, and the
    /// lifecycle edges alone decide.
    hook_at: Option<u32>,
    /// The compositor's latest word: in front (`Some(true)`), covered or minimized
    /// (`Some(false)`), or nothing yet.
    front: Option<bool>,
}

impl Hold {
    /// An event reached the loop. DID edges drive the background shrink (the WILL edge may be
    /// followed by a return that never leaves the screen). A foreground edge or a key means the
    /// app is in front whatever the shell has said, so a compositor that never reports cannot
    /// leave a shown window small.
    pub(crate) fn note(&mut self, event: u32) {
        match event {
            0x104 => self.did_background = true,
            0x106 => self.did_background = false,
            _ => {}
        }
        if matches!(event, 0x105 | 0x106 | 0x300 | 0x301 | 0x400..=0x403) {
            self.front = Some(true);
        }
    }

    /// The shell hook is installed and reporting.
    pub(crate) fn hooked(&mut self, now: u32) {
        self.hook_at.get_or_insert(now);
    }

    /// The compositor said the window is in front (`true`) or not.
    pub(crate) fn shell(&mut self, front: bool) {
        self.front = Some(front);
    }

    /// Hidden by the compositor's word: covered, minimized, or never told anything once the hook
    /// has had [`GRACE_MS`] to hear the first `exposed`.
    fn hidden_by_shell(&self, now: u32) -> bool {
        match (self.hook_at, self.front) {
            (None, _) | (_, Some(true)) => false,
            (Some(_), Some(false)) => true,
            (Some(at), None) => now.wrapping_sub(at) >= GRACE_MS,
        }
    }

    /// This iteration's work. `background`: the loop is parked (the window may not present);
    /// `enabled`: the kill switch is not armed and the run is live (not a recording). Answer it
    /// with [`Hold::applied`].
    pub(crate) fn step(&mut self, background: bool, enabled: bool, now: u32) -> Step {
        let hidden = self.hidden_by_shell(now);
        if !background {
            self.drawn = true;
            self.did_background = false;
            if self.shrunk {
                return if hidden { Step::None } else { Step::Restore };
            }
            return if hidden && enabled && !self.off { Step::Shrink } else { Step::None };
        }
        if (self.did_background || hidden) && self.drawn && enabled && !self.off && !self.shrunk {
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
            Step::Shrink if done => {
                self.shrunk = true;
                self.restore_tries = 0;
            }
            Step::Shrink => self.off = true,
            Step::Restore if done => self.shrunk = false,
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
        Step::Shrink => backing.shrink(),
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
    if shell::hooked() {
        hold.hooked(now);
    }
    if let Some(front) = shell::take_change() {
        hold.shell(front);
    }
    let step = run(hold, background, live && !keep_buffers(), now, &mut Window(win));
    if step == Step::Restore {
        // A window shown after a hidden start has no other reason to draw its first frame.
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

#[cfg(feature = "hostsim")]
mod shell {
    use std::os::raw::c_void;

    pub(crate) fn watch(_win: *mut c_void) {}
    pub(super) fn hooked() -> bool {
        false
    }
    pub(super) fn take_change() -> Option<bool> {
        None
    }
}
#[cfg(not(feature = "hostsim"))]
use native as shell;
pub(crate) use shell::watch;

#[cfg(not(feature = "hostsim"))]
mod native {
    use super::Head;
    use plx_base::dynlib::Handle;
    use plx_base::eventlog::log;
    use std::cell::Cell;
    use std::os::raw::{c_int, c_void};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

    extern "C" {
        fn SDL_GetWindowWMInfo(window: *mut c_void, info: *mut c_void) -> c_int;
        fn SDL_GetVersion(ver: *mut u8);
        fn glBindFramebuffer(target: u32, framebuffer: u32);
        fn glClearColor(r: f32, g: f32, b: f32, a: f32);
        fn glClear(mask: u32);
    }

    /// `SDL_SYSWM_WAYLAND`, and `egl_window`'s place among the pointers of the union's `wl`
    /// member (`display`, `surface`, `shell_surface`, `egl_window`), which begins at byte 8.
    const SYSWM_WAYLAND: i32 = 6;
    const EGL_WINDOW_SLOT: usize = 3;
    /// The size the surface is shrunk to: small, but a size every driver allocates.
    const SMALL: c_int = 64;
    /// The driver keeps three buffers and reallocates one per drawn swap at the new size.
    const SWAPS: u32 = 3;
    const GL_FRAMEBUFFER: u32 = 0x8D40;
    const GL_COLOR_BUFFER_BIT: u32 = 0x4000;

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

    /// One pointer of `SDL_SysWMinfo`'s Wayland member (the union begins at byte 8).
    unsafe fn wm_slot(win: *mut c_void, slot: usize) -> Result<*mut c_void, &'static str> {
        let mut info = [0u8; 512]; // the television's SDL writes more than the header declares
        SDL_GetVersion(info.as_mut_ptr());
        if SDL_GetWindowWMInfo(win, info.as_mut_ptr().cast()) == 0 {
            return Err("SDL_GetWindowWMInfo failed");
        }
        if i32::from_ne_bytes(info[4..8].try_into().unwrap()) != SYSWM_WAYLAND {
            return Err("not a Wayland window");
        }
        let p = (info.as_ptr().add(8) as *const *mut c_void).add(slot).read_unaligned();
        if p.is_null() || p as usize % std::mem::align_of::<isize>() != 0 {
            return Err("empty slot in SDL_SysWMinfo");
        }
        Ok(p)
    }

    /// The window's `wl_egl_window` and its head, or why not.
    unsafe fn egl_window(win: *mut c_void) -> Result<(*mut c_void, Head), &'static str> {
        let p = wm_slot(win, EGL_WINDOW_SLOT)?;
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
            // A swap alone re-posts the buffer the last frame drew into, at its old size: the driver
            // takes a buffer of the window's new size only when a frame draws, and the compositor
            // kept the full-size window after a hidden shrink that swapped without drawing
            // (2026-11-25, `ctl windows` 5.8 s after Home). Each swap follows a clear of the
            // window's own framebuffer.
            for _ in 0..SWAPS {
                glBindFramebuffer(GL_FRAMEBUFFER, plx_base::surface::default_fb());
                glClearColor(0.0, 0.0, 0.0, 0.0);
                glClear(GL_COLOR_BUFFER_BIT);
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

    // ---- the compositor's word: wl_webos_shell_surface ------------------------------------------

    /// `SDL_SysWMinfo`'s `shell_surface` slot.
    const SHELL_SURFACE_SLOT: usize = 2;
    /// `wl_webos_shell_surface`'s events in protocol order: state_changed, position_changed, close,
    /// exposed, state_about_to_change, addon_status_changed.
    const EVENTS: usize = 6;
    const EV_STATE: usize = 0;
    const EV_EXPOSED: usize = 3;
    /// `wl_webos_shell_surface.state`.
    const STATE_MINIMIZED: u32 = 1;
    const STATE_FULLSCREEN: u32 = 3;

    static HOOKED: AtomicBool = AtomicBool::new(false);
    /// The latest word, and a count of words so the loop sees each change once.
    static FRONT: AtomicBool = AtomicBool::new(false);
    static SEQ: AtomicU32 = AtomicU32::new(0);
    static SEEN: AtomicU32 = AtomicU32::new(0);
    /// SDL's own handlers, called after ours, and the table the proxy is given.
    static ORIG: [AtomicUsize; EVENTS] = [const { AtomicUsize::new(0) }; EVENTS];
    static TABLE: [AtomicUsize; EVENTS] = [const { AtomicUsize::new(0) }; EVENTS];

    /// `struct wl_array`.
    #[repr(C)]
    struct WlArray {
        size: usize,
        alloc: usize,
        data: *const i32,
    }

    pub(crate) fn hooked() -> bool {
        HOOKED.load(Ordering::Acquire)
    }

    /// The compositor's word since the last call, if it changed.
    pub(crate) fn take_change() -> Option<bool> {
        let seq = SEQ.load(Ordering::Acquire);
        (SEEN.swap(seq, Ordering::Relaxed) != seq).then(|| FRONT.load(Ordering::Acquire))
    }

    fn report(front: bool, what: &str) {
        FRONT.store(front, Ordering::Release);
        SEQ.fetch_add(1, Ordering::Release);
        log(&format!("vram: shell {what}"));
    }

    unsafe extern "C" fn on_state(data: *mut c_void, proxy: *mut c_void, state: u32) {
        match state {
            STATE_FULLSCREEN => report(true, "state fullscreen"),
            STATE_MINIMIZED => report(false, "state minimized"),
            _ => {}
        }
        type F = unsafe extern "C" fn(*mut c_void, *mut c_void, u32);
        let orig = ORIG[EV_STATE].load(Ordering::Relaxed);
        if orig != 0 {
            std::mem::transmute::<usize, F>(orig)(data, proxy, state);
        }
    }

    unsafe extern "C" fn on_exposed(data: *mut c_void, proxy: *mut c_void, rects: *const WlArray) {
        // `x,y,w,h,...,-1`: an empty list is the terminator alone.
        let visible = !rects.is_null() && {
            let a = &*rects;
            a.size >= 4 * std::mem::size_of::<i32>() && !a.data.is_null() && a.data.read_unaligned() != -1
        };
        report(visible, if visible { "exposed" } else { "exposed nothing" });
        type F = unsafe extern "C" fn(*mut c_void, *mut c_void, *const WlArray);
        let orig = ORIG[EV_EXPOSED].load(Ordering::Relaxed);
        if orig != 0 {
            std::mem::transmute::<usize, F>(orig)(data, proxy, rects);
        }
    }

    fn client_sym(name: &str) -> Option<*mut c_void> {
        Handle::self_handle()
            .sym(name)
            .or_else(|| Handle::open_loaded(&["libwayland-client.so.0", "libwayland-client.so"]).and_then(|(h, _)| h.sym(name)))
            .filter(|p| !p.is_null())
    }

    /// The window's `wl_webos_shell_surface`. `SDL_SysWMinfo`'s `shell_surface` is the plain
    /// `wl_shell_surface`; LG's SDL 2.0.14 keeps the webOS one in its `SDL_WindowData` at byte 56,
    /// which `SDL_Window.driverdata` (byte 144 of the window) points to (read from the build's
    /// `Wayland_MinimizeWindow` and `Wayland_CreateWindow`, 2026-11-25). Nothing is read through a
    /// pointer until the build is that SDL and the structure shows SDL's own handles in the
    /// places SDL_WindowData puts them: the window, the `wl_surface`, the `wl_shell_surface` and
    /// the `wl_egl_window`.
    unsafe fn shell_surface(win: *mut c_void) -> Result<*mut c_void, &'static str> {
        const WINDOW_DRIVERDATA: usize = 144;
        const DATA_SURFACE: usize = 2; // words: sdlwindow, waylandData, surface, shell_surface, egl_window
        const DATA_SHELL: usize = 3;
        const DATA_EGL: usize = 4;
        const DATA_WEBOS_SHELL: usize = 14; // byte 56
        let mut ver = [0u8; 3];
        SDL_GetVersion(ver.as_mut_ptr());
        if ver != [2, 0, 14] {
            return Err("not the SDL 2.0.14 this layout was read from");
        }
        let surface = wm_slot(win, 1)?;
        let shell = wm_slot(win, SHELL_SURFACE_SLOT)?;
        let egl = wm_slot(win, EGL_WINDOW_SLOT)?;
        let data = (win as *const u8).add(WINDOW_DRIVERDATA).cast::<*const usize>().read_unaligned();
        if data.is_null() || data as usize % std::mem::align_of::<usize>() != 0 {
            return Err("no SDL_WindowData");
        }
        let word = |i: usize| data.add(i).read();
        if word(0) != win as usize
            || word(DATA_SURFACE) != surface as usize
            || word(DATA_SHELL) != shell as usize
            || word(DATA_EGL) != egl as usize
        {
            return Err("SDL_WindowData is not laid out as read");
        }
        let proxy = word(DATA_WEBOS_SHELL) as *mut c_void;
        if proxy.is_null() || proxy as usize % std::mem::align_of::<usize>() != 0 {
            return Err("the window has no wl_webos_shell_surface");
        }
        Ok(proxy)
    }

    /// Listen for the compositor's `exposed` and `state_changed` on the window's shell surface,
    /// before its first buffer. SDL's own handlers keep running: when it has installed a table the
    /// proxy is given a copy with ours in front. Refused unless every step checks out, and then
    /// nothing is hidden by the shell's word (the lifecycle edges alone decide).
    pub(crate) fn watch(win: *mut c_void) {
        // SAFETY: the main thread right after the window exists, before events are pumped; every
        // pointer is checked against the library's own answers before a write.
        unsafe {
            if hooked() {
                return; // wrapping our own table would make it call itself
            }
            let proxy = match shell_surface(win) {
                Ok(p) => p,
                Err(why) => return log(&format!("vram: shell hook off ({why})")),
            };
            let (Some(class), Some(add)) = (client_sym("wl_proxy_get_class"), client_sym("wl_proxy_add_listener")) else {
                return log("vram: shell hook off (no libwayland-client symbols)");
            };
            let class: unsafe extern "C" fn(*mut c_void) -> *const std::os::raw::c_char = std::mem::transmute(class);
            let add: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int = std::mem::transmute(add);
            let name = std::ffi::CStr::from_ptr(class(proxy)).to_string_lossy().into_owned();
            if name != "wl_webos_shell_surface" {
                return log(&format!("vram: shell hook off (the slot holds {name})"));
            }
            let get: Option<unsafe extern "C" fn(*mut c_void) -> *const c_void> =
                client_sym("wl_proxy_get_listener").map(|p| std::mem::transmute(p));
            let table = TABLE.as_ptr() as *const c_void;
            // `wl_object` is `{ interface, implementation, id }`, the proxy's first field.
            let slot = (proxy as *mut *const c_void).add(1);
            let existing = get.map_or_else(|| slot.read(), |g| g(proxy));
            if existing.is_null() {
                TABLE[EV_STATE].store(on_state as *const () as usize, Ordering::Relaxed);
                TABLE[EV_EXPOSED].store(on_exposed as *const () as usize, Ordering::Relaxed);
                if add(proxy, table, std::ptr::null_mut()) != 0 {
                    return log("vram: shell hook off (the proxy refused a listener)");
                }
                HOOKED.store(true, Ordering::Release);
                return log("vram: shell hook on (own listener)");
            }
            // SDL listens already: wrap its table.
            if slot.read() != existing {
                return log("vram: shell hook off (proxy layout is not the one assumed)");
            }
            for i in 0..EVENTS {
                let orig = (existing as *const usize).add(i).read();
                ORIG[i].store(orig, Ordering::Relaxed);
                TABLE[i].store(orig, Ordering::Relaxed);
            }
            TABLE[EV_STATE].store(on_state as *const () as usize, Ordering::Release);
            TABLE[EV_EXPOSED].store(on_exposed as *const () as usize, Ordering::Release);
            slot.write(table);
            if get.is_some_and(|g| g(proxy) != table) {
                slot.write(existing);
                return log("vram: shell hook off (the swap did not take)");
            }
            HOOKED.store(true, Ordering::Release);
            log("vram: shell hook on (wrapping SDL's listener)");
        }
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
        hold.note(0x103);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 0, "the WILL edge alone is no reason to give the buffers up");
        hold.note(0x104);
        for _ in 0..3 {
            at(&mut hold, &mut fake, true, 2);
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 0));
        hold.note(0x105);
        at(&mut hold, &mut fake, true, 3);
        assert_eq!(fake.restores, 0, "the WILL foreground edge does not authorize a frame");
        hold.note(0x106);
        for _ in 0..3 {
            at(&mut hold, &mut fake, false, 4);
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 1));
    }

    #[test]
    fn a_return_before_the_loop_parks_never_shrinks() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104);
        hold.note(0x106);
        at(&mut hold, &mut fake, false, 1);
        assert_eq!((fake.shrinks, fake.restores), (0, 0));
    }

    #[test]
    fn repeated_toggles_pair_every_shrink_with_a_restore() {
        let (mut hold, mut fake) = drawn();
        for round in 1..=4 {
            hold.note(0x104);
            at(&mut hold, &mut fake, true, 1);
            at(&mut hold, &mut fake, true, 1);
            hold.note(0x106);
            at(&mut hold, &mut fake, false, 1);
            assert_eq!((fake.shrinks, fake.restores), (round, round));
        }
    }

    #[test]
    fn nothing_shrinks_in_front_of_the_viewer() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104); // a stale edge while frames are presenting (a video plays in front)
        for _ in 0..3 {
            at(&mut hold, &mut fake, false, 1);
        }
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn a_parked_playback_shrinks_like_any_hidden_app() {
        // The video plane is the player's own; the UI window has nothing on it while hidden.
        let (mut hold, mut fake) = drawn();
        hold.note(0x104);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 1);
    }

    #[test]
    fn an_app_that_never_drew_has_no_buffers_to_give_up() {
        let mut hold = Hold::default();
        let mut fake = Fake::working();
        hold.note(0x104);
        at(&mut hold, &mut fake, true, 1);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn the_kill_switch_keeps_the_buffers() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104);
        run(&mut hold, true, false, 1, &mut fake);
        run(&mut hold, false, false, 60_000, &mut fake);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn a_refused_shrink_is_not_tried_again() {
        let (mut hold, mut fake) = drawn();
        fake.shrink_ok = false;
        hold.note(0x104);
        for _ in 0..3 {
            at(&mut hold, &mut fake, true, 1);
        }
        hold.note(0x106);
        at(&mut hold, &mut fake, false, 2);
        assert_eq!((fake.shrinks, fake.restores), (1, 0), "nothing was shrunk, so nothing is restored");
    }

    #[test]
    fn a_failed_restore_is_retried_then_given_up() {
        let (mut hold, mut fake) = drawn();
        hold.note(0x104);
        at(&mut hold, &mut fake, true, 1);
        fake.restore_ok = false;
        hold.note(0x106);
        for _ in 0..10 {
            at(&mut hold, &mut fake, false, 2);
        }
        assert_eq!(fake.restores, u32::from(RESTORE_TRIES));
        hold.note(0x104);
        at(&mut hold, &mut fake, true, 3);
        assert_eq!(fake.shrinks, 1, "a window that would not restore is never shrunk again");
    }

    const G: u32 = GRACE_MS;

    /// A hold whose shell hook began to report at 0 and that has drawn once.
    fn hooked() -> (Hold, Fake) {
        let (mut hold, fake) = drawn();
        hold.hooked(0);
        (hold, fake)
    }

    #[test]
    fn a_window_the_compositor_never_mentions_is_hidden_after_the_grace_and_shrunk_once() {
        let (mut hold, mut fake) = hooked();
        assert_eq!(at(&mut hold, &mut fake, false, G - 1), Step::None, "its first exposed may still come");
        assert_eq!(at(&mut hold, &mut fake, false, G), Step::Shrink);
        for t in 1..4 {
            assert_eq!(at(&mut hold, &mut fake, false, G + t), Step::None);
        }
        assert_eq!((fake.shrinks, fake.restores), (1, 0));
    }

    #[test]
    fn a_window_that_is_told_it_is_in_front_is_never_shrunk() {
        let (mut hold, mut fake) = hooked();
        hold.shell(true);
        at(&mut hold, &mut fake, false, 60_000);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn without_a_hook_silence_means_nothing() {
        let (mut hold, mut fake) = drawn();
        at(&mut hold, &mut fake, false, 600_000);
        assert_eq!(fake.shrinks, 0);
    }

    #[test]
    fn the_first_exposure_or_fullscreen_restores_a_hidden_start() {
        let (mut hold, mut fake) = hooked();
        at(&mut hold, &mut fake, false, G);
        hold.shell(true);
        assert_eq!(at(&mut hold, &mut fake, false, G + 1), Step::Restore);
        assert_eq!(at(&mut hold, &mut fake, false, G + 2), Step::None);
        assert_eq!((fake.shrinks, fake.restores), (1, 1));
    }

    #[test]
    fn covered_shrinks_at_once_and_front_restores_every_time() {
        let (mut hold, mut fake) = hooked();
        hold.shell(true);
        for round in 1..=3 {
            hold.shell(false);
            at(&mut hold, &mut fake, false, 100);
            at(&mut hold, &mut fake, false, 101);
            hold.shell(true);
            at(&mut hold, &mut fake, false, 102);
            assert_eq!((fake.shrinks, fake.restores), (round, round));
        }
    }

    #[test]
    fn a_hidden_report_shrinks_a_parked_loop_too_and_the_foreground_edge_overrides_it() {
        let (mut hold, mut fake) = hooked();
        hold.shell(false);
        assert_eq!(at(&mut hold, &mut fake, true, 5), Step::Shrink);
        hold.note(0x106); // the edge says shown before the compositor's word arrives
        assert_eq!(at(&mut hold, &mut fake, false, 6), Step::Restore);
    }

    #[test]
    fn a_key_proves_the_window_is_in_front_when_the_compositor_never_reported() {
        let (mut hold, mut fake) = hooked();
        at(&mut hold, &mut fake, false, G);
        hold.note(0x300);
        assert_eq!(at(&mut hold, &mut fake, false, G + 1), Step::Restore);
    }

    #[test]
    fn a_hidden_start_that_is_backgrounded_after_it_was_shown_ends_full_size() {
        let (mut hold, mut fake) = hooked();
        at(&mut hold, &mut fake, false, G);
        hold.shell(true);
        at(&mut hold, &mut fake, false, G + 1);
        hold.note(0x104);
        at(&mut hold, &mut fake, true, G + 2);
        hold.note(0x106);
        at(&mut hold, &mut fake, false, G + 3);
        assert_eq!((fake.shrinks, fake.restores), (2, 2));
    }

    #[test]
    fn the_kill_switch_and_a_refusal_also_hold_for_a_hidden_start() {
        let (mut hold, mut fake) = hooked();
        run(&mut hold, false, false, G, &mut fake);
        assert_eq!(fake.shrinks, 0);
        fake.shrink_ok = false;
        for t in 0..5 {
            at(&mut hold, &mut fake, false, G + t);
        }
        assert_eq!(fake.shrinks, 1, "a refused shrink is not tried again");
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
