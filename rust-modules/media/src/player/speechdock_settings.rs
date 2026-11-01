//! SpeechDock connection settings and session-only diagnostic consent.

use std::net::Ipv6Addr;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};

static ORIGIN: Mutex<Option<String>> = Mutex::new(None);
static DIAGNOSTICS_CONSENT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginError;

/// Accept only an HTTP(S) origin, never user information, a path, query, or fragment.
/// An empty value disables alignment. An optional trailing slash is permitted.
pub fn normalize_origin(value: &str) -> Result<Option<String>, OriginError> {
    let value = value.trim();
    if value.is_empty() { return Ok(None); }
    if value.len() > 200 { return Err(OriginError); }
    let (scheme, authority) = value.split_once("://").ok_or(OriginError)?;
    let scheme = if scheme.eq_ignore_ascii_case("http") { "http" }
        else if scheme.eq_ignore_ascii_case("https") { "https" }
        else { return Err(OriginError) };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.bytes().any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b'\\') || b.is_ascii_whitespace() || b.is_ascii_control()) {
        return Err(OriginError);
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (ip, suffix) = rest.split_once(']').ok_or(OriginError)?;
        ip.parse::<Ipv6Addr>().map_err(|_| OriginError)?;
        let port = if suffix.is_empty() { None } else { Some(suffix.strip_prefix(':').ok_or(OriginError)?) };
        (&authority[..ip.len() + 2], port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if host.len() > 253 || host.is_empty() || !host.split('.').all(|label| {
            !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }) { return Err(OriginError); }
        (host, port)
    };
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || !port.parse::<u16>().is_ok_and(|n| n != 0) {
            return Err(OriginError);
        }
    }
    Ok(Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }))
}

pub fn origin() -> Option<String> {
    ORIGIN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Invalid input leaves the previously configured origin unchanged.
pub fn set_origin(value: &str) -> Result<(), OriginError> {
    let origin = normalize_origin(value)?;
    let mut current = ORIGIN.lock().unwrap_or_else(|e| e.into_inner());
    if *current != origin {
        // Advance the epoch before publishing the new origin, even if it changes back
        // before the next playback pump. Invalidation uses a nonblocking state lock.
        super::alignment::invalidate();
        DIAGNOSTICS_CONSENT.store(false, Ordering::Relaxed);
        *current = origin;
    }
    Ok(())
}

/// Never restored from persistent or global logging preferences.
pub fn diagnostics_consent() -> bool {
    DIAGNOSTICS_CONSENT.load(Ordering::Relaxed)
}

pub fn set_diagnostics_consent(consent: bool) {
    DIAGNOSTICS_CONSENT.store(consent, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_origins() {
        assert_eq!(normalize_origin(" "), Ok(None));
        assert_eq!(normalize_origin("HTTPS://speech.example:8443/"), Ok(Some("https://speech.example:8443".into())));
        assert_eq!(normalize_origin("http://[::1]:8080"), Ok(Some("http://[::1]:8080".into())));
        for bad in ["ftp://host", "http://u:p@host", "http://host/path", "http://host?x=1", "http://host/#x", "http://host:0", "http://host:65536", "http://[::xyz]", "http://bad host", "http://host//"] {
            assert_eq!(normalize_origin(bad), Err(OriginError));
        }
    }
}
