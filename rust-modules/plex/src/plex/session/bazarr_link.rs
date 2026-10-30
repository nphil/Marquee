//! **Where the viewer's Bazarr runs, and the key it answers to** (Settings > Integrations > Bazarr).
//!
//! The viewer types both on the television. The key rides in the PROTECTED half of the session
//! (`CanonicalSessionAuth`), beside the Plex tokens: sealed by the key manager where one exists,
//! never in the public preferences, never in a log line. [`BazarrLink`]'s `Debug` prints the host
//! and whether a key is set, and nothing else, so a `{:?}` anywhere (a test failure, an event-log
//! line) cannot leak it.

use serde::{Deserialize, Serialize};

/// Longest address and key accepted: a Bazarr key is 32 hex characters; a URL longer than this is
/// a paste gone wrong.
pub const MAX_URL: usize = 200;
pub const MAX_KEY: usize = 128;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazarrLink {
    /// `https://bazarr.example.net` or `http://192.168.1.5:6767`, no path.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
}

impl std::fmt::Debug for BazarrLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BazarrLink {{ host: {:?}, key: {} }}", self.host().unwrap_or(""), if self.api_key.is_empty() { "unset" } else { "set" })
    }
}

/// Why an address was not accepted. The Settings page says each differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlProblem {
    Empty,
    /// No `http://` or `https://` in front.
    Scheme,
    /// No host, or one with a character a host cannot hold.
    Host,
    /// Plain `http://` to a name that is not on the home network: the key would cross the internet
    /// in the clear.
    Plaintext,
}

impl BazarrLink {
    /// Nothing set: the Integrations row reads "not set up" and the session omits the field.
    pub fn is_empty(&self) -> bool {
        self.url.is_empty() && self.api_key.is_empty()
    }

    /// An address and a key are both present: the player may offer Bazarr.
    pub fn is_complete(&self) -> bool {
        !self.url.is_empty() && !self.api_key.is_empty()
    }

    /// The host (and port) of the address, for the one line that names it on screen.
    pub fn host(&self) -> Option<&str> {
        let rest = self.url.split_once("://")?.1;
        let host = rest.split(['/', '?', '#']).next()?;
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        (!host.is_empty()).then_some(host)
    }

    /// The address as a typed string settles to: trimmed, a scheme in front, no trailing slash or
    /// path, or the reason it cannot be one. `https://` is assumed when no scheme was typed and
    /// the host looks like a name; plain `http://` is accepted only for the home network (a private
    /// address, `localhost`, a single-label name or `.local`), because the key travels in a header.
    pub fn normalize_url(typed: &str) -> Result<String, UrlProblem> {
        let t = typed.trim();
        if t.is_empty() {
            return Err(UrlProblem::Empty);
        }
        if t.len() > MAX_URL {
            return Err(UrlProblem::Host);
        }
        let (scheme, rest) = match t.split_once("://") {
            Some((s, r)) if s.eq_ignore_ascii_case("http") => ("http", r),
            Some((s, r)) if s.eq_ignore_ascii_case("https") => ("https", r),
            Some(_) => return Err(UrlProblem::Scheme),
            None if t.contains(':') && !t.rsplit_once(':').is_some_and(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())) => {
                return Err(UrlProblem::Scheme)
            }
            None => ("https", t),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() || authority.contains('@') {
            return Err(UrlProblem::Host);
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p)) => (h, Some(p)),
            Some((_, _)) if !authority.starts_with('[') => return Err(UrlProblem::Host),
            _ => (authority, None),
        };
        let host_ok = !host.is_empty()
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'[' | b']' | b':'))
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-']);
        if !host_ok {
            return Err(UrlProblem::Host);
        }
        if scheme == "http" && !is_home_network(host) {
            return Err(UrlProblem::Plaintext);
        }
        Ok(match port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        })
    }

    /// A pasted key as it settles: trimmed, no inner whitespace, within the cap; `None` when it
    /// is empty or holds a character an HTTP header cannot carry.
    pub fn normalize_key(typed: &str) -> Option<String> {
        let k = typed.trim();
        let ok = !k.is_empty()
            && k.len() <= MAX_KEY
            && k.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\');
        ok.then(|| k.to_string())
    }
}

/// One edit of the stored link, as the Settings page makes it: the address, the key, or both
/// forgotten. Already normalised by [`BazarrLink::normalize_url`] / [`BazarrLink::normalize_key`];
/// `Debug` never prints a key.
#[derive(Clone, PartialEq, Eq)]
pub enum BazarrChange {
    Url(String),
    Key(String),
    Forget,
}

impl std::fmt::Debug for BazarrChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BazarrChange::Url(url) => write!(f, "BazarrChange::Url({url:?})"),
            BazarrChange::Key(_) => f.write_str("BazarrChange::Key(..)"),
            BazarrChange::Forget => f.write_str("BazarrChange::Forget"),
        }
    }
}

