//! **One-shot import of Bazarr's address and key from a file.**
//!
//! The link is normally typed on the television (Settings > Integrations > Bazarr). For a set that is
//! being provisioned from another machine, this is the other door: a JSON file
//! `{"url": "https://…", "api_key": "…"}` at [`IMPORT_FILE`] in the runtime directory
//! (`tools/import-bazarr-link.sh` writes it from a key file without printing the key) is read ONCE, at
//! start-up, held to exactly the rules the typed values are ([`BazarrLink::normalize_url`],
//! [`BazarrLink::normalize_key`]), stored in the same sealed session record, and then deleted.
//!
//! The file is read only if it is a regular file (never a link), owned by this process's user, closed to
//! everyone else (`0600`) and small; anything else is refused and removed. It is kept, unread, while
//! nobody is signed in, so the next start finds it. Nothing here logs an address or a key: the event log
//! says only that a link was imported or refused, and why in a word.

use super::BazarrLink;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// The file's name in the runtime directory (`/tmp` on a television).
pub const IMPORT_FILE: &str = "marquee-bazarr-import.json";
/// A link is an address and a key: nothing legitimate is bigger.
const MAX_BYTES: u64 = 2048;

/// Why a file was not taken. Worded in one word in the event log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    NotAFile,
    NotOurs,
    Open,
    TooBig,
    NotJson,
    Address,
    Key,
}

impl Refusal {
    fn word(self) -> &'static str {
        match self {
            Refusal::NotAFile => "not a regular file",
            Refusal::NotOurs => "not owned by this app, or readable by others",
            Refusal::Open => "unreadable",
            Refusal::TooBig => "too big",
            Refusal::NotJson => "not the expected JSON",
            Refusal::Address => "unusable address",
            Refusal::Key => "unusable key",
        }
    }
}

/// The link in `bytes`: `{"url": …, "api_key": …}`, each normalized as typed values are.
fn parse(bytes: &[u8]) -> Result<BazarrLink, Refusal> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Refusal::NotJson)?;
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).ok_or(Refusal::NotJson);
    let url = BazarrLink::normalize_url(field("url")?).map_err(|_| Refusal::Address)?;
    let api_key = BazarrLink::normalize_key(field("api_key")?).ok_or(Refusal::Key)?;
    Ok(BazarrLink { url, api_key })
}

/// Whether `meta` is a file this process may trust: a regular file, not a link, owned by its user,
/// with no permission bit for group or others.
fn trusted(meta: &std::fs::Metadata, euid: u32) -> Result<(), Refusal> {
    if !meta.file_type().is_file() {
        return Err(Refusal::NotAFile);
    }
    if meta.uid() != euid || meta.mode() & 0o077 != 0 {
        return Err(Refusal::NotOurs);
    }
    if meta.len() > MAX_BYTES {
        return Err(Refusal::TooBig);
    }
    Ok(())
}

/// Read the link out of the file at `path`, with every check above.
fn read(path: &Path) -> Result<BazarrLink, Refusal> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| Refusal::Open)?;
    trusted(&meta, unsafe { libc::geteuid() })?;
    let bytes = std::fs::read(path).map_err(|_| Refusal::Open)?;
    parse(&bytes)
}

const KEEP: u8 = 0;
const APPLIED: u8 = 1;
const REFUSED: u8 = 2;

/// Import the file at [`IMPORT_FILE`], if there is one. Cheap when there is not (one `stat`).
/// Called once, at start-up, off the frame loop's hot path; the read and the write both happen on the
/// storage worker.
pub fn import_once() {
    import_from(plx_base::paths::in_runtime_dir(IMPORT_FILE));
}

