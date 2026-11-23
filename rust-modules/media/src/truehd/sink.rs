//! LG's audio sink, met inside this process. libpf builds its pipeline itself, so a GObject
//! emission hook on GstBin's `element-added` finds the sink as a carrier Load adds it: LG's
//! `audiosink`, the one with a `pcm-multi-ch` property. A probe on its input then tells it about
//! the carrier the way the helper names ([`Carrier`](super::contract::Carrier)) and zeros every
//! buffer the route does not pass at that moment, as upstream's `lgroute.c` does.
//!
//! The layouts below are GStreamer 1.x's public structs, the same on every webOS release:
//! `GValue` (24 bytes), the head of `GstPadProbeInfo`, `GstMiniObject` and `GstEvent`.
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void, CStr, CString};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

use super::session;

#[repr(C)]
struct GValue {
    g_type: usize,
    _data: [u64; 2],
}

#[repr(C)]
struct ProbeInfo {
    kind: c_uint,
    _id: c_ulong,
    data: *mut c_void,
}

#[repr(C)]
struct MiniObject {
    _type: usize,
    _refcount: c_int,
    _lockstate: c_int,
    flags: c_uint,
    _copy: *mut c_void,
    _dispose: *mut c_void,
    _free: *mut c_void,
    _priv_uint: c_uint,
    _priv_pointer: *mut c_void,
}

#[repr(C)]
struct Event {
    _mini: MiniObject,
    kind: c_int,
}

type Hook = unsafe extern "C" fn(*mut c_void, c_uint, *const GValue, *mut c_void) -> c_int;
type ProbeFn = unsafe extern "C" fn(*mut c_void, *mut ProbeInfo, *mut c_void) -> c_int;
type Destroy = unsafe extern "C" fn(*mut c_void);

const G_TYPE_BOOLEAN: usize = 5 << 2;
const PROBE_BUFFER: c_uint = 1 << 4;
const PROBE_EVENT_DOWNSTREAM: c_uint = 1 << 6;
/// Flushes are not downstream events; a seek or a resume sends them.
const PROBE_EVENT_FLUSH: c_uint = 1 << 8;
const PROBE_DROP: c_int = 0;
const PROBE_OK: c_int = 1;
const PROBE_REMOVE: c_int = 2;
/// `GST_EVENT_MAKE_TYPE (50, DOWNSTREAM | SERIALIZED | STICKY)`.
const EVENT_CAPS: c_int = 12814;
/// `GST_EVENT_MAKE_TYPE (20, BOTH | SERIALIZED)`.
const EVENT_FLUSH_STOP: c_int = 5127;
/// LG's buffer flag for an elementary stream (`LGAO_BUFFER_FLAG_ES`).
const ES_FLAG: c_uint = 1 << 21;

plx_base::dynlib! {
    glib: ["libglib-2.0.so.0"] {
        fn g_free(p: *mut c_void);
    }
}

plx_base::dynlib! {
    gobject: ["libgobject-2.0.so.0"] {
        fn g_type_class_ref(t: usize) -> *mut c_void;
        fn g_signal_lookup(name: *const c_char, itype: usize) -> c_uint;
        fn g_signal_add_emission_hook(signal: c_uint, detail: c_uint, hook: Hook, data: *mut c_void, destroy: *mut c_void) -> c_ulong;
        fn g_value_get_object(v: *const GValue) -> *mut c_void;
        fn g_object_class_find_property(class: *mut c_void, name: *const c_char) -> *mut c_void;
        fn g_value_init(v: *mut GValue, t: usize) -> *mut GValue;
        fn g_value_set_boolean(v: *mut GValue, b: c_int);
        fn g_object_set_property(object: *mut c_void, name: *const c_char, v: *const GValue);
        fn g_value_unset(v: *mut GValue);
    }
}

