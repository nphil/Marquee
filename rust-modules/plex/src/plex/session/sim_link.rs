//! The screenshot harness's runtime-only integration links. All three point at the harness's
//! loopback listener, with fixed synthetic identities. No user credential enters this path.

use super::{ArrKind, ArrLink, SeerrLink};

pub const ACCOUNT: &str = "demo-sim-account";
pub const PLEX_TOKEN: &str = "demo-sim-plextv-token";

pub fn base() -> Option<&'static str> {
    static BASE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        let v = plx_base::devtrig::read("reqlink")?;
        let port = v.strip_prefix("http://127.0.0.1:")?;
        (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p != 0))
            .then_some(v)
    })
    .as_deref()
}

pub(super) fn seerr() -> Option<SeerrLink> {
    Some(SeerrLink { url: base()?.into(), enabled: true, ..Default::default() })
}

pub(super) fn arr(kind: ArrKind) -> Option<ArrLink> {
    Some(ArrLink {
        url: base()?.into(),
        api_key: match kind {
            ArrKind::Radarr => "demo-radarr-key",
            ArrKind::Sonarr => "demo-sonarr-key",
        }.into(),
    })
}
