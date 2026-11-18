//! **The exported window's watch** (webOS 5+): is the picture's window still the one the
//! compositor knows?
//!
//! An exported window is an object on the compositor connection that made it. When the television
//! takes the window away and gives it back (a compositor restart looks exactly like that), the
//! connection is new, ids start again from 1, and the old export names nothing, or another
//! export. The pipeline keeps decoding into it: sound plays, frames are counted, and the picture
//! stays black.
//!
//! The pump hands [`ExportWatch::step`] one [`Observation`] per tick. A playback bound to an older
//! connection than the current one, or whose export was refused or never made, is rebuilt with a
//! quiet restart at the playhead ([`Action::Rebind`]), the way the freeze safety net restarts.
//! A refused export is retried once per playback, and a connection is rebuilt for once, so a
//! television that keeps refusing is not restarted in a loop; a newer connection earns a new try.

/// One pump tick, as the watch needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The playback this tick belongs to. A new one forgets the retry.
    pub playback: u32,
    /// The watch applies: an Engine on the exported-window path whose Load has returned, with no
    /// seek, rebuild or end of film in the way. Anything else asks nothing.
    pub covered: bool,
    /// The connection the process is on now.
    pub current: u32,
    /// The connection this Engine's export was made on; 0 when it has none.
    pub bound: u32,
    /// The television refused to place the export.
    pub refused: bool,
}

/// Why the window is rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The export belongs to a connection that is gone.
    Reconnected,
    /// The export was never made or the television refused it.
    Unusable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    /// Reopen at the playhead on a fresh export, keeping the playback's choices.
    Rebind(Why),
}

#[derive(Debug, Default)]
pub struct ExportWatch {
    playback: u32,
    /// This playback has already been rebuilt for an unusable export.
    retried_unusable: bool,
    /// The connection this playback was last rebuilt for, so a rebuild that cannot start is not
    /// asked again every tick.
    rebuilt_for: u32,
}

impl ExportWatch {
    pub fn step(&mut self, obs: Observation) -> Action {
        if obs.playback != self.playback {
            *self = ExportWatch { playback: obs.playback, ..ExportWatch::default() };
        }
        if !obs.covered {
            return Action::None;
        }
        if obs.bound != 0 && obs.bound != obs.current && self.rebuilt_for != obs.current {
            self.rebuilt_for = obs.current;
            return Action::Rebind(Why::Reconnected);
        }
        if (obs.bound == 0 || obs.refused) && !self.retried_unusable {
            self.retried_unusable = true;
            return Action::Rebind(Why::Unusable);
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs() -> Observation {
        Observation { playback: 1, covered: true, current: 3, bound: 3, refused: false }
    }

    #[test]
    fn a_window_on_the_current_connection_is_left_alone() {
        assert_eq!(ExportWatch::default().step(obs()), Action::None);
    }

    #[test]
    fn a_window_from_an_older_connection_is_rebuilt_once_per_connection() {
        let mut w = ExportWatch::default();
        assert_eq!(w.step(Observation { bound: 2, ..obs() }), Action::Rebind(Why::Reconnected));
        assert_eq!(w.step(Observation { bound: 2, ..obs() }), Action::None);
        assert_eq!(w.step(Observation { bound: 2, current: 4, ..obs() }), Action::Rebind(Why::Reconnected));
    }

    #[test]
    fn a_missing_or_refused_window_is_rebuilt_once_per_playback() {
        let mut w = ExportWatch::default();
        assert_eq!(w.step(Observation { bound: 0, ..obs() }), Action::Rebind(Why::Unusable));
        assert_eq!(w.step(Observation { refused: true, ..obs() }), Action::None);
        assert_eq!(w.step(Observation { playback: 2, refused: true, ..obs() }), Action::Rebind(Why::Unusable));
    }

    #[test]
    fn nothing_is_asked_while_the_watch_does_not_apply() {
        let mut w = ExportWatch::default();
        assert_eq!(w.step(Observation { covered: false, bound: 1, refused: true, ..obs() }), Action::None);
    }
}
