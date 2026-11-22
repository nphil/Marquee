//! **Ranges of one media object over connections bound to source addresses**: the transport
//! [`crate::multilink`] drives beside the default reader.
//!
//! It has a multi handle of its own, so the default reader's transfer and its connection cache
//! are never touched. Every request is a fresh easy handle for the object's own URL with one
//! closed range, the object's `If-Range`, the trust the default reader's open settled on, and the
//! address libcurl binds before it connects (`CURLOPT_INTERFACE` as `host!`: an address, never a
//! device). libcurl reuses a connection only for the address it was bound to, so each source
//! keeps its own. Redirects are not followed: the URL carries the token, and a range from another
//! origin is another object.
//!
//! What an answer is worth is [`crate::multilink::engine`]'s to judge. This file decides only
//! what must be decided before a byte is kept: nothing of an answer that is not a `206` is
//! stored, and nothing past the length that was asked for.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_void};
use std::sync::Arc;

use plx_net::net::{self, curl_slist, keypin};

use super::{
    curl_multi_add_handle, curl_multi_cleanup, curl_multi_info_read, curl_multi_init, curl_multi_perform, curl_multi_remove_handle,
    msg_result, multi_wait, Abort, CurlSource, Wait, CURL, CURLM, CURLMSG_DONE, CURLM_OK, CURLOPT_BUFFERSIZE, CURLOPT_CONNECTTIMEOUT,
    CURLOPT_FOLLOWLOCATION, CURLOPT_HEADERDATA, CURLOPT_HEADERFUNCTION, CURLOPT_NOPROGRESS, CURLOPT_NOSIGNAL, CURLOPT_PROTOCOLS,
    CURLOPT_RANGE, CURLOPT_RESOLVE, CURLOPT_SSL_VERIFYHOST, CURLOPT_SSL_VERIFYPEER, CURLOPT_TCP_NODELAY, CURLOPT_URL,
    CURLOPT_WRITEDATA, CURLOPT_WRITEFUNCTION, CURLPROTO_HTTP, CURLPROTO_HTTPS,
};
use crate::multilink::drive::{Arrival, Transport};
use crate::multilink::engine::{Fault, ReqId, Request};
use crate::multilink::object::Head;
use crate::multilink::reader::Wire;

const CURLOPT_HTTPHEADER: c_int = 10023;
const CURLOPT_INTERFACE: c_int = 10062;
const CURLOPT_IPRESOLVE: c_int = 113;
const CURL_IPRESOLVE_V4: c_long = 1;
const CURLOPT_SOCKOPTFUNCTION: c_int = 20148;

/// libcurl's own bound on a connection attempt. The engine gives a silent request up after a
/// second; this only ends an attempt nobody is waiting for any more.
const CONNECT_S: c_long = 3;
/// The receive buffer of a connection over the built-in wire. The server can have little more
/// than this on its way, so what queues at the switch ahead of the TV's own traffic stays in the
/// low tens of milliseconds of the wire's 100 Mbit/s.
const WIRE_RECEIVE: c_int = 128 << 10;
/// How many times one poll goes round its connections. libcurl 7.53 hands over one TLS record
/// of a connection a call, 16 KiB at most: a single call a turn holds a link to what the turns
/// bring, not to what it carries. The bound keeps one turn to half a megabyte a connection.
const READS_A_POLL: u32 = 32;
/// Dialled requests are told at most this often.
const DIAL_TOLD_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// One request in flight. Boxed: libcurl holds its address for the two callbacks.
struct Flight {
    id: ReqId,
    easy: *mut CURL,
    /// The lists the handle was given. libcurl keeps the pointers, so they are freed after it.
    headers: *mut curl_slist,
    resolve: *mut curl_slist,
    /// Held for the life of the handle, as [`CurlSource`] holds its own strings.
    range: CString,
    source: CString,
    /// Body bytes asked for, and received.
    asked: u64,
    received: u64,
    head: Head,
    /// The answer's status, kept once `head` has been reported.
    status: u16,
    /// The head is complete, and has been reported.
    headed: bool,
    reported: bool,
    /// Body received and not yet reported.
    body: Vec<u8>,
    /// The answer brought more than was asked for.
    overran: bool,
}

impl Flight {
    /// How a transfer libcurl ended with `rc` ended for the engine.
    fn ended(&self, rc: c_int) -> Result<(), Fault> {
        match rc {
            0 if self.reported && self.status == 206 && self.received == self.asked => Ok(()),
            _ if self.overran => Err(Fault::Overrun),
            // The handshake, the certificate or the pin.
            35 | 51 | 58 | 59 | 60 | 77 | 83 | 90 | 91 => Err(Fault::Tls),
            // No name, no route from that address, refused, or the address could not be bound.
            5 | 6 | 7 | 45 => Err(Fault::Connect),
            28 if !self.headed => Err(Fault::Connect),
            _ => Err(Fault::Cut),
        }
    }

