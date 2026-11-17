//! **Where the viewer's Seerr runs, whether the Requests area is on, and the Seerr session**
//! (Settings > Integrations > Requests).
//!
//! The session cookie Seerr sets when it signs the seated Plex account in is a credential: it lives
//! under the session's `seerr` extension key, which is in the PROTECTED half of the record beside
//! the Plex tokens. [`SeerrLink`]'s `Debug` prints the host and whether a cookie is held, nothing
//! else, so a `{:?}` anywhere cannot leak it.

use super::bazarr_link::{host_of, BazarrLink, UrlProblem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeerrLink {
    /// `https://requests.example.net` or `http://192.168.1.5:5055`, no path.
    #[serde(default)]
    pub url: String,
    /// The viewer turned the Requests area on.
    #[serde(default)]
    pub enabled: bool,
    /// The `name=value` session cookie Seerr set at sign-in; empty when signed out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cookie: String,
    /// The Plex account (uuid) `cookie` was signed in as: another seated profile signs in again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account: String,
}

impl std::fmt::Debug for SeerrLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SeerrLink {{ host: {:?}, enabled: {}, session: {} }}", self.host().unwrap_or(""), self.enabled,
            if self.cookie.is_empty() { "none" } else { "held" })
    }
}

impl SeerrLink {
    /// Nothing set: the session omits the key.
    pub fn is_empty(&self) -> bool {
        self.url.is_empty() && !self.enabled && self.cookie.is_empty()
    }

    /// Turned on with an address: the Requests area is offered.
    pub fn is_ready(&self) -> bool {
        self.enabled && !self.url.is_empty()
    }

    /// The host (and port) of the address, for the one line that names it on screen.
    pub fn host(&self) -> Option<&str> {
        host_of(&self.url)
    }

    /// The cookie when it was signed in as `account`; a session of another profile is not this one's.
    pub fn session_for(&self, account: &str) -> &str {
        if self.account == account { &self.cookie } else { "" }
    }

    /// Bazarr's address rules: `https://` assumed, plain `http://` only on the home network,
    /// because the session cookie travels in a header the way Bazarr's key does.
    pub fn normalize_url(typed: &str) -> Result<String, UrlProblem> {
        BazarrLink::normalize_url(typed)
    }

    pub fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// One edit of the stored link. `Debug` never prints a cookie.
#[derive(Clone, PartialEq, Eq)]
pub enum SeerrChange {
    /// A new address, already normalised by [`SeerrLink::normalize_url`]. It drops the session:
    /// a cookie belongs to the server that set it.
    Url(String),
    Enabled(bool),
    /// Seerr at `url` signed `account` in and set `cookie`. Ignored when the address changed while
    /// the sign-in was out.
    SignedIn { url: String, cookie: String, account: String },
    /// Seerr no longer accepts the session.
    SignedOut,
    Forget,
}

impl std::fmt::Debug for SeerrChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeerrChange::Url(url) => write!(f, "SeerrChange::Url({url:?})"),
            SeerrChange::Enabled(on) => write!(f, "SeerrChange::Enabled({on})"),
            SeerrChange::SignedIn { .. } => f.write_str("SeerrChange::SignedIn(..)"),
            SeerrChange::SignedOut => f.write_str("SeerrChange::SignedOut"),
            SeerrChange::Forget => f.write_str("SeerrChange::Forget"),
        }
    }
}

impl SeerrChange {
    /// `link` with this edit made.
    pub fn applied(&self, link: &SeerrLink) -> SeerrLink {
        let signed_out = SeerrLink { cookie: String::new(), account: String::new(), ..link.clone() };
        match self {
            SeerrChange::Url(url) => SeerrLink { url: url.clone(), ..signed_out },
            SeerrChange::Enabled(on) => SeerrLink { enabled: *on, ..link.clone() },
            SeerrChange::SignedIn { url, cookie, account } if *url == link.url => {
                SeerrLink { cookie: cookie.clone(), account: account.clone(), ..link.clone() }
            }
            SeerrChange::SignedIn { .. } => link.clone(),
            SeerrChange::SignedOut => signed_out,
            SeerrChange::Forget => SeerrLink::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> SeerrLink {
        SeerrLink { url: "https://requests.example.net".into(), enabled: true, cookie: "connect.sid=secret".into(), account: "u1".into() }
    }

    #[test]
    fn debug_names_the_host_and_never_the_cookie() {
        let shown = format!("{:?} {:?}", linked(), SeerrChange::SignedIn { url: "u".into(), cookie: "connect.sid=secret".into(), account: "a".into() });
        assert!(shown.contains("requests.example.net") && shown.contains("held"));
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn a_new_address_drops_the_session_and_a_late_sign_in_for_the_old_one_is_ignored() {
        let moved = SeerrChange::Url("https://other.example.net".into()).applied(&linked());
        assert_eq!((moved.cookie.as_str(), moved.account.as_str(), moved.enabled), ("", "", true));
        let late = SeerrChange::SignedIn { url: "https://requests.example.net".into(), cookie: "c=1".into(), account: "u1".into() };
        assert_eq!(late.applied(&moved), moved);
        let current = SeerrChange::SignedIn { url: moved.url.clone(), cookie: "c=2".into(), account: "u2".into() };
        assert_eq!(current.applied(&moved).session_for("u2"), "c=2");
        assert_eq!(current.applied(&moved).session_for("u1"), "", "another profile's session is not offered");
    }

    #[test]
    fn the_link_round_trips_through_the_extension_value_and_forget_empties_it() {
        assert_eq!(SeerrLink::from_value(&linked().to_value()), linked());
        assert_eq!(SeerrLink::from_value(&Value::String("junk".into())), SeerrLink::default());
        assert!(SeerrChange::Forget.applied(&linked()).is_empty());
        assert!(!SeerrChange::Enabled(false).applied(&linked()).is_ready());
    }
}