plx_base::dynlib! {
    gst: ["libgstreamer-1.0.so.0"] {
        fn gst_bin_get_type() -> usize;
        fn gst_element_get_factory(element: *mut c_void) -> *mut c_void;
        fn gst_object_get_name(object: *mut c_void) -> *mut c_char;
        fn gst_object_unref(object: *mut c_void);
        fn gst_element_get_static_pad(element: *mut c_void, name: *const c_char) -> *mut c_void;
        fn gst_pad_add_probe(pad: *mut c_void, mask: c_uint, callback: ProbeFn, data: *mut c_void, destroy: Destroy) -> c_ulong;
        fn gst_event_parse_caps(event: *mut c_void, caps: *mut *mut c_void);
        fn gst_event_new_caps(caps: *mut c_void) -> *mut c_void;
        fn gst_caps_from_string(text: *const c_char) -> *mut c_void;
        fn gst_caps_to_string(caps: *const c_void) -> *mut c_char;
        fn gst_mini_object_unref(object: *mut c_void);
        fn gst_mini_object_make_writable(object: *mut c_void) -> *mut c_void;
        fn gst_buffer_get_size(buffer: *mut c_void) -> usize;
        fn gst_buffer_memset(buffer: *mut c_void, offset: usize, val: u8, size: usize) -> usize;
    }
}

/// GstBin is hooked before the first carrier Load builds its pipeline; once per process. False
/// when GStreamer is out of reach, and the carrier then plays without the probe.
static INSTALLED: LazyLock<bool> = LazyLock::new(|| {
    // SAFETY: GLib's own calls on a type and a signal both exist in GStreamer 1.x.
    let ok = unsafe { hook() };
    crate::player::log(if ok { "truehd: sink hook installed" } else { "truehd: sink hook unavailable" });
    ok
});

pub fn install() -> bool {
    *INSTALLED
}

unsafe fn hook() -> bool {
    if !(glib::load(None).ok() && gobject::load(None).ok() && gst::load(None).ok()) {
        return false;
    }
    let bin = gst_bin_get_type();
    // GstBin's signals exist once its class does; the reference is kept for the process.
    g_type_class_ref(bin);
    let signal = g_signal_lookup(c"element-added".as_ptr(), bin);
    signal != 0 && g_signal_add_emission_hook(signal, 0, element_added, null_mut(), null_mut()) != 0
}

/// The hook stays for the process (TRUE); it acts only for a carrier Load.
unsafe extern "C" fn element_added(_hint: *mut c_void, n: c_uint, params: *const GValue, _data: *mut c_void) -> c_int {
    if n >= 2 && super::carried() {
        let element = g_value_get_object(params.add(1));
        if !element.is_null() && is_lg_sink(element) {
            attach(element);
        }
    }
    1
}

unsafe fn is_lg_sink(element: *mut c_void) -> bool {
    let factory = gst_element_get_factory(element);
    if factory.is_null() {
        return false;
    }
    let name = gst_object_get_name(factory);
    if name.is_null() {
        return false;
    }
    let audiosink = CStr::from_ptr(name).to_bytes() == b"audiosink";
    g_free(name.cast());
    // G_OBJECT_GET_CLASS: an instance's first word is its class.
    audiosink && !g_object_class_find_property(*element.cast::<*mut c_void>(), c"pcm-multi-ch".as_ptr()).is_null()
}

unsafe fn attach(element: *mut c_void) {
    let pad = gst_element_get_static_pad(element, c"sink".as_ptr());
    if pad.is_null() {
        crate::player::log("truehd: LG's audio sink has no input pad");
        return;
    }
    let probe = Box::into_raw(Box::new(Probe {
        sink: element,
        first: AtomicBool::new(false),
        configured: AtomicBool::new(false),
        feeding: AtomicBool::new(false),
    }));
    let mask = PROBE_BUFFER | PROBE_EVENT_DOWNSTREAM | PROBE_EVENT_FLUSH;
    // A probe of this kind is always added; GStreamer frees it through `drop_probe`.
    gst_pad_add_probe(pad, mask, on_probe, probe.cast(), drop_probe);
    gst_object_unref(pad);
    crate::player::log("truehd: probing LG's audio sink");
}

/// One sink's state. `sink` is the pad's element, which outlives its probe.
struct Probe {
    sink: *mut c_void,
    /// The next buffer is the first since the carrier's caps.
    first: AtomicBool,
    /// The first buffer has passed: the start event announced the bitstream.
    configured: AtomicBool,
    /// `pcm-multi-ch` is back on for the data.
    feeding: AtomicBool,
}

unsafe extern "C" fn drop_probe(data: *mut c_void) {
    drop(Box::from_raw(data.cast::<Probe>()));
}

unsafe extern "C" fn on_probe(_pad: *mut c_void, info: *mut ProbeInfo, data: *mut c_void) -> c_int {
    let probe = &*data.cast::<Probe>();
    let info = &mut *info;
    if info.kind & PROBE_BUFFER != 0 {
        probe.buffer(info)
    } else {
        probe.event(info)
    }
}

