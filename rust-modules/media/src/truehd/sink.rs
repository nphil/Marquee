//! LG's audio sink, met inside this process. libpf builds its pipeline itself, so a GObject
//! emission hook on GstBin's `element-added` finds the sink as a carrier Load adds it: LG's
//! `audiosink`, the one with a `pcm-multi-ch` property. A probe on its input then tells it about
//! the carrier the way the helper names ([`Carrier`](super::contract::Carrier)) and zeros every
//! buffer the route does not pass at that moment, as upstream's `lgroute.c` does.
//!
//! The layouts below are GStreamer 1.x's public structs, the same on every webOS release:
//! `GValue` (24 bytes), the heads of `GParamSpec` and `GstPadProbeInfo`, `GstMiniObject` and
//! `GstEvent`.
//!
//! libpf gives a PCM Load a mixer channel (the sink's `index` 4 with `mixer` on), which the helper
//! cannot route, so a carrier Load puts the sink on [`CARRIER_ADEC`] off the mixer as it is added,
//! before it opens. Its `index` property ("Index of audio resource") and `mixer` property ("Set
//! using mixer") then read back the input it holds ([`Granted`]); only that input is armed.
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void, CStr, CString};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Once, OnceLock};
use std::time::{Duration, Instant};

use super::session::{self, Granted};

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

/// The head of `GParamSpec`.
#[repr(C)]
struct ParamSpec {
    _instance: *mut c_void,
    _name: *const c_char,
    flags: c_uint,
    value_type: usize,
}

type Hook = unsafe extern "C" fn(*mut c_void, c_uint, *const GValue, *mut c_void) -> c_int;
type InterfaceCheck = unsafe extern "C" fn(*mut c_void, *mut c_void);
type ProbeFn = unsafe extern "C" fn(*mut c_void, *mut ProbeInfo, *mut c_void) -> c_int;
type Destroy = unsafe extern "C" fn(*mut c_void);

const G_TYPE_BOOLEAN: usize = 5 << 2;
const G_TYPE_INT: usize = 6 << 2;
const G_TYPE_UINT: usize = 7 << 2;
const PARAM_WRITABLE: c_uint = 1 << 1;
const PARAM_CONSTRUCT_ONLY: c_uint = 1 << 3;
/// The decoder input a carrier Load's sink is put on: ADEC1. The proven run (`appsrc format=time
/// block=true ! audiosink index=1`, 2026-11-22: the receiver showed Dolby Atmos) played from it,
/// and LG's own AC3 Loads use it too. An HDMI input keeps its decoder: the helper arms only while
/// no decoder input is connected and this one is not running.
const CARRIER_ADEC: u8 = 1;
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
        fn g_type_from_name(name: *const c_char) -> usize;
        fn g_type_name(t: usize) -> *const c_char;
        fn g_type_class_peek(t: usize) -> *mut c_void;
        fn g_type_add_interface_check(data: *mut c_void, check: InterfaceCheck);
        fn g_signal_lookup(name: *const c_char, itype: usize) -> c_uint;
        fn g_signal_add_emission_hook(signal: c_uint, detail: c_uint, hook: Hook, data: *mut c_void, destroy: *mut c_void) -> c_ulong;
        fn g_value_get_object(v: *const GValue) -> *mut c_void;
        fn g_object_class_find_property(class: *mut c_void, name: *const c_char) -> *mut c_void;
        fn g_value_init(v: *mut GValue, t: usize) -> *mut GValue;
        fn g_value_set_boolean(v: *mut GValue, b: c_int);
        fn g_value_set_int(v: *mut GValue, i: c_int);
        fn g_value_get_boolean(v: *const GValue) -> c_int;
        fn g_value_get_int(v: *const GValue) -> c_int;
        fn g_object_set_property(object: *mut c_void, name: *const c_char, v: *const GValue);
        fn g_object_get_property(object: *mut c_void, name: *const c_char, v: *mut GValue);
        fn g_value_unset(v: *mut GValue);
    }
}

