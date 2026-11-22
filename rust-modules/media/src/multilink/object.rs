//! **What makes two range answers parts of the same object.**
//!
//! Bytes from different connections are joined only when every answer says the same thing about
//! what it is a part of: `206`, the range that was asked for, the size the default reader opened,
//! a plain sized body, and one validator that never changes (a strong `ETag`, else `Last-Modified`
//! on a Plex part URL, which names the part and its stamp). Any other answer is refused before a
//! byte of it is kept.

/// What an answer to a range request said, as far as joining cares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// `Content-Range: bytes first-last/total`; the total is `None` when the server wrote `*`.
    pub range: Option<(u64, u64, Option<u64>)>,
    pub length: Option<u64>,
    pub etag: Option<String>,
    pub modified: Option<String>,
    /// A `Content-Encoding` other than `identity`: the bytes are not the file's.
    pub encoded: bool,
    /// `Transfer-Encoding: chunked`: a body whose length the server did not commit to.
    pub chunked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Validator {
    Etag(String),
    Modified(String),
}

/// The object every joined range belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    total: u64,
    validator: Validator,
}

/// Why an answer is not a part of the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A `3xx`. Never followed: the request carries the token, and a range from another origin
    /// is another object.
    Redirect,
    /// `200`: the server ignored the range and is sending the file from its head.
    RangeIgnored,
    Status(u16),
    NoContentRange,
    WrongOffset,
    WrongLength,
    WrongTotal,
    Encoded,
    Chunked,
    NoValidator,
    ValidatorChanged,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Redirect => f.write_str("redirected"),
            Refusal::RangeIgnored => f.write_str("range ignored"),
            Refusal::Status(status) => write!(f, "status {status}"),
            Refusal::NoContentRange => f.write_str("no Content-Range"),
            Refusal::WrongOffset => f.write_str("another range than asked"),
            Refusal::WrongLength => f.write_str("another length than asked"),
            Refusal::WrongTotal => f.write_str("another total size"),
            Refusal::Encoded => f.write_str("content-encoded"),
            Refusal::Chunked => f.write_str("chunked"),
            Refusal::NoValidator => f.write_str("no validator"),
            Refusal::ValidatorChanged => f.write_str("validator changed"),
        }
    }
}

impl Head {
    /// What every answer to `first..=last` of an object of `total` bytes must say.
    fn frames(&self, first: u64, last: u64, total: u64) -> Result<(), Refusal> {
        match self.status {
            206 => {}
            200 => return Err(Refusal::RangeIgnored),
            300..=399 => return Err(Refusal::Redirect),
            other => return Err(Refusal::Status(other)),
        }
        let (start, end, of) = self.range.ok_or(Refusal::NoContentRange)?;
        if start != first || end != last {
            return Err(Refusal::WrongOffset);
        }
        if of != Some(total) {
            return Err(Refusal::WrongTotal);
        }
        if self.encoded {
            return Err(Refusal::Encoded);
        }
        if self.chunked {
            return Err(Refusal::Chunked);
        }
        if self.length != Some(last - first + 1) {
            return Err(Refusal::WrongLength);
        }
        Ok(())
    }

    /// A weak `ETag` promises less than byte equality, so only a strong one counts.
    fn strong_etag(&self) -> Option<&str> {
        self.etag.as_deref().filter(|tag| tag.len() > 2 && tag.starts_with('"') && tag.ends_with('"'))
    }

    fn stamp(&self) -> Option<&str> {
        self.modified.as_deref().filter(|stamp| !stamp.is_empty())
    }

