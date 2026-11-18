//! Cold-start milestones: one `startup: <stage> +<ms>ms` event-log line the first time each stage
//! is reached, counted from the moment the process was exec'd.
//!
//! The event log has no timestamps, so "how long until the picker shows?" could only be read off a
//! stopwatch. These lines make a cold start a number that can be compared between builds. The
//! first stamp also carries the process's age when the app's own code began (dynamic linking and
//! the shim's `main`), taken from the kernel's start time, so `+ms` is wall time since launch.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::LazyLock;
use std::time::Instant;

/// A point on the way from launch to a usable screen. Each is logged once.
#[derive(Clone, Copy)]
pub enum Stage {
    /// `enter_application`: the app's own code has started.
    Enter,
    /// The stored session record is read (it decides the window size and the first screen).
    Session,
    /// The window and its GL context exist.
    Window,
    /// Shaders and GL state are built.
    Gl,
    /// The font chain is loaded.
    Text,
    /// Image pipelines and the blur passes are built.
    Image,
    /// The HTTP stack is bound.
    Net,
    /// The poster workers are running.
    Posters,
    /// The boot gate has chosen the first screen (picker, Home or sign-in).
    Gate,
    /// The first screen of the launch finished a prepared, drawn frame.
    FirstFrame,
    /// The who's-watching picker's first drawn frame.
    Profiles,
    /// The server is installed for the picked profile.
    Server,
    /// Home's first drawn frame.
    Home,
    /// The first poster or avatar texture reached the GPU (the picker's pictures, or Home's first card).
    Poster,
    /// The stored profile pictures were read, on their own thread.
    Avatars,
}

impl Stage {
    const fn name(self) -> &'static str {
        match self {
            Stage::Enter => "enter",
            Stage::Session => "session",
            Stage::Window => "window",
            Stage::Gl => "gl",
            Stage::Text => "text",
            Stage::Image => "image",
            Stage::Net => "net",
            Stage::Posters => "posters",
            Stage::Gate => "gate",
            Stage::FirstFrame => "first-frame",
            Stage::Profiles => "profiles",
            Stage::Server => "server",
            Stage::Home => "home",
            Stage::Poster => "first-poster",
            Stage::Avatars => "avatars",
        }
    }
}

/// When the clock started: an `Instant`, and how old the process already was at that moment.
static ORIGIN: LazyLock<(Instant, u64)> = LazyLock::new(|| (Instant::now(), process_age_ms().unwrap_or(0)));
static SEEN: AtomicU32 = AtomicU32::new(0);

/// Log `stage` with the milliseconds since launch, unless it was already logged.
pub fn mark(stage: Stage) {
    mark_with(stage, "");
}

/// [`mark`] with a short detail after the time (the screen a first frame belongs to).
pub fn mark_with(stage: Stage, detail: &str) {
    let bit = 1u32 << stage as u32;
    if SEEN.fetch_or(bit, Relaxed) & bit != 0 {
        return;
    }
    let (at, age) = &*ORIGIN;
    let ms = age + at.elapsed().as_millis() as u64;
    let sep = if detail.is_empty() { "" } else { " " };
    crate::eventlog::log(&format!("startup: {} +{ms}ms{sep}{detail}", stage.name()));
}

/// Feed a `coldopen screen=<name> …` line: the first one of the launch is [`Stage::FirstFrame`],
/// and the picker's and Home's own first frames get their stages.
pub fn note_screen(line: &str) {
    let Some(name) = screen_of(line) else { return };
    mark_with(Stage::FirstFrame, name);
    match name {
        "profiles" => mark(Stage::Profiles),
        "home" => mark(Stage::Home),
        _ => {}
    }
}

fn screen_of(line: &str) -> Option<&str> {
    line.strip_prefix("coldopen screen=")?.split_whitespace().next()
}

/// How long ago the process started, from `/proc/self/stat` (field 22, the start time in clock
/// ticks since boot) and `/proc/uptime`. Resolution is the kernel's tick, 10 ms.
fn process_age_ms() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    age_from(&stat, &uptime)
}

/// `USER_HZ`: the unit `/proc/<pid>/stat` counts in, fixed at 100 on every Linux the app runs on.
const TICKS_PER_SECOND: u64 = 100;

fn age_from(stat: &str, uptime: &str) -> Option<u64> {
    // The command name is parenthesised and may hold spaces; the numbered fields follow the last `)`.
    // Field 3 (the state) is the first of them, so field 22 is the 20th.
    let started: u64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let up_ms = {
        let (secs, hundredths) = uptime.split_whitespace().next()?.split_once('.')?;
        secs.parse::<u64>().ok()? * 1000 + hundredths.get(..2)?.parse::<u64>().ok()? * 10
    };
    up_ms.checked_sub(started * 1000 / TICKS_PER_SECOND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_is_uptime_minus_the_start_tick() {
        // A command name with spaces and a parenthesis must not shift the fields.
        let stat = "412 (plx (native)) S 1 412 412 0 -1 4194560 3000 0 0 0 20 5 0 0 20 0 14 0 123456 99999999 5000 18446744073709551615";
        assert_eq!(age_from(stat, "1240.00 4000.00"), Some(5_440));
    }

    #[test]
    fn a_start_in_the_future_or_a_short_record_is_no_age() {
        let stat = "1 (x) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 999999999 1 1";
        assert_eq!(age_from(stat, "10.00 5.00"), None);
        assert_eq!(age_from("1 (x) S 1 1", "10.00 5.00"), None);
    }

    #[test]
    fn only_a_coldopen_line_names_a_screen() {
        assert_eq!(screen_of("coldopen screen=profiles ms=0 prepared=true"), Some("profiles"));
        assert_eq!(screen_of("auth: roster refreshed n=3"), None);
    }
}
