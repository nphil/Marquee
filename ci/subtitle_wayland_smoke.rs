//! Standalone transport smoke: rustc this file with subtitle_wayland_smoke.c and libwayland.
use std::ffi::{c_char, c_void, CString};

mod system {
    use super::{c_char, c_void, CString};
    extern "C" { fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void; }
    pub(crate) fn wl_sym(name: &str) -> Option<*mut c_void> {
        let name = CString::new(name).unwrap();
        let symbol = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        (!symbol.is_null()).then_some(symbol)
    }
}
#[path = "../rust-modules/src/subtitle_wayland.rs"]
mod subtitle_wayland;
extern "C" {
    fn smoke_start(advertise: i32) -> *mut c_void;
    fn smoke_surface() -> *mut c_void;
    fn smoke_sync(display: *mut c_void);
    fn smoke_count(which: i32) -> i32;
    fn smoke_stop(display: *mut c_void);
}
fn count(which: i32) -> i32 { unsafe { smoke_count(which) } }
fn sync(display: *mut c_void) { unsafe { smoke_sync(display) } }
fn main() {
    unsafe {
        let display = smoke_start(1);
        let surface = smoke_surface();
        subtitle_wayland::init(display, surface);
        subtitle_wayland::set(1920, 1080, Some((120, 800, 500, 90)));
        subtitle_wayland::set(1920, 1080, Some((120, 800, 500, 90)));
        sync(display);
        assert_eq!(count(0), 1);
        assert_eq!((5..11).map(count).collect::<Vec<_>>(), [1920, 1080, 120, 800, 500, 90]);
        subtitle_wayland::set(1920, 1080, None);
        sync(display);
        assert_eq!(count(0), 2);
        assert_eq!((5..11).map(count).collect::<Vec<_>>(), [1920, 1080, 0, 0, 0, 0]);
        subtitle_wayland::release();
        subtitle_wayland::init(display, surface);
        subtitle_wayland::set(1920, 1080, None);
        sync(display);
        assert_eq!((count(0), count(1), count(2), count(3), count(4)), (3, 2, 1, 1, 0));
        subtitle_wayland::release();
        sync(display);
        assert_eq!((count(2), count(3), count(4)), (2, 2, 0));
        smoke_stop(display);

        let display = smoke_start(0);
        subtitle_wayland::init(display, smoke_surface());
        subtitle_wayland::set(1920, 1080, None);
        subtitle_wayland::release();
        sync(display);
        assert_eq!((count(0), count(1), count(2), count(3), count(4)), (0, 0, 0, 0, 0));
        smoke_stop(display);
    }
    println!("subtitle Wayland transport smoke passed");
}
