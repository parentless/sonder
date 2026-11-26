#![cfg_attr(not(feature = "ui"), allow(dead_code, unused_imports))]

#[cfg(feature = "ui")]
mod app;
mod audio_output;
pub mod backend;
mod credential_worker;
pub mod credits;
#[cfg(target_os = "windows")]
mod diagnostics;
pub(crate) mod discord;
pub mod equalizer;
pub(crate) mod lastfm;
pub mod lifecycle;
mod link_download;
pub mod local_files;
mod local_time;
pub mod lyrics;
mod memory;
pub mod model;
mod musicbrainz;
mod oauth_callback;
mod oauth_page;
pub mod playback;
#[cfg(any(target_os = "windows", test))]
mod rotating_log;
mod similar;
pub mod soloist;
pub mod spotify;
pub mod storage;
mod trash;

#[cfg(feature = "ui")]
pub use app::run;

#[cfg(any(target_os = "windows", test))]
#[cfg(target_os = "windows")]
pub mod startup;
#[cfg(target_os = "windows")]
mod tray;
mod update_release;