fn import_from(path: PathBuf) {
    if std::fs::symlink_metadata(&path).is_err() {
        return;
    }
    let outcome = Arc::new(AtomicU8::new(KEEP));
    let (seen, settled) = (Arc::clone(&outcome), Arc::clone(&outcome));
    let removal = path.clone();
    super::queue_update_settled(
        move |current| match read(&path) {
            Ok(link) => {
                // nobody is signed in: the link has no record to live in. Keep the file for the next start.
                if current.account_token.is_empty() {
                    return None;
                }
                seen.store(APPLIED, Ordering::Release);
                Some(current.with_bazarr(link))
            }
            Err(refusal) => {
                plx_base::eventlog::log(&format!("bazarr: import file refused ({})", refusal.word()));
                seen.store(REFUSED, Ordering::Release);
                None
            }
        },
        move |persisted| {
            let outcome = settled.load(Ordering::Acquire);
            if outcome == REFUSED || (outcome == APPLIED && persisted) {
                let _ = std::fs::remove_file(&removal);
                if outcome == APPLIED {
                    plx_base::eventlog::log("bazarr: link imported from file");
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const GOOD: &str = r#"{"url":"https://bazarr.example.net/","api_key":"0123456789abcdef0123456789abcdef"}"#;

    /// One directory per test, removed when the test process exits (`plx_base::testscratch`).
    fn scratch(name: &str) -> PathBuf {
        plx_base::testscratch::process_dir(&format!("bazarr-import-{name}"))
    }

    fn write(dir: &Path, mode: u32, body: &str) -> PathBuf {
        let path = dir.join(IMPORT_FILE);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn a_private_file_with_a_good_link_is_taken_as_typed_values_are() {
        let dir = scratch("good");
        let link = read(&write(&dir, 0o600, GOOD)).expect("a good file");
        assert_eq!(link.url, "https://bazarr.example.net");
        assert_eq!(link.api_key, "0123456789abcdef0123456789abcdef");
        assert!(link.is_complete());
    }

    #[test]
    fn a_file_others_can_read_or_write_is_refused() {
        let dir = scratch("open");
        for mode in [0o644, 0o640, 0o604, 0o666] {
            assert_eq!(read(&write(&dir, mode, GOOD)).unwrap_err(), Refusal::NotOurs, "mode {mode:o}");
        }
    }

    #[test]
    fn a_link_to_a_file_is_not_a_file() {
        let dir = scratch("link");
        let real = write(&dir, 0o600, GOOD);
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read(&link).unwrap_err(), Refusal::NotAFile);
    }

    #[test]
    fn an_oversized_file_is_refused_unread() {
        let dir = scratch("big");
        let path = write(&dir, 0o600, &format!("{GOOD}{}", " ".repeat(4096)));
        assert_eq!(read(&path).unwrap_err(), Refusal::TooBig);
    }

    #[test]
    fn values_are_held_to_the_rules_typed_ones_are() {
        assert_eq!(parse(b"not json").unwrap_err(), Refusal::NotJson);
        assert_eq!(parse(br#"{"url":"https://b.example.net"}"#).unwrap_err(), Refusal::NotJson);
        assert_eq!(parse(br#"{"url":"ftp://b.example.net","api_key":"0123456789abcdef"}"#).unwrap_err(), Refusal::Address);
        // plain http is for the home network only
        assert_eq!(parse(br#"{"url":"http://bazarr.example.net","api_key":"0123456789abcdef"}"#).unwrap_err(), Refusal::Address);
        assert_eq!(parse(br#"{"url":"https://b.example.net","api_key":"has a space"}"#).unwrap_err(), Refusal::Key);
        assert_eq!(parse(br#"{"url":"https://b.example.net","api_key":""}"#).unwrap_err(), Refusal::Key);
    }

    #[test]
    fn no_refusal_says_what_the_file_held() {
        for refusal in [Refusal::NotAFile, Refusal::NotOurs, Refusal::Open, Refusal::TooBig, Refusal::NotJson, Refusal::Address, Refusal::Key] {
            let word = refusal.word();
            assert!(!word.contains("0123") && !word.contains("example"), "{word}");
        }
    }
}
