//! Optional private compositor subtitle geometry hint. All Wayland calls are on SDL's thread.
use std::os::raw::c_void;

type Bounds = (i32, i32, i32, i32, i32, i32);

#[derive(Default)]
struct BoundsCache(Option<Bounds>);

impl BoundsCache {
    fn changed(&mut self, canvas_width: i32, canvas_height: i32, rect: Option<(i32, i32, i32, i32)>) -> Option<Bounds> {
        let (x, y, width, height) = rect.unwrap_or((0, 0, 0, 0));
        let bounds = (canvas_width, canvas_height, x, y, width, height);
        if self.0 == Some(bounds) { return None; }
        self.0 = Some(bounds);
        Some(bounds)
    }
}

#[cfg(all(not(feature = "hostsim"), not(test)))]
mod wayland {
    use super::{BoundsCache, c_void};
    use std::ffi::{c_char, CStr};
    use std::os::raw::{c_int, c_uint};
    use std::ptr::{addr_of, addr_of_mut, null, null_mut};

    // These layouts and signatures match wl_interface and wl_message in wayland-util.h.
    #[repr(C)]
    struct Message { name: *const c_char, signature: *const c_char, types: *const *const Interface }
    #[repr(C)]
    struct Interface { name: *const c_char, version: c_int, method_count: c_int, methods: *const Message, event_count: c_int, events: *const Message }

    static mut REGION_METHODS: [Message; 2] = [
        Message { name: b"destroy\0".as_ptr().cast(), signature: b"\0".as_ptr().cast(), types: null() },
        Message { name: b"set_bounds\0".as_ptr().cast(), signature: b"iiiiii\0".as_ptr().cast(), types: null() },
    ];
    static mut REGION_INTERFACE: Interface = Interface {
        name: b"lumatv_subtitle_region_v1\0".as_ptr().cast(), version: 1, method_count: 2,
        methods: addr_of!(REGION_METHODS).cast(), event_count: 0, events: null(),
    };
    static mut GET_REGION_TYPES: [*const Interface; 2] = [addr_of!(REGION_INTERFACE), null()];
    static mut MANAGER_METHODS: [Message; 2] = [
        Message { name: b"destroy\0".as_ptr().cast(), signature: b"\0".as_ptr().cast(), types: null() },
        Message { name: b"get_region\0".as_ptr().cast(), signature: b"no\0".as_ptr().cast(), types: addr_of!(GET_REGION_TYPES).cast() },
    ];
    static mut MANAGER_INTERFACE: Interface = Interface {
        name: b"lumatv_subtitle_manager_v1\0".as_ptr().cast(), version: 1, method_count: 2,
        methods: addr_of!(MANAGER_METHODS).cast(), event_count: 0, events: null(),
    };

    type Constructor = unsafe extern "C" fn(*mut c_void, c_uint, *const c_void, ...) -> *mut c_void;
    type Bind = unsafe extern "C" fn(*mut c_void, c_uint, *const c_void, c_uint, ...) -> *mut c_void;
    type Destroy = unsafe extern "C" fn(*mut c_void);
    type Marshal = unsafe extern "C" fn(*mut c_void, c_uint, ...);
    struct Discovery { bind: Bind, manager: *mut c_void }
    #[repr(C)]
    struct Listener {
        global: unsafe extern "C" fn(*mut c_void, *mut c_void, c_uint, *const c_char, c_uint),
        remove: unsafe extern "C" fn(*mut c_void, *mut c_void, c_uint),
    }
    unsafe extern "C" fn global(data: *mut c_void, registry: *mut c_void, name: c_uint, interface: *const c_char, version: c_uint) {
        let state = &mut *data.cast::<Discovery>();
        if !state.manager.is_null() || interface.is_null() || version == 0 || CStr::from_ptr(interface).to_bytes() != b"lumatv_subtitle_manager_v1" { return; }
        state.manager = (state.bind)(registry, 0, addr_of!(MANAGER_INTERFACE).cast(), 1,
            name, b"lumatv_subtitle_manager_v1\0".as_ptr().cast::<c_char>(), 1u32, null_mut::<c_void>());
    }
    unsafe extern "C" fn remove(_: *mut c_void, _: *mut c_void, _: c_uint) {}
    static LISTENER: Listener = Listener { global, remove };

    struct Client { manager: *mut c_void, region: *mut c_void, destroy: Destroy, marshal: Marshal, cache: BoundsCache }
    static mut CLIENT: Option<Client> = None;