    /// Free the handle and what it was given.
    ///
    /// # Safety
    /// The handle is in no multi.
    unsafe fn free(&mut self) {
        unsafe {
            net::curl_easy_cleanup(self.easy);
            // After the handle, never before: the lists must outlive the transfer.
            for list in [self.headers, self.resolve] {
                if !list.is_null() {
                    net::curl_slist_free_all(list);
                }
            }
        }
    }
}

pub struct Ranges {
    multi: *mut CURLM,
    /// What ends a wait at teardown: the default reader's own signal when it is a curl source.
    abort: Arc<Abort>,
    url: CString,
    resolve: Option<CString>,
    /// The roots bundle the default reader's open settled on, when it is not the firmware's.
    roots: Option<CString>,
    flights: Vec<Box<Flight>>,
    /// libcurl refused its own bookkeeping: nothing more is sent on this multi.
    broken: bool,
    /// When a dialled request was last told, and how many were left unlogged since.
    dial_told: Option<std::time::Instant>,
    dials_unlogged: u32,
}

impl Ranges {
    /// Ranges of the object `source` opened at `url`, woken by the same teardown signal. `Err`
    /// says why this object gets none.
    pub fn beside(source: &CurlSource, url: &str) -> Result<Ranges, &'static str> {
        if source.url.as_bytes() != url.as_bytes() || source.hop_url != source.url {
            return Err("its open was redirected");
        }
        Ranges::of(url, source.abort.clone())
    }

    /// Ranges of the object a plaintext socket has open at `url`. Nothing signals this transport
    /// at teardown, so its reader keeps every wait short.
    pub fn alone(url: &str) -> Result<Ranges, &'static str> {
        Ranges::of(url, Abort::new().ok_or("no wake pipe")?)
    }

    fn of(url: &str, abort: Arc<Abort>) -> Result<Ranges, &'static str> {
        if !super::available() {
            return Err("this libcurl has no multi interface");
        }
        if !super::media_url_allowed(url) {
            return Err("its URL may not carry the token");
        }
        let (origin, _) = plx_plex::plex::origin::split(url);
        let resolve = super::resolve_entry_for(&origin).map_err(|_| "its name is not resolved")?;
        let roots = match keypin::key_of_url(url).map(|key| keypin::begin(&key)) {
            // A remembered key stands in for a certificate out of its dates. That trust is the
            // default reader's alone.
            Some(keypin::Mode::Key { .. }) => return Err("its server is trusted by a remembered key"),
            Some(keypin::Mode::Roots { path, .. }) => Some(CString::new(path).map_err(|_| "no roots path")?),
            Some(keypin::Mode::Strict) | None => None,
        };
        let url = CString::new(url).map_err(|_| "its URL is not a C string")?;
        let multi = unsafe { curl_multi_init() };
        if multi.is_null() {
            return Err("no multi handle");
        }
        Ok(Ranges { multi, abort, url, resolve, roots, flights: Vec::new(), broken: false, dial_told: None, dials_unlogged: 0 })
    }

    /// Give `flight`'s handle everything a request needs. False when libcurl refused an option
    /// the request may not be sent without.
    ///
    /// # Safety
    /// `flight.easy` is a live easy handle in no multi.
    unsafe fn prepare(&self, flight: &mut Flight, request: &Request, if_range: Option<&str>) -> bool {
        let easy = flight.easy;
        let data = flight as *mut Flight as *mut c_void;
        unsafe {
            let mut refused = 0;
            for (option, value) in [
                (CURLOPT_URL, self.url.as_ptr() as *const c_void),
                (CURLOPT_RANGE, flight.range.as_ptr() as *const c_void),
                // Bound before it connects, or it is not this link's request.
                (CURLOPT_INTERFACE, flight.source.as_ptr() as *const c_void),
                (CURLOPT_WRITEFUNCTION, body_cb as *const c_void),
                (CURLOPT_WRITEDATA, data),
                (CURLOPT_HEADERFUNCTION, head_cb as *const c_void),
                (CURLOPT_HEADERDATA, data),
            ] {
                refused |= net::curl_easy_setopt_ptr(easy, option, value);
            }
            for (option, value) in [
                (CURLOPT_SSL_VERIFYPEER, 1),
                (CURLOPT_SSL_VERIFYHOST, 2),
                (CURLOPT_FOLLOWLOCATION, 0),
                (CURLOPT_PROTOCOLS, CURLPROTO_HTTP | CURLPROTO_HTTPS),
            ] {
                refused |= net::curl_easy_setopt_long(easy, option, value);
            }
            net::apply_user_agent(easy);
            if let Some(tag) = if_range {
                let Ok(line) = CString::new(format!("If-Range: {tag}")) else { return false };
                flight.headers = net::curl_slist_append(std::ptr::null_mut(), line.as_ptr());
                if flight.headers.is_null() {
                    return false;
                }
                refused |= net::curl_easy_setopt_ptr(easy, CURLOPT_HTTPHEADER, flight.headers as *const c_void);
            }
            if let Some(entry) = &self.resolve {
                flight.resolve = net::curl_slist_append(std::ptr::null_mut(), entry.as_ptr());
                if flight.resolve.is_null() {
                    return false;
                }
                let rc = net::curl_easy_setopt_ptr(easy, CURLOPT_RESOLVE, flight.resolve as *const c_void);
                if net::resolve::note_setopt(rc).is_err() {
                    return false;
                }
            }
            if let Some(path) = &self.roots {
                if keypin::apply_roots(easy, path).is_err() {
                    return false;
                }
            }
            // The rest shapes the transfer and guards nothing: a libcurl without one of them
            // still sends the same request.
            net::curl_easy_setopt_long(easy, CURLOPT_IPRESOLVE, CURL_IPRESOLVE_V4);
            net::curl_easy_setopt_long(easy, CURLOPT_NOSIGNAL, 1);
            net::curl_easy_setopt_long(easy, CURLOPT_NOPROGRESS, 1);
            net::curl_easy_setopt_long(easy, CURLOPT_TCP_NODELAY, 1);
            net::curl_easy_setopt_long(easy, CURLOPT_BUFFERSIZE, 65536);
            net::curl_easy_setopt_long(easy, CURLOPT_CONNECTTIMEOUT, CONNECT_S);
            if request.bounded {
                net::curl_easy_setopt_ptr(easy, CURLOPT_SOCKOPTFUNCTION, bound_receive_cb as *const c_void);
            }
            refused == 0
        }
    }

    /// Take `flight` out of the multi and free it. A handle libcurl will not let go is left
    /// with it, as [`CurlSource::stop`] leaves its own, and nothing more is sent here.
    fn retire(&mut self, mut flight: Box<Flight>) {
        unsafe {
            if curl_multi_remove_handle(self.multi, flight.easy) == CURLM_OK {
                flight.free();
            } else {
                self.broken = true;
                std::mem::forget(flight);
            }
        }
    }

    /// Every request has failed with the multi itself.
    fn fail_all(&mut self, arrived: &mut Vec<Arrival>) {
        self.broken = true;
        while let Some(flight) = self.flights.pop() {
            arrived.push(Arrival::Failed(flight.id, Fault::Connect));
            self.retire(flight);
        }
    }
}

