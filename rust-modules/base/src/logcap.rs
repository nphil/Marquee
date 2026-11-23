//! Size caps for the runtime logs that grow while the app runs (the event log and the stderr
//! capture). `src/main.c` keeps the previous run's copy at launch; this keeps a long session
//! bounded: past the cap the whole file moves to the `.prev` twin and the live file is emptied
//! IN PLACE.
//!
//! In place, not by rename, because several descriptors hold the live inode: the C event stream and
//! the crash tracer's duplicate (both `O_APPEND`), and fd 2 for stderr. A rename would leave them
//! writing into the `.prev` file. Every holder appends, so after the truncate their next write lands
//! at the new end. Lines appended between the copy and the truncate are lost, a window of a few
//! milliseconds once per cap.

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::Duration;

use crate::paths::runtime_file as file;

/// Event log: about two hours of a busy session keeps well under this.
const EVENTS_MAX: u64 = 8 << 20;
/// What the TV's libraries print: GL info logs, aborts, panics.
const STDERR_MAX: u64 = 2 << 20;
const TICK: Duration = Duration::from_secs(30);

/// Move `path` into `prev` and empty `path` in place when it is over `max` bytes. `Ok(true)` when it
/// rotated. Both files go through [`crate::eventlog::open_log_append`]'s checks (no symlink, ours,
/// one link). A failed copy leaves `path` whole.
pub fn rotate_in_place(path: &Path, prev: &Path, max: u64) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && m.len() > max => {}
        _ => return Ok(false),
    }
    let live = crate::eventlog::open_log_append(path)?;
    let live_meta = live.metadata()?;
    if live_meta.len() <= max {
        return Ok(false);
    }
    let src = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    let src_meta = src.metadata()?;
    if (src_meta.dev(), src_meta.ino()) != (live_meta.dev(), live_meta.ino()) {
        return Err(io::Error::other("log replaced while rotating"));
    }
    let dst = crate::eventlog::open_log_append(prev)?;
    dst.set_len(0)?;
    io::copy(&mut src.take(live_meta.len()), &mut &dst)?;
    live.set_len(0)?;
    Ok(true)
}

fn tick(dir: &Path) {
    for (name, prev, max) in [
        (file::EVENTS, file::EVENTS_PREV, EVENTS_MAX),
        (file::STDERR, file::STDERR_PREV, STDERR_MAX),
    ] {
        match rotate_in_place(&dir.join(name), &dir.join(prev), max) {
            Ok(true) => crate::eventlog::log(&format!("logcap: {name} passed {max} bytes; the old part is in {prev}")),
            Ok(false) => {}
            Err(e) => crate::eventlog::log(&format!("logcap: {name} not rotated: {e}")),
        }
    }
}

/// Start the keeper thread, once, at boot. It only wakes every [`TICK`] and stats two files.
pub fn start() {
    let dir = crate::paths::runtime_dir();
    crate::task::spawn("log caps", move || loop {
        std::thread::sleep(TICK);
        tick(dir);
    });
}

#[cfg(test)]
mod tests {
    use super::rotate_in_place;
    use std::io::Write;

    #[test]
    fn an_oversize_log_moves_to_prev_and_open_appenders_follow_the_new_end() {
        let dir = crate::testscratch::process_dir("logcap");
        let (live, prev) = (dir.join("a.log"), dir.join("a.prev.log"));
        let mut appender = crate::eventlog::open_log_append(&live).unwrap();
        appender.write_all(b"0123456789").unwrap();

        assert!(!rotate_in_place(&live, &prev, 10).unwrap(), "at the cap is not over it");
        assert!(!prev.exists());

        assert!(rotate_in_place(&live, &prev, 9).unwrap());
        assert_eq!(std::fs::read(&prev).unwrap(), b"0123456789");
        assert_eq!(std::fs::metadata(&live).unwrap().len(), 0);

        appender.write_all(b"new").unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new", "an old descriptor writes at the new end, no hole");

        appender.write_all(b"0123456789").unwrap();
        assert!(rotate_in_place(&live, &prev, 9).unwrap());
        assert_eq!(std::fs::read(&prev).unwrap(), b"new0123456789", "the twin keeps one rotation, not two");
    }

    #[test]
    fn a_missing_or_symlinked_log_is_left_alone() {
        let dir = crate::testscratch::process_dir("logcap-link");
        let (live, prev, target) = (dir.join("b.log"), dir.join("b.prev.log"), dir.join("target"));
        assert!(!rotate_in_place(&live, &prev, 0).unwrap());
        std::fs::write(&target, b"data").unwrap();
        std::os::unix::fs::symlink(&target, &live).unwrap();
        assert!(!rotate_in_place(&live, &prev, 0).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"data");
    }
}