    /// Read one line of an answer's head. True when the line ends the head of the answer itself
    /// (a `1xx` is followed by another).
    pub fn line(&mut self, line: &[u8]) -> bool {
        let line = line.trim_ascii();
        if line.is_empty() {
            return !(100..200).contains(&self.status);
        }
        if line.starts_with(b"HTTP/") {
            // Another answer begins: nothing said under the last one holds.
            *self = Head { status: status_of(line), ..Head::default() };
            return false;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else { return false };
        let (name, value) = (&line[..colon], line[colon + 1..].trim_ascii());
        if name.eq_ignore_ascii_case(b"content-range") {
            self.range = range_of(value);
        } else if name.eq_ignore_ascii_case(b"content-length") {
            self.length = number(value);
        } else if name.eq_ignore_ascii_case(b"etag") {
            self.etag = text(value);
        } else if name.eq_ignore_ascii_case(b"last-modified") {
            self.modified = text(value);
        } else if name.eq_ignore_ascii_case(b"content-encoding") {
            self.encoded = !value.eq_ignore_ascii_case(b"identity");
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            self.chunked = !value.eq_ignore_ascii_case(b"identity");
        }
        false
    }
}

impl Identity {
    /// The identity an answer to `first..=last` gives an object the default reader opened at
    /// `total` bytes.
    pub fn of(head: &Head, first: u64, last: u64, total: u64) -> Result<Identity, Refusal> {
        head.frames(first, last, total)?;
        let validator = match (head.strong_etag(), head.stamp()) {
            (Some(tag), _) => Validator::Etag(tag.to_owned()),
            (None, Some(stamp)) => Validator::Modified(stamp.to_owned()),
            (None, None) => return Err(Refusal::NoValidator),
        };
        Ok(Identity { total, validator })
    }

    /// Whether an answer to `first..=last` is a part of this object.
    pub fn accepts(&self, head: &Head, first: u64, last: u64) -> Result<(), Refusal> {
        head.frames(first, last, self.total)?;
        let same = match (&self.validator, head.strong_etag(), head.stamp()) {
            (_, None, None) => return Err(Refusal::NoValidator),
            (Validator::Etag(tag), Some(seen), _) => tag == seen,
            (Validator::Modified(stamp), None, Some(seen)) => stamp == seen,
            _ => false,
        };
        if same {
            Ok(())
        } else {
            Err(Refusal::ValidatorChanged)
        }
    }

    /// What a request sends as `If-Range`, so a server whose object changed answers `200` instead
    /// of a range of the new one. Only an `ETag` is sent: servers compare a date in their own ways.
    pub fn if_range(&self) -> Option<&str> {
        match &self.validator {
            Validator::Etag(tag) => Some(tag),
            Validator::Modified(_) => None,
        }
    }
}

/// `HTTP/1.1 206 Partial Content` is 206; 0 when no three digits stand where the code belongs.
fn status_of(line: &[u8]) -> u16 {
    let code = line.split(|&b| b == b' ').filter(|part| !part.is_empty()).nth(1);
    code.filter(|code| code.len() == 3).and_then(number).map_or(0, |code| code as u16)
}

/// `bytes 4-7/8` is `(4, 7, Some(8))`, and `bytes 4-7/*` leaves the total unsaid.
fn range_of(value: &[u8]) -> Option<(u64, u64, Option<u64>)> {
    let rest = value.get(..6).filter(|unit| unit.eq_ignore_ascii_case(b"bytes "))?;
    let rest = value[rest.len()..].trim_ascii();
    let slash = rest.iter().position(|&b| b == b'/')?;
    let (span, total) = (&rest[..slash], rest[slash + 1..].trim_ascii());
    let dash = span.iter().position(|&b| b == b'-')?;
    let total = if total == b"*" { None } else { Some(number(total)?) };
    Some((number(span[..dash].trim_ascii())?, number(span[dash + 1..].trim_ascii())?, total))
}

/// A decimal number of at most 19 digits, which a `u64` always holds.
fn number(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || digits.len() > 19 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(digits.iter().fold(0, |n, digit| n * 10 + u64::from(digit - b'0')))
}

fn text(value: &[u8]) -> Option<String> {
    std::str::from_utf8(value).ok().filter(|value| !value.is_empty()).map(str::to_owned)
}