    pub(super) unsafe fn init(display: *mut c_void, surface: *mut c_void) {
        release();
        if display.is_null() || surface.is_null() { return; }
        let (Some(ctor), Some(bind), Some(add_listener), Some(roundtrip), Some(destroy), Some(marshal), Some(registry_iface), Some(surface_iface)) = (
            crate::system::wl_sym("wl_proxy_marshal_constructor"),
            crate::system::wl_sym("wl_proxy_marshal_constructor_versioned"),
            crate::system::wl_sym("wl_proxy_add_listener"),
            crate::system::wl_sym("wl_display_roundtrip"),
            crate::system::wl_sym("wl_proxy_destroy"),
            crate::system::wl_sym("wl_proxy_marshal"),
            crate::system::wl_sym("wl_registry_interface"),
            crate::system::wl_sym("wl_surface_interface"),
        ) else { return; };
        let ctor: Constructor = std::mem::transmute(ctor);
        let bind: Bind = std::mem::transmute(bind);
        let add_listener: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int = std::mem::transmute(add_listener);
        let roundtrip: unsafe extern "C" fn(*mut c_void) -> c_int = std::mem::transmute(roundtrip);
        let destroy: Destroy = std::mem::transmute(destroy);
        let marshal: Marshal = std::mem::transmute(marshal);
        (*addr_of_mut!(GET_REGION_TYPES))[1] = surface_iface.cast();
        let registry = ctor(display, 1, registry_iface, null_mut::<c_void>());
        if registry.is_null() { return; }
        let mut discovery = Discovery { bind, manager: null_mut() };
        let listening = add_listener(registry, addr_of!(LISTENER).cast(), (&mut discovery as *mut Discovery).cast()) == 0;
        let ok = listening && roundtrip(display) >= 0;
        destroy(registry);
        let manager = discovery.manager;
        if manager.is_null() { return; }
        if !ok {
            marshal(manager, 0);
            destroy(manager);
            return;
        }
        let region = ctor(manager, 1, addr_of!(REGION_INTERFACE).cast(), null_mut::<c_void>(), surface);
        if region.is_null() {
            marshal(manager, 0);
            destroy(manager);
            return;
        }
        CLIENT = Some(Client { manager, region, destroy, marshal, cache: BoundsCache::default() });
    }

    pub(super) unsafe fn release() {
        if let Some(client) = (*addr_of_mut!(CLIENT)).take() {
            (client.marshal)(client.region, 0);
            (client.destroy)(client.region);
            (client.marshal)(client.manager, 0);
            (client.destroy)(client.manager);
        }
    }

    pub(super) unsafe fn set(canvas_width: i32, canvas_height: i32, rect: Option<(i32, i32, i32, i32)>) {
        if let Some(client) = (*addr_of_mut!(CLIENT)).as_mut() {
            if let Some((cw, ch, x, y, w, h)) = client.cache.changed(canvas_width, canvas_height, rect) {
                (client.marshal)(client.region, 1, cw, ch, x, y, w, h);
            }
        }
    }
}

pub(super) fn init(display: *mut c_void, surface: *mut c_void) {
    #[cfg(all(not(feature = "hostsim"), not(test)))]
    unsafe { wayland::init(display, surface); }
    #[cfg(any(feature = "hostsim", test))]
    let _ = (display, surface);
}
pub(super) fn release() {
    #[cfg(all(not(feature = "hostsim"), not(test)))]
    unsafe { wayland::release(); }
}
pub(super) fn set(canvas_width: i32, canvas_height: i32, rect: Option<(i32, i32, i32, i32)>) {
    #[cfg(all(not(feature = "hostsim"), not(test)))]
    unsafe { wayland::set(canvas_width, canvas_height, rect); }
    #[cfg(any(feature = "hostsim", test))]
    let _ = (canvas_width, canvas_height, rect);
}

#[cfg(test)]
mod tests {
    use super::BoundsCache;
    #[test]
    fn sends_changes_including_empty_and_canvas_resize() {
        let mut cache = BoundsCache::default();
        assert_eq!(cache.changed(1920, 1080, None), Some((1920, 1080, 0, 0, 0, 0)));
        assert_eq!(cache.changed(1920, 1080, None), None);
        assert_eq!(cache.changed(1920, 1080, Some((2, 3, 40, 50))), Some((1920, 1080, 2, 3, 40, 50)));
        assert_eq!(cache.changed(1920, 1080, Some((2, 3, 40, 50))), None);
        assert_eq!(cache.changed(1280, 720, Some((2, 3, 40, 50))), Some((1280, 720, 2, 3, 40, 50)));
        assert_eq!(cache.changed(1280, 720, None), Some((1280, 720, 0, 0, 0, 0)));
    }
    #[test]
    fn new_surface_has_no_cached_bounds() {
        let mut old = BoundsCache::default();
        assert!(old.changed(100, 50, None).is_some());
        assert!(BoundsCache::default().changed(100, 50, None).is_some());
    }
}