plx_base::dynlib! {
    gst: ["libgstreamer-1.0.so.0"] {
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

/// libpf's GLib and GStreamer, bound once per process; this never maps a library itself.
static BOUND: LazyLock<bool> = LazyLock::new(|| {
    let mapped = [glib::CANDIDATES, gobject::CANDIDATES, gst::CANDIDATES]
        .into_iter()
        .all(|names| plx_base::dynlib::Handle::open_loaded(names).is_some());
    let ok = mapped && glib::load(None).ok() && gobject::load(None).ok() && gst::load(None).ok();
    if !ok {
        crate::player::log("truehd: sink hook unavailable: GStreamer is not in the process");
    }
    ok
});
/// Whether GstBin's `element-added` is hooked; unset until GStreamer has built GstBin's class.
static HOOKED: OnceLock<bool> = OnceLock::new();
static WATCH: Once = Once::new();

/// Whether the sink hook is in, or goes in as libpf starts GStreamer inside the coming Load.
/// False when it cannot, and the Load plays the track the carrier displaced.
///
/// GstBin's class is never built here: built before `gst_init`, its element metadata is stored
/// under GStreamer's quarks while they are still zero, and `gst_init` then aborts registering
/// "bin" (`gst_register_core_elements`, gst.c:617 on the TV). GStreamer 1.24 and older store it
/// that way; 1.26 keys it by string.
pub fn install() -> bool {
    if !*BOUND {
        return false;
    }
    if let Some(&hooked) = HOOKED.get() {
        return hooked;
    }
    // SAFETY: GLib's calls on the libraries libpf mapped; none of them builds a class.
    unsafe {
        let bin = g_type_from_name(c"GstBin".as_ptr());
        if bin != 0 && !g_type_class_peek(bin).is_null() {
            return add_hook(bin);
        }
        // GLib walks its check list unlocked while it builds a class, so the check is added
        // only while GStreamer, which libpf starts inside a Load, builds none.
        WATCH.call_once(|| g_type_add_interface_check(null_mut(), on_interface));
    }
    crate::player::log("truehd: sink hook goes in as GStreamer starts");
    true
}

/// Called as GLib finishes each interface of a class it builds. GStreamer builds GstBin inside
/// `gst_init`, after its `class_init` made the signal and before any bin exists.
unsafe extern "C" fn on_interface(_data: *mut c_void, iface: *mut c_void) {
    if HOOKED.get().is_some() {
        return;
    }
    // GTypeInterface: the interface's type, then the type whose class is being built.
    let owner = *iface.cast::<usize>().add(1);
    let name = g_type_name(owner);
    if !name.is_null() && CStr::from_ptr(name) == c"GstBin" {
        add_hook(owner);
    }
}

unsafe fn add_hook(bin: usize) -> bool {
    *HOOKED.get_or_init(|| {
        let signal = g_signal_lookup(c"element-added".as_ptr(), bin);
        let ok = signal != 0 && g_signal_add_emission_hook(signal, 0, element_added, null_mut(), null_mut()) != 0;
        crate::player::log(if ok { "truehd: sink hook installed" } else { "truehd: sink hook unavailable" });
        ok
    })
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
    let given = give_input(element);
    let probe = Box::into_raw(Box::new(Probe {
        sink: element,
        given,
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

/// Whether a carrier Load's sink goes on [`CARRIER_ADEC`] off the mixer: only while the helper
/// still admits this process, which it does only while no decoder input is connected (no HDMI
/// input or Live TV in front), and only when `index` is an integer and `mixer` a boolean that can
/// both be set after construction. Each property is its value type and flags.
fn gives_input(admitted: bool, index: Option<(usize, c_uint)>, mixer: Option<(usize, c_uint)>) -> bool {
    let settable = |spec: Option<(usize, c_uint)>, types: &[usize]| {
        spec.is_some_and(|(t, flags)| types.contains(&t) && flags & PARAM_WRITABLE != 0 && flags & PARAM_CONSTRUCT_ONLY == 0)
    };
    admitted && settable(index, &[G_TYPE_INT, G_TYPE_UINT]) && settable(mixer, &[G_TYPE_BOOLEAN])
}

/// The decoder input the sink's `index` and `mixer` read back as. A sink put on [`CARRIER_ADEC`]
/// arms only while it still holds that input off the mixer: LG moving it anywhere else is a
/// refusal, never an arm on another input.
fn read_back(given: bool, index: Option<i32>, mixer: Option<bool>) -> Granted {
    if !given {
        Granted::from_sink(index, mixer)
    } else if index == Some(i32::from(CARRIER_ADEC)) && mixer == Some(false) {
        Granted::Decoder(CARRIER_ADEC)
    } else {
        Granted::Refused("LG's audio sink did not keep the decoder input it was given")
    }
}

/// Puts the sink on [`CARRIER_ADEC`] off the mixer as libpf adds it, as the proven run's
/// `audiosink index=1` was, when [`gives_input`] allows. Whether it did.
unsafe fn give_input(sink: *mut c_void) -> bool {
    let spec = |name: &CStr| {
        // G_OBJECT_GET_CLASS: an instance's first word is its class.
        let p = g_object_class_find_property(*sink.cast::<*mut c_void>(), name.as_ptr()).cast::<ParamSpec>();
        (!p.is_null()).then(|| ((*p).value_type, (*p).flags))
    };
    let admitted = super::watch::topic().is_some_and(|t| t.admits(std::process::id()));
    if !gives_input(admitted, spec(c"index"), spec(c"mixer")) {
        crate::player::log(&format!("truehd: LG's audio sink keeps LG's input (the helper admits this process: {admitted})"));
        return false;
    }
    set_int(sink, c"index", c_int::from(CARRIER_ADEC));
    set_boolean(sink, c"mixer", false);
    let (index, mixer) = (read_int(sink, c"index"), read_boolean(sink, c"mixer"));
    crate::player::log(&format!(
        "truehd: LG's audio sink put on ADEC{CARRIER_ADEC} off the mixer; it reads index {index:?}, mixer {mixer:?}"
    ));
    true
}

/// One sink's state. `sink` is the pad's element, which outlives its probe.
struct Probe {
    sink: *mut c_void,
    /// The sink was put on [`CARRIER_ADEC`].
    given: bool,
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

/// Upstream configures the bypass converter after the first buffer, with `pcm-multi-ch` off, and
/// turns the property back on with the second. The recipe that played TrueHD on the set held the
/// second buffer back until the helper had armed, for at most this long; the arm is that
/// configuration here.
const ARM_WINDOW: Duration = Duration::from_secs(5);
const ARM_POLL: Duration = Duration::from_millis(10);

/// Calls `done` until it holds or `window` has passed in steps of `step`; `sleep` is
/// `thread::sleep` outside the tests. True when `done` held.
fn hold_until(window: Duration, step: Duration, mut done: impl FnMut() -> bool, mut sleep: impl FnMut(Duration)) -> bool {
    let mut waited = Duration::ZERO;
    while !done() {
        if waited >= window {
            return false;
        }
        sleep(step);
        waited += step;
    }
    true
}

/// Whether a buffer's ES flag comes off. The feeder that played the carrier on the set never sets
/// the flag, on the first buffer or any other: the caps' `es` field declares the bitstream.
fn clears_es(start_as_bitstream: bool, flags: c_uint) -> bool {
    start_as_bitstream && flags & ES_FLAG != 0
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
                self.read_granted(&shared);
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

    /// Upstream's order around the first buffer, then zeros unless the route passes it now. The
    /// first buffer holds `pcm-multi-ch` off until the helper has armed. A buffer after the
    /// session ended is the carrier's tail and is zeroed too.
    unsafe fn buffer(&self, info: &mut ProbeInfo) -> c_int {
        let shared = session::active();
        if let Some(s) = shared.as_deref() {
            self.read_granted(s);
        }
        let bitstream = shared.as_ref().is_some_and(|s| s.carrier.start_as_bitstream);
        if bitstream {
            if self.first.swap(false, Ordering::AcqRel) {
                self.configured.store(true, Ordering::Release);
                set_multi_ch(self.sink, false);
                if let Some(s) = shared.as_deref() {
                    let named = || s.armed() || matches!(s.granted(), Granted::Refused(_));
                    if !hold_until(ARM_WINDOW, ARM_POLL, named, std::thread::sleep) {
                        crate::player::log("truehd: the route was not armed within 5 s of the first buffer");
                    }
                }
            } else if self.configured.load(Ordering::Acquire) && !self.feeding.swap(true, Ordering::AcqRel) {
                set_multi_ch(self.sink, true);
            }
        }
        let zero = !shared.is_some_and(|s| s.facts().safe(Instant::now(), s.epoch()));
        let flags = (*info.data.cast::<MiniObject>()).flags;
        let flip = clears_es(bitstream, flags);
        if !zero && !flip {
            return PROBE_OK;
        }
        let buffer = gst_mini_object_make_writable(info.data);
        if buffer.is_null() {
            return PROBE_DROP;
        }
        info.data = buffer;
        if flip {
            (*buffer.cast::<MiniObject>()).flags &= !ES_FLAG;
        }
        if zero {
            gst_buffer_memset(buffer, 0, 0, gst_buffer_get_size(buffer));
        }
        PROBE_OK
    }
}

impl Probe {
    /// What the sink's `index` and `mixer` properties say, until the sink names a decoder input.
    unsafe fn read_granted(&self, shared: &session::Shared) {
        if shared.granted() != Granted::Unknown {
            return;
        }
        let index = read_int(self.sink, c"index");
        let mixer = read_boolean(self.sink, c"mixer");
        let granted = read_back(self.given, index, mixer);
        if granted != Granted::Unknown {
            crate::player::log(&format!("truehd: LG's audio sink reports index {index:?}, mixer {mixer:?}"));
        }
        shared.note_granted(granted);
    }
}

/// A property of the sink, or `None` when its class has none by that name.
unsafe fn read_property(sink: *mut c_void, name: &CStr, kind: usize, get: unsafe fn(*const GValue) -> c_int) -> Option<c_int> {
    // G_OBJECT_GET_CLASS: an instance's first word is its class.
    if g_object_class_find_property(*sink.cast::<*mut c_void>(), name.as_ptr()).is_null() {
        return None;
    }
    let mut v = GValue { g_type: 0, _data: [0; 2] };
    g_value_init(&mut v, kind);
    g_object_get_property(sink, name.as_ptr(), &mut v);
    let value = get(&v);
    g_value_unset(&mut v);
    Some(value)
}

unsafe fn read_int(sink: *mut c_void, name: &CStr) -> Option<i32> {
    read_property(sink, name, G_TYPE_INT, |v| g_value_get_int(v))
}

unsafe fn read_boolean(sink: *mut c_void, name: &CStr) -> Option<bool> {
    read_property(sink, name, G_TYPE_BOOLEAN, |v| g_value_get_boolean(v)).map(|b| b != 0)
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

unsafe fn set_property(sink: *mut c_void, name: &CStr, kind: usize, set: impl FnOnce(*mut GValue)) {
    let mut v = GValue { g_type: 0, _data: [0; 2] };
    g_value_init(&mut v, kind);
    set(&mut v);
    g_object_set_property(sink, name.as_ptr(), &v);
    g_value_unset(&mut v);
}

unsafe fn set_int(sink: *mut c_void, name: &CStr, value: c_int) {
    set_property(sink, name, G_TYPE_INT, |v| g_value_set_int(v, value));
}

unsafe fn set_boolean(sink: *mut c_void, name: &CStr, on: bool) {
    set_property(sink, name, G_TYPE_BOOLEAN, |v| g_value_set_boolean(v, c_int::from(on)));
}

unsafe fn set_multi_ch(sink: *mut c_void, on: bool) {
    set_boolean(sink, c"pcm-multi-ch", on);
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
        assert_eq!(std::mem::offset_of!(ParamSpec, flags), 2 * std::mem::size_of::<usize>());
        assert_eq!(std::mem::offset_of!(ParamSpec, value_type), 3 * std::mem::size_of::<usize>());
    }

    #[test]
    fn the_second_buffer_waits_for_the_arm_for_at_most_the_window() {
        let mut polls = 0;
        let mut slept = Duration::ZERO;
        assert!(hold_until(ARM_WINDOW, ARM_POLL, || { polls += 1; polls > 5 }, |d| slept += d));
        assert_eq!(slept, 5 * ARM_POLL, "released by the arm, not by the clock");
        let mut slept = Duration::ZERO;
        assert!(!hold_until(ARM_WINDOW, ARM_POLL, || false, |d| slept += d));
        assert_eq!(slept, ARM_WINDOW);
        assert!(hold_until(ARM_WINDOW, ARM_POLL, || true, |_| unreachable!("already armed")));
    }

    #[test]
    fn a_bitstream_start_clears_the_es_flag_and_never_sets_it() {
        assert!(clears_es(true, ES_FLAG | 1));
        assert!(!clears_es(true, 1));
        assert!(!clears_es(false, ES_FLAG), "a start that leaves the property alone leaves the flags alone");
    }

    #[test]
    fn a_carrier_sink_goes_on_adec1_only_while_the_helper_admits_and_both_properties_take_it() {
        let index = Some((G_TYPE_INT, 1 | PARAM_WRITABLE));
        let mixer = Some((G_TYPE_BOOLEAN, 1 | PARAM_WRITABLE));
        assert!(gives_input(true, index, mixer));
        assert!(gives_input(true, Some((G_TYPE_UINT, PARAM_WRITABLE)), mixer));
        assert!(!gives_input(false, index, mixer), "a connected decoder: an HDMI input or Live TV may be in front");
        assert!(!gives_input(true, index, None), "no mixer property");
        assert!(!gives_input(true, index, Some((G_TYPE_BOOLEAN, 1))), "a read-only mixer property");
        assert!(!gives_input(true, Some((G_TYPE_INT, PARAM_WRITABLE | PARAM_CONSTRUCT_ONLY)), mixer));
        assert!(!gives_input(true, Some((G_TYPE_BOOLEAN, PARAM_WRITABLE)), mixer), "an index that is not a number");
    }

    #[test]
    fn a_sink_put_on_adec1_arms_only_while_it_reads_that_back() {
        assert_eq!(read_back(true, Some(1), Some(false)), Granted::Decoder(CARRIER_ADEC));
        assert!(matches!(read_back(true, Some(4), Some(true)), Granted::Refused(_)), "libpf put it back on its mixer channel");
        assert!(matches!(read_back(true, Some(4), Some(false)), Granted::Refused(_)), "never an arm on another input");
        assert!(matches!(read_back(true, Some(1), Some(true)), Granted::Refused(_)));
        assert_eq!(read_back(false, Some(4), Some(true)), Granted::from_sink(Some(4), Some(true)), "a sink left alone reads as before");
    }

    /// libpf maps GStreamer at exec and starts it inside its first Load. The hook waits for that,
    /// goes in as `gst_init` builds GstBin, and `gst_init` survives it; with GstBin built first,
    /// GStreamer 1.24 and older aborted registering "bin". Runs in a fresh process, which it starts
    /// GStreamer in, on the host's GStreamer with no plugins; a host without GStreamer says so.
    #[test]
    fn the_hook_goes_in_as_gstreamer_starts_and_gstreamer_survives_it() {
        const TEST: &str = "truehd::sink::tests::the_hook_goes_in_as_gstreamer_starts_and_gstreamer_survives_it";
        if plx_base::testheap::is_child() {
            return start_gstreamer_after_the_hook();
        }
        let registry = std::env::temp_dir().join(format!("plx-truehd-gst-{}.bin", std::process::id()));
        let none = std::ffi::OsStr::new("");
        let ran = plx_base::testheap::in_fresh_process_with_env(
            TEST,
            &[("GST_PLUGIN_SYSTEM_PATH_1_0", none), ("GST_PLUGIN_PATH_1_0", none), ("GST_REGISTRY_1_0", registry.as_os_str())],
        );
        let _ = std::fs::remove_file(&registry);
        if ran["gstreamer"] == 0 {
            eprintln!("no GStreamer on this host: the hook's wait for gst_init is untested here");
            return;
        }
        assert_eq!(ran["hooked"], 1, "the hook went in as GStreamer started");
    }

    fn start_gstreamer_after_the_hook() {
        let Some((lib, _)) = plx_base::dynlib::Handle::open(gst::CANDIDATES) else {
            return plx_base::testheap::report("gstreamer", 0);
        };
        assert!(install(), "the hook waits for GStreamer to start");
        assert!(HOOKED.get().is_none(), "nothing of GStreamer's is built before gst_init");
        // SAFETY: gst_init(int *argc, char ***argv) takes NULL for both.
        unsafe {
            let init: unsafe extern "C" fn(*mut c_int, *mut c_void) = std::mem::transmute(lib.sym("gst_init").expect("gst_init"));
            init(null_mut(), null_mut());
        }
        plx_base::testheap::report("gstreamer", 1);
        plx_base::testheap::report("hooked", u64::from(HOOKED.get() == Some(&true)));
    }
}