impl Transport for Ranges {
    fn start(&mut self, request: &Request, if_range: Option<&str>) -> bool {
        // Nothing is dialled under a teardown the main thread is already waiting behind.
        if self.broken || self.abort.is_set() {
            return false;
        }
        let (Ok(range), Ok(source)) = (CString::new(format!("{}-{}", request.first, request.last)), CString::new(format!("host!{}", request.source)))
        else {
            return false;
        };
        let easy = unsafe { net::curl_easy_init() };
        if easy.is_null() {
            return false;
        }
        let mut flight = Box::new(Flight {
            id: request.id,
            easy,
            headers: std::ptr::null_mut(),
            resolve: std::ptr::null_mut(),
            range,
            source,
            asked: request.last - request.first + 1,
            received: 0,
            head: Head::default(),
            status: 0,
            headed: false,
            reported: false,
            body: Vec::new(),
            overran: false,
        });
        unsafe {
            if !self.prepare(&mut flight, request, if_range) || curl_multi_add_handle(self.multi, easy) != CURLM_OK {
                flight.free();
                return false;
            }
        }
        self.flights.push(flight);
        true
    }

    fn cancel(&mut self, id: ReqId) {
        if let Some(at) = self.flights.iter().position(|flight| flight.id == id) {
            let flight = self.flights.swap_remove(at);
            self.retire(flight);
        }
    }

