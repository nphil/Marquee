//! **What the LumaTV helper says about the TV's links**, parsed and judged.
//!
//! The helper's Multi Link plugin publishes one state on loopback
//! (`GET /v1/state/network.multi-link`): the source addresses it has prepared a route for, with the
//! facts that identify each, and how long the state vouches for itself. Everything here is a pure
//! reading of that answer. Anything it does not expect reads as "no links", and the reader then
//! stays on the default route.

use std::net::{IpAddr, Ipv4Addr};

use serde_json::Value;

pub const URL: &str = "http://127.0.0.1:18731/v1/state/network.multi-link";
const TOPIC: &str = "network.multi-link";
/// The longest a state may vouch for itself, whatever it claims. The helper says 1000 ms and
/// refreshes every 500.
const VALID_LONGEST_MS: u64 = 2_000;
/// More candidates than a television has ports for is not a state this reads.
const CANDIDATES_MOST: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ethernet,
    UsbEthernet,
    Wifi,
}

/// One source address the helper prepared. `interface`, `address`, `prefix` and `carrier_changes`
/// are its identity ([`Candidate::same_link`]): a proof holds for exactly these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub interface: String,
    pub kind: Kind,
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub carrier_changes: u64,
    /// The built-in wire, which the TV's own traffic and its remote controls use.
    pub control: bool,
}

impl Candidate {
    pub fn same_link(&self, other: &Candidate) -> bool {
        self.interface == other.interface
            && self.address == other.address
            && self.prefix == other.prefix
            && self.carrier_changes == other.carrier_changes
    }
}

/// A usable state: the plugin is on and active, and offers these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The helper's start. Another value is another helper: nothing proven before it holds.
    pub boot: u64,
    /// When the helper sampled the links, in milliseconds of `CLOCK_BOOTTIME`.
    pub sampled_ms: u64,
    pub valid_for_ms: u64,
    pub candidates: Vec<Candidate>,
}

/// Why an answer offers nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unusable {
    /// No answer: the helper or its plugin is not there.
    Missing,
    Malformed,
    /// A schema this build does not read.
    Schema,
    Disabled,
    Inactive,
    /// Older than it vouches for, or dated after now.
    Stale,
}

impl Unusable {
    pub fn name(self) -> &'static str {
        match self {
            Unusable::Missing => "missing",
            Unusable::Malformed => "malformed",
            Unusable::Schema => "unknown schema",
            Unusable::Disabled => "disabled",
            Unusable::Inactive => "inactive",
            Unusable::Stale => "stale",
        }
    }
}

impl Status {
    /// Whether the state still vouches for itself at `now_ms` of the helper's clock. A state dated
    /// after now is not believed either: the clocks disagree, so its age is unknown.
    pub fn fresh(&self, now_ms: u64) -> bool {
        now_ms >= self.sampled_ms && now_ms - self.sampled_ms <= self.valid_for_ms.min(VALID_LONGEST_MS)
    }
}

/// What the reader may use at `now_ms`: the candidates of a fresh state, or why there are none.
/// `now_ms` is `None` where the helper's clock cannot be read.
pub fn usable(latest: &Result<Status, Unusable>, now_ms: Option<u64>) -> Result<&Status, Unusable> {
    let status = latest.as_ref().map_err(|why| *why)?;
    if now_ms.is_some_and(|now| status.fresh(now)) {
        Ok(status)
    } else {
        Err(Unusable::Stale)
    }
}

/// Read the helper's answer. The control link is the built-in wire the state names; the others
/// are kept only when they are IPv4 addresses on its subnet, which is what the helper's routes
/// were made for.
pub fn parse(body: &[u8]) -> Result<Status, Unusable> {
    let root: Value = serde_json::from_slice(body).map_err(|_| Unusable::Malformed)?;
    if root.get("ok").and_then(Value::as_bool) != Some(true) || root.get("topic").and_then(Value::as_str) != Some(TOPIC) {
        return Err(Unusable::Malformed);
    }
    let value = root.get("value").filter(|v| v.is_object()).ok_or(Unusable::Malformed)?;
    if root.get("api").and_then(Value::as_u64) != Some(1) || value.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(Unusable::Schema);
    }
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).ok_or(Unusable::Malformed);
    let number = |key: &str| value.get(key).and_then(Value::as_u64).ok_or(Unusable::Malformed);
    if !flag("enabled")? {
        return Err(Unusable::Disabled);
    }
    if !flag("active")? {
        return Err(Unusable::Inactive);
    }
    let (boot, sampled_ms, valid_for_ms) = (number("boot")?, number("sampledUptimeMs")?, number("validForMs")?);
    let control = value.get("controlInterface").and_then(Value::as_str).ok_or(Unusable::Malformed)?;
    let listed = value.get("candidates").and_then(Value::as_array).ok_or(Unusable::Malformed)?;
    if listed.len() > CANDIDATES_MOST {
        return Err(Unusable::Malformed);
    }
    let mut candidates: Vec<Candidate> = Vec::with_capacity(listed.len());
    for entry in listed {
        if let Some(candidate) = candidate(entry, control)? {
            if !candidates.iter().any(|c| c.address == candidate.address || c.interface == candidate.interface) {
                candidates.push(candidate);
            }
        }
    }
    let wire = candidates.iter().find(|c| c.control).ok_or(Unusable::Malformed)?;
    if wire.kind != Kind::Ethernet {
        return Err(Unusable::Malformed);
    }
    let (subnet, prefix) = (network(wire.address, wire.prefix), wire.prefix);
    candidates.retain(|c| c.prefix == prefix && network(c.address, prefix) == subnet);
    Ok(Status { boot, sampled_ms, valid_for_ms, candidates })
}

/// One entry of the list: `Ok(None)` for an address this never uses (IPv6, or one no host has).
fn candidate(entry: &Value, control: &str) -> Result<Option<Candidate>, Unusable> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).ok_or(Unusable::Malformed);
    let interface = text("interface")?;
    if interface.is_empty() || interface.len() > 15 || !interface.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Unusable::Malformed);
    }
    let kind = match text("kind")? {
        "ethernet" => Kind::Ethernet,
        "usb-ethernet" => Kind::UsbEthernet,
        "wifi" => Kind::Wifi,
        _ => return Err(Unusable::Malformed),
    };
    let address = match text("address")?.parse::<IpAddr>().map_err(|_| Unusable::Malformed)? {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => return Ok(None),
    };
    let prefix = entry.get("prefix").and_then(Value::as_u64).filter(|p| (1..=30).contains(p)).ok_or(Unusable::Malformed)? as u8;
    let carrier_changes = entry.get("carrierChanges").and_then(Value::as_u64).ok_or(Unusable::Malformed)?;
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() || address.is_broadcast() || address.is_link_local() {
        return Ok(None);
    }
    Ok(Some(Candidate { interface: interface.to_owned(), kind, address, prefix, carrier_changes, control: interface == control }))
}

fn network(address: Ipv4Addr, prefix: u8) -> u32 {
    u32::from(address) & (u32::MAX << (32 - u32::from(prefix)))
}