impl Probe {
    unsafe fn event(&self, info: &mut ProbeInfo) -> c_int {
        match (*info.data.cast::<Event>()).kind {
            EVENT_FLUSH_STOP => {
                if let Some(shared) = session::active() {
                    shared.note_flush();
                }
            }
            EVENT_CAPS => {
                let Some(shared) = session::active() else {
                    crate::player::log("truehd: LG's audio sink left the carrier");
                    return PROBE_REMOVE;
                };
                if let Some(caps) = &shared.carrier.sink_caps {
                    rewrite_caps(info, caps);
                }
                if shared.carrier.start_as_bitstream && !self.configured.load(Ordering::Acquire) {
                    self.first.store(true, Ordering::Release);
                    set_multi_ch(self.sink, true);
                }
            }
            _ => {}
        }
        PROBE_OK
    }

    /// Upstream's order around the first buffer, then zeros unless the route passes it now. A
    /// buffer after the session ended is the carrier's tail and is zeroed too.
    unsafe fn buffer(&self, info: &mut ProbeInfo) -> c_int {
        let shared = session::active();
        let mut es = None;
        if shared.as_ref().is_some_and(|s| s.carrier.start_as_bitstream) {
            if self.first.swap(false, Ordering::AcqRel) {
                self.configured.store(true, Ordering::Release);
                set_multi_ch(self.sink, false);
                es = Some(true);
            } else if self.configured.load(Ordering::Acquire) {
                if !self.feeding.swap(true, Ordering::AcqRel) {
                    set_multi_ch(self.sink, true);
                }
                es = Some(false);
            }
        }
        let zero = !shared.is_some_and(|s| s.facts().safe(Instant::now(), s.epoch()));
        let flags = (*info.data.cast::<MiniObject>()).flags;
        let flip = es.is_some_and(|on| (flags & ES_FLAG != 0) != on);
        if !zero && !flip {
            return PROBE_OK;
        }
        let buffer = gst_mini_object_make_writable(info.data);
        if buffer.is_null() {
            return PROBE_DROP;
        }
        info.data = buffer;
        if flip {
            let head = &mut *buffer.cast::<MiniObject>();
            head.flags = if es == Some(true) { head.flags | ES_FLAG } else { head.flags & !ES_FLAG };
        }
        if zero {
            gst_buffer_memset(buffer, 0, 0, gst_buffer_get_size(buffer));
        }
        PROBE_OK
    }
}

/// Swap the caps event for one with the helper's caps.
unsafe fn rewrite_caps(info: &mut ProbeInfo, text: &str) {
    let Ok(text) = CString::new(text) else { return };
    let caps = gst_caps_from_string(text.as_ptr());
    if caps.is_null() {
        crate::player::log("truehd: the helper's sink caps do not parse");
        return;
    }
    let mut was = null_mut();
    gst_event_parse_caps(info.data, &mut was);
    let said = if was.is_null() { null_mut() } else { gst_caps_to_string(was) };
    let event = gst_event_new_caps(caps);
    gst_mini_object_unref(caps);
    if event.is_null() {
        return;
    }
    gst_mini_object_unref(info.data);
    info.data = event;
    let lg = if said.is_null() { "?".into() } else { CStr::from_ptr(said).to_string_lossy() };
    crate::player::log(&format!("truehd: LG's sink caps {lg} replaced"));
    if !said.is_null() {
        g_free(said.cast());
    }
}

unsafe fn set_multi_ch(sink: *mut c_void, on: bool) {
    let mut v = GValue { g_type: 0, _data: [0; 2] };
    g_value_init(&mut v, G_TYPE_BOOLEAN);
    g_value_set_boolean(&mut v, c_int::from(on));
    g_object_set_property(sink, c"pcm-multi-ch".as_ptr(), &v);
    g_value_unset(&mut v);
    crate::player::log(if on { "truehd: pcm-multi-ch on" } else { "truehd: pcm-multi-ch off" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glib_layouts_match_gstreamer_1() {
        assert_eq!(std::mem::size_of::<GValue>(), 24);
        let mini = std::mem::size_of::<MiniObject>();
        assert_eq!(mini, if cfg!(target_pointer_width = "64") { 64 } else { 36 });
        assert_eq!(std::mem::offset_of!(Event, kind), mini);
        assert_eq!(std::mem::offset_of!(ProbeInfo, data), 2 * std::mem::size_of::<usize>());
    }
}