    fn poll(&mut self, arrived: &mut Vec<Arrival>) {
        if self.flights.is_empty() {
            return;
        }
        for _ in 0..READS_A_POLL {
            let mut running = 0;
            if self.broken || unsafe { curl_multi_perform(self.multi, &mut running) } != CURLM_OK {
                self.fail_all(arrived);
                return;
            }
            let mut brought = false;
            for flight in &mut self.flights {
                if flight.headed && !flight.reported {
                    flight.reported = true;
                    // A request that had to dial: where its wait for an answer went. A reused
                    // connection says nothing, and a burst of dials is told once a quarter second.
                    let (dialled, timing) = net::connect_timing(flight.easy);
                    if dialled > 0 {
                        if self.dial_told.is_none_or(|at| at.elapsed() >= DIAL_TOLD_EVERY) {
                            crate::player::log(&format!(
                                "multilink: a request dialled: {timing} ({} more dials unlogged)",
                                self.dials_unlogged
                            ));
                            (self.dial_told, self.dials_unlogged) = (Some(std::time::Instant::now()), 0);
                        } else {
                            self.dials_unlogged += 1;
                        }
                    }
                    arrived.push(Arrival::Head(flight.id, std::mem::take(&mut flight.head)));
                    brought = true;
                }
                if !flight.body.is_empty() {
                    arrived.push(Arrival::Body(flight.id, std::mem::take(&mut flight.body)));
                    brought = true;
                }
            }
            if !brought {
                break;
            }
        }
        loop {
            let mut left = 0;
            let message = unsafe { curl_multi_info_read(self.multi, &mut left) };
            if message.is_null() {
                break;
            }
            let (kind, easy, rc) = unsafe { ((*message).msg, (*message).easy_handle, msg_result(message)) };
            if kind != CURLMSG_DONE {
                continue;
            }
            let Some(at) = self.flights.iter().position(|flight| flight.easy == easy) else { continue };
            let flight = self.flights.swap_remove(at);
            arrived.push(match flight.ended(rc) {
                Ok(()) => Arrival::Done(flight.id),
                Err(fault) => Arrival::Failed(flight.id, fault),
            });
            self.retire(flight);
        }
    }
}

impl Wire for Ranges {
    fn wait(&mut self, most_ms: u32) {
        // A wait that fails returns at once: the next poll then fails every request, and with
        // no request and no proven link the reader has nothing left to wait here for.
        if let Wait::Failed(_) = multi_wait(self.multi, &self.abort, most_ms.min(c_int::MAX as u32) as c_int) {
            self.broken = true;
        }
    }
}

impl Drop for Ranges {
    fn drop(&mut self) {
        while let Some(flight) = self.flights.pop() {
            self.retire(flight);
        }
        // A multi that kept a handle it would not release is left with it.
        if !self.broken {
            unsafe { curl_multi_cleanup(self.multi) };
        }
    }
}

/// `CURLOPT_WRITEFUNCTION`. Returning anything but the length ends the transfer, which is the
/// answer to a body that is not a `206`'s or runs past what was asked.
extern "C" fn body_cb(ptr: *mut c_char, size: usize, nmemb: usize, data: *mut c_void) -> usize {
    let n = size.saturating_mul(nmemb);
    if data.is_null() || ptr.is_null() {
        return 0;
    }
    let flight = unsafe { &mut *(data as *mut Flight) };
    if flight.status != 206 {
        return 0;
    }
    if flight.received + n as u64 > flight.asked {
        flight.overran = true;
        return 0;
    }
    // Exactly what arrived: the engine counts what it holds by these lengths.
    flight.body.reserve_exact(n);
    flight.body.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr as *const u8, n) });
    flight.received += n as u64;
    n
}

/// `CURLOPT_HEADERFUNCTION`: one line at a time, the blank line that ends a head included.
extern "C" fn head_cb(ptr: *mut c_char, size: usize, nmemb: usize, data: *mut c_void) -> usize {
    let n = size.saturating_mul(nmemb);
    if data.is_null() || ptr.is_null() {
        return 0;
    }
    let flight = unsafe { &mut *(data as *mut Flight) };
    if flight.reported {
        // Trailers: the head the engine judged was the answer's own.
        return n;
    }
    if flight.head.line(unsafe { std::slice::from_raw_parts(ptr as *const u8, n) }) {
        flight.headed = true;
    }
    flight.status = flight.head.status;
    n
}

/// `CURLOPT_SOCKOPTFUNCTION`, called on the new socket before it connects, which is the only
/// time a receive buffer still sets the window the connection offers.
extern "C" fn bound_receive_cb(_data: *mut c_void, fd: c_int, _purpose: c_int) -> c_int {
    let size = WIRE_RECEIVE;
    unsafe {
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, &size as *const c_int as *const c_void, std::mem::size_of::<c_int>() as libc::socklen_t);
    }
    0
}
