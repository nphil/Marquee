//! Tests whose premise a root run, or a run after another user's, does not meet.
//!
//! A read-only directory, a mode-000 file or an unlink refused by the parent stops an ordinary user
//! and nobody else: root writes through all three, so the refusal the test provokes never happens
//! and its assertions describe a different situation. Such a test returns early under
//! [`skip_as_root!`].
//!
//! A test that reads back the machine-wide event log (`/tmp/plxnative-events.log` in a host run)
//! needs this uid to own it: [`crate::eventlog::open_log_append`] refuses a file another uid owns,
//! so after one user's run every line another user's run logs is dropped, a `contains` fails and a
//! `!contains` passes without proving anything. Such a test returns early under
//! [`skip_unless_log_is_ours!`].
//!
//! Both print the test and the reason on stderr, so a root run is green and says what it did not
//! check; CI runs every test as one ordinary user.

/// Whether this process is the superuser.
pub fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The owner of the log at `path` when it is not this process's uid: a log this process cannot
/// append to. Creates the log if it is missing, as the first logged line would.
pub fn foreign_log_owner(path: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    match crate::eventlog::open_log_append(path) {
        Ok(_) => None,
        Err(_) => std::fs::symlink_metadata(path).ok().map(|m| m.uid()),
    }
}

/// Says which test skipped and why. Written to the stderr handle rather than through `eprintln!`,
/// which libtest captures and discards for a passing test.
pub fn report_skip(module: &str, reason: &str) {
    use std::io::Write;
    let thread = std::thread::current();
    let test = thread.name().unwrap_or(module);
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {reason}");
}

/// Return from the test when running as root, saying why. Put it first in the test, before any
/// fixture is built: `skip_as_root!("needs a read-only directory to refuse the write");`.
#[macro_export]
macro_rules! skip_as_root {
    ($reason:expr) => {
        if $crate::testroot::running_as_root() {
            $crate::testroot::report_skip(module_path!(), &format!("as root: {}", $reason));
            return;
        }
    };
}

/// Return from the test when the event log at `$path` belongs to another uid, so the lines the
/// test reads back would never be written.
#[macro_export]
macro_rules! skip_unless_log_is_ours {
    ($path:expr) => {
        let path: std::path::PathBuf = $path.into();
        if let Some(owner) = $crate::testroot::foreign_log_owner(&path) {
            $crate::testroot::report_skip(
                module_path!(),
                &format!("{} belongs to uid {owner}, so the lines this test reads back are never logged", path.display()),
            );
            return;
        }
    };
}
