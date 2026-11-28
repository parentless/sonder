//! Voice play: hold a shortcut, say a song and artist, let go, and it plays.
//!
//! Everything here is independent of the interface. `src/app/voice_play.rs`
//! connects it to the player and to Settings.

pub(crate) mod capture;
pub(crate) mod hook;
pub(crate) mod hotkey;
pub(crate) mod model;
pub(crate) mod query;
pub(crate) mod sounds;
pub(crate) mod transcribe;
pub(crate) mod worker;

/// Runs the speech-to-text helper if this process was started as one.
/// Must be the first thing `main` does.
pub fn run_worker_if_requested() -> bool {
    worker::run_if_requested()
}

pub(crate) use hotkey::{Hotkey, HotkeyEvent};
pub(crate) use model::Model;

/// What Voice play is set to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Config {
    pub enabled: bool,
    pub hotkey: Hotkey,
    pub model: Model,
    /// Shows the listening animation over whatever is on screen.
    pub animation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: Hotkey::default(),
            model: Model::Standard,
            animation: false,
        }
    }
}
