//! **Where the viewer's Radarr and Sonarr run, and the keys they answer to** (Settings >
//! Integrations > Radarr / Sonarr). Used only to take a deleted movie or show out of them.
//!
//! One link per app, stored under the session's `radarr` / `sonarr` extension key, which is in the
//! PROTECTED half of the record beside the Plex tokens: sealed, never in the public preferences.
//! [`ArrLink`]'s `Debug` prints the host and whether a key is set, nothing else. The address and key
//! follow Bazarr's rules ([`BazarrLink::normalize_url`] / [`BazarrLink::normalize_key`]).

use super::bazarr_link::{host_of, BazarrLink, UrlProblem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrKind {
    Radarr,
    Sonarr,
}

impl ArrKind {
    /// The product name: shown as is in every language.
    pub fn name(self) -> &'static str {
        match self {
            ArrKind::Radarr => "Radarr",
            ArrKind::Sonarr => "Sonarr",
        }
    }

    /// The session extension key the link is stored under.
    pub(super) fn key(self) -> &'static str {
        match self {
            ArrKind::Radarr => "radarr",
            ArrKind::Sonarr => "sonarr",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrLink {
    /// `https://radarr.example.net` or `http://192.168.1.5:7878`, no path.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
}

impl std::fmt::Debug for ArrLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ArrLink {{ host: {:?}, key: {} }}", self.host().unwrap_or(""), if self.api_key.is_empty() { "unset" } else { "set" })
    }
}

impl ArrLink {
    /// Nothing set: the session omits the key.
    pub fn is_empty(&self) -> bool {
        self.url.is_empty() && self.api_key.is_empty()
    }

    /// An address and a key are both present: a deletion asks this app too.
    pub fn is_complete(&self) -> bool {
        !self.url.is_empty() && !self.api_key.is_empty()
    }

    pub fn host(&self) -> Option<&str> {
        host_of(&self.url)
    }

    pub fn normalize_url(typed: &str) -> Result<String, UrlProblem> {
        BazarrLink::normalize_url(typed)
    }

    pub fn normalize_key(typed: &str) -> Option<String> {
        BazarrLink::normalize_key(typed)
    }

    pub(super) fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    pub(super) fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// One edit of a stored link. Already normalised; `Debug` never prints a key.
#[derive(Clone, PartialEq, Eq)]
pub enum ArrChange {
    Url(String),
    Key(String),
    Forget,
}

impl std::fmt::Debug for ArrChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArrChange::Url(url) => write!(f, "ArrChange::Url({url:?})"),
            ArrChange::Key(_) => f.write_str("ArrChange::Key(..)"),
            ArrChange::Forget => f.write_str("ArrChange::Forget"),
        }
    }
}

impl ArrChange {
    /// `link` with this edit made.
    pub fn applied(&self, link: &ArrLink) -> ArrLink {
        match self {
            ArrChange::Url(url) => ArrLink { url: url.clone(), ..link.clone() },
            ArrChange::Key(key) => ArrLink { api_key: key.clone(), ..link.clone() },
            ArrChange::Forget => ArrLink::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_key() {
        let link = ArrLink { url: "https://radarr.example.net".into(), api_key: "0123456789abcdef".into() };
        let shown = format!("{link:?} {:?}", ArrChange::Key("0123456789abcdef".into()));
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert!(shown.contains("radarr.example.net"));
    }

    #[test]
    fn an_edit_changes_one_half_and_forget_clears_both() {
        let link = ArrLink { url: "https://a.example.net".into(), api_key: "k1".into() };
        assert_eq!(ArrChange::Url("https://b.example.net".into()).applied(&link).api_key, "k1");
        assert_eq!(ArrChange::Key("k2".into()).applied(&link).url, "https://a.example.net");
        assert!(ArrChange::Forget.applied(&link).is_empty());
        assert!(link.is_complete() && !ArrLink { url: link.url.clone(), api_key: String::new() }.is_complete());
    }

    #[test]
    fn the_link_round_trips_through_the_extension_value() {
        let link = ArrLink { url: "http://192.168.1.5:7878".into(), api_key: "k".into() };
        assert_eq!(ArrLink::from_value(&link.to_value()), link);
        assert_eq!(ArrLink::from_value(&Value::String("junk".into())), ArrLink::default());
    }
}