impl BazarrChange {
    /// `link` with this edit made. A changed address keeps the key: it is the viewer's own, and the
    /// Test row says at once whether the pair still works.
    pub fn applied(&self, link: &BazarrLink) -> BazarrLink {
        match self {
            BazarrChange::Url(url) => BazarrLink { url: url.clone(), ..link.clone() },
            BazarrChange::Key(key) => BazarrLink { api_key: key.clone(), ..link.clone() },
            BazarrChange::Forget => BazarrLink::default(),
        }
    }
}

/// Is `host` on the home network: loopback, a private IPv4 range, link-local, a single-label name
/// or one ending `.local`? Used only to decide whether plain `http://` is acceptable.
fn is_home_network(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host == "::1" || host.ends_with(".local") || (!host.contains('.') && !host.contains(':')) {
        return true;
    }
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() == 4 && host.split('.').count() == 4 {
        return matches!(
            octets[..],
            [10, ..] | [127, ..] | [192, 168, ..] | [169, 254, ..] | [172, 16..=31, ..]
        );
    }
    host.starts_with("fe80:") || host.starts_with("fc") || host.starts_with("fd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_key() {
        let link = BazarrLink { url: "https://bazarr.example.net".into(), api_key: "0123456789abcdef0123456789abcdef".into() };
        let shown = format!("{link:?} {:?}", Some(&link));
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert!(shown.contains("bazarr.example.net") && shown.contains("set"));
        assert!(format!("{:?}", BazarrLink::default()).contains("unset"));
    }

    #[test]
    fn addresses_settle_to_scheme_host_port() {
        for (typed, want) in [
            ("https://bazarr.nateshome.net", "https://bazarr.nateshome.net"),
            ("  https://bazarr.nateshome.net/  ", "https://bazarr.nateshome.net"),
            ("HTTPS://Bazarr.Example.net/settings/general", "https://Bazarr.Example.net"),
            ("bazarr.nateshome.net", "https://bazarr.nateshome.net"),
            ("bazarr.nateshome.net:6767", "https://bazarr.nateshome.net:6767"),
            ("http://192.168.1.50:6767", "http://192.168.1.50:6767"),
            ("http://bazarr:6767/", "http://bazarr:6767"),
            ("http://nas.local", "http://nas.local"),
            ("http://localhost:6767", "http://localhost:6767"),
            ("http://172.20.0.5:6767", "http://172.20.0.5:6767"),
        ] {
            assert_eq!(BazarrLink::normalize_url(typed).as_deref(), Ok(want), "{typed:?}");
        }
    }

    #[test]
    fn addresses_that_cannot_work_say_why() {
        assert_eq!(BazarrLink::normalize_url("   "), Err(UrlProblem::Empty));
        assert_eq!(BazarrLink::normalize_url("ftp://x.example.net"), Err(UrlProblem::Scheme));
        assert_eq!(BazarrLink::normalize_url("https://"), Err(UrlProblem::Host));
        assert_eq!(BazarrLink::normalize_url("https://user:pw@bazarr.example.net"), Err(UrlProblem::Host));
        assert_eq!(BazarrLink::normalize_url("https://bad host.example.net"), Err(UrlProblem::Host));
        assert_eq!(BazarrLink::normalize_url("https://bazarr.example.net:99999"), Err(UrlProblem::Host));
        assert_eq!(BazarrLink::normalize_url("http://bazarr.example.net"), Err(UrlProblem::Plaintext));
        assert_eq!(BazarrLink::normalize_url("http://8.8.8.8:6767"), Err(UrlProblem::Plaintext));
        assert_eq!(BazarrLink::normalize_url("http://172.32.0.1"), Err(UrlProblem::Plaintext), "172.32 is public");
    }

    #[test]
    fn keys_are_trimmed_and_header_safe() {
        assert_eq!(BazarrLink::normalize_key(" abc123 ").as_deref(), Some("abc123"));
        for bad in ["", "   ", "has space", "quo\"te", "back\\slash", "tab\tkey", &"x".repeat(MAX_KEY + 1)] {
            assert_eq!(BazarrLink::normalize_key(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_edit_changes_one_half_and_forget_clears_both() {
        let link = BazarrLink { url: "https://a.example.net".into(), api_key: "secret-key".into() };
        let moved = BazarrChange::Url("https://b.example.net".into()).applied(&link);
        assert_eq!((moved.url.as_str(), moved.api_key.as_str()), ("https://b.example.net", "secret-key"));
        let rekeyed = BazarrChange::Key("other".into()).applied(&link);
        assert_eq!((rekeyed.url.as_str(), rekeyed.api_key.as_str()), ("https://a.example.net", "other"));
        assert!(BazarrChange::Forget.applied(&link).is_empty());
        assert!(!format!("{:?}", BazarrChange::Key("secret-key".into())).contains("secret"));
    }

    #[test]
    fn host_names_the_machine_only() {
        let link = BazarrLink { url: "https://bazarr.nateshome.net:6767".into(), api_key: "k".into() };
        assert_eq!(link.host(), Some("bazarr.nateshome.net:6767"));
        assert_eq!(BazarrLink::default().host(), None);
        assert!(link.is_complete() && !link.is_empty());
        assert!(!BazarrLink { url: "https://x.net".into(), api_key: String::new() }.is_complete());
    }
}
