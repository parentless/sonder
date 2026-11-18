//! Turns Soloist's reports into what the rest of Sonder expects from a player:
//! a song started loading, began playing, paused, reached its end or could not
//! play. Soloist plays one song for each `play`, so it also decides when a song
//! has finished, and is careful about the reports that arrive late for the song
//! before. It holds no clock and no socket: callers pass the time in and send
//! the commands it returns, so every rule can be tested.

use serde_json::Value;

use super::protocol::{self, Anchor, Status, Update};

/// A song that has not started this long after it was asked for has failed.
const START_TIMEOUT_MS: u64 = 20_000;
/// Within this much of its length, a song that stops has finished rather than
/// been stopped.
const END_MARGIN_MS: u64 = 4_000;
/// A song still reported as playing this long past its length has finished and
/// the report was missed.
const OVERRUN_MS: u64 = 6_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    Loading { uri: String },
    Playing { uri: String, position_ms: u32 },
    Paused { uri: String, position_ms: u32 },
    Position { uri: String, position_ms: u32 },
    EndOfTrack { uri: String },
    Unavailable { uri: String, reason: &'static str },
}

/// What a call produced: things to tell the app, and commands for Soloist.
#[derive(Debug, Default, PartialEq)]
pub struct Out {
    pub signals: Vec<Signal>,
    pub commands: Vec<Value>,
}

struct Loaded {
    uri: String,
    status: Status,
    /// Soloist has reported this song playing at least once.
    started: bool,
    /// Loaded paused: Soloist is told nothing until the listener plays, since
    /// it has no way to open a song silently.
    held: bool,
    asked_at: u64,
    duration_ms: Option<u64>,
    anchor: Option<Anchor>,
    /// The furthest point seen, kept because the report that ends a song may
    /// carry a position of zero.
    last_position_ms: u64,
    /// Where to seek once the song is playing.
    start_at_ms: u64,
}

impl Loaded {
    fn position(&self, now: u64) -> u64 {
        let position = match (self.status, self.anchor) {
            (Status::Playing, Some(anchor)) => anchor.position_at(now),
            (_, Some(anchor)) => anchor.position_ms,
            (_, None) => self.last_position_ms,
        };
        self.duration_ms.map_or(position, |end| position.min(end))
    }

    fn observe(&mut self, now: u64) {
        self.last_position_ms = self.last_position_ms.max(self.position(now));
    }

    /// Whether stopping here means the song ran out.
    fn ran_out(&self) -> bool {
        self.duration_ms
            .is_none_or(|end| self.last_position_ms + END_MARGIN_MS >= end)
    }
}

#[derive(Default)]
pub struct Tracker {
    loaded: Option<Loaded>,
}

fn ms(position: u64) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}

impl Tracker {
    #[cfg(test)]
    pub fn loaded_uri(&self) -> Option<&str> {
        self.loaded.as_ref().map(|loaded| loaded.uri.as_str())
    }

    pub fn load(&mut self, uri: &str, playing: bool, position_ms: u32, now: u64) -> Out {
        let mut out = Out::default();
        out.signals.push(Signal::Loading { uri: uri.into() });
        self.loaded = Some(Loaded {
            uri: uri.into(),
            status: Status::Buffering,
            started: false,
            held: !playing,
            asked_at: now,
            duration_ms: None,
            anchor: Some(Anchor {
                position_ms: u64::from(position_ms),
                timestamp_ms: now,
                speed: 0.0,
            }),
            last_position_ms: u64::from(position_ms),
            start_at_ms: u64::from(position_ms),
        });
        if playing {
            out.commands.push(protocol::play(Some(uri)));
        } else {
            out.signals.push(Signal::Paused {
                uri: uri.into(),
                position_ms,
            });
        }
        out
    }

    /// Plays, opening the held song first if the listener loaded it paused.
    pub fn play(&mut self, now: u64) -> Out {
        let mut out = Out::default();
        match self.loaded.as_mut() {
            Some(loaded) if loaded.held => {
                loaded.held = false;
                loaded.asked_at = now;
                out.signals.push(Signal::Loading {
                    uri: loaded.uri.clone(),
                });
                out.commands.push(protocol::play(Some(&loaded.uri)));
            }
            _ => out.commands.push(protocol::play(None)),
        }
        out
    }

    pub fn pause(&mut self) -> Out {
        let mut out = Out::default();
        if self.loaded.as_ref().is_none_or(|loaded| !loaded.held) {
            out.commands.push(protocol::command("pause"));
        }
        out
    }

    pub fn seek(&mut self, position_ms: u32, now: u64) -> Out {
        let mut out = Out::default();
        let Some(loaded) = self.loaded.as_mut() else {
            return out;
        };
        let position = u64::from(position_ms);
        loaded.last_position_ms = position;
        loaded.start_at_ms = position;
        loaded.anchor = Some(Anchor {
            position_ms: position,
            timestamp_ms: now,
            speed: if loaded.status == Status::Playing {
                1.0
            } else {
                0.0
            },
        });
        out.signals.push(Signal::Position {
            uri: loaded.uri.clone(),
            position_ms,
        });
        if !loaded.held {
            out.commands.push(protocol::seek(position));
        }
        out
    }

    /// Stops the song and forgets it.
    pub fn stop(&mut self) -> Out {
        let mut out = Out::default();
        if self.loaded.take().is_some_and(|loaded| !loaded.held) {
            out.commands.push(protocol::command("pause"));
        }
        out
    }

    pub fn on_update(&mut self, update: &Update, now: u64) -> Out {
        let mut out = Out::default();
        let Some(loaded) = self.loaded.as_mut() else {
            return out;
        };
        loaded.observe(now);
        if update.uri.as_ref().is_some_and(|uri| *uri != loaded.uri) {
            // Another song is playing. If ours just ran out this is what
            // follows it; if not, it is the song before ours still winding
            // down, or another device taking over, and is none of our business.
            if loaded.started && !loaded.held && loaded.ran_out() {
                let uri = loaded.uri.clone();
                self.loaded = None;
                out.signals.push(Signal::EndOfTrack { uri });
            }
            return out;
        }
        if loaded.held {
            return out;
        }
        let previous = loaded.status;
        // Where the song was before this report, while it still counts as it
        // was playing: a pause report may not say.
        let before = loaded.position(now);
        if let Some(duration) = update.duration_ms {
            loaded.duration_ms = Some(duration);
        }
        let anchor_moved = update.anchor.is_some();
        if let Some(anchor) = update.anchor {
            loaded.anchor = Some(anchor);
        }
        let uri = loaded.uri.clone();
        let mut status_changed = false;
        match update.status {
            Some(Status::Playing) => {
                if !loaded.started {
                    loaded.started = true;
                    if loaded.start_at_ms > 0 {
                        out.commands.push(protocol::seek(loaded.start_at_ms));
                        loaded.anchor = Some(Anchor {
                            position_ms: loaded.start_at_ms,
                            timestamp_ms: now,
                            speed: 1.0,
                        });
                        loaded.start_at_ms = 0;
                    }
                }
                loaded.status = Status::Playing;
                if previous != Status::Playing {
                    status_changed = true;
                    out.signals.push(Signal::Playing {
                        uri: uri.clone(),
                        position_ms: ms(loaded.position(now)),
                    });
                }
            }
            Some(Status::Paused) => {
                loaded.status = Status::Paused;
                if loaded.started && previous != Status::Paused {
                    status_changed = true;
                    let position = update.anchor.map_or(before, |anchor| anchor.position_ms);
                    out.signals.push(Signal::Paused {
                        uri: uri.clone(),
                        position_ms: ms(position),
                    });
                }
            }
            Some(Status::Buffering) => loaded.status = Status::Buffering,
            Some(Status::Idle) if loaded.started => {
                status_changed = true;
                if loaded.ran_out() {
                    self.loaded = None;
                    out.signals.push(Signal::EndOfTrack { uri });
                    return out;
                }
                loaded.status = Status::Idle;
                if previous != Status::Idle {
                    out.signals.push(Signal::Paused {
                        uri: uri.clone(),
                        position_ms: ms(loaded.last_position_ms),
                    });
                }
            }
            // Idle before our song ever played is the song before it.
            Some(Status::Idle) | None => {}
        }
        if anchor_moved && !status_changed && loaded.started {
            out.signals.push(Signal::Position {
                uri,
                position_ms: ms(loaded.position(now)),
            });
        }
        out
    }

    /// Soloist refused a command. If it was the one opening our song, the song
    /// cannot play.
    pub fn on_error(&mut self) -> Out {
        let mut out = Out::default();
        if self
            .loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.started && !loaded.held)
            && let Some(loaded) = self.loaded.take()
        {
            out.signals.push(Signal::Unavailable {
                uri: loaded.uri,
                reason: "Spotify’s player couldn’t play this song",
            });
        }
        out
    }

    /// Called a couple of times a second: keeps the clock moving, gives up on a
    /// song that never started, and catches a finish that went unreported.
    pub fn tick(&mut self, now: u64) -> Out {
        let mut out = Out::default();
        let Some(loaded) = self.loaded.as_mut() else {
            return out;
        };
        if loaded.held {
            return out;
        }
        loaded.observe(now);
        if !loaded.started {
            if now.saturating_sub(loaded.asked_at) > START_TIMEOUT_MS {
                let uri = loaded.uri.clone();
                self.loaded = None;
                out.signals.push(Signal::Unavailable {
                    uri,
                    reason: "Spotify’s player didn’t start this song",
                });
            }
            return out;
        }
        if loaded.status != Status::Playing {
            return out;
        }
        let position = loaded.position(now);
        if loaded.duration_ms.is_some_and(|end| {
            loaded
                .anchor
                .is_some_and(|anchor| anchor.position_at(now) > end + OVERRUN_MS)
        }) {
            let uri = loaded.uri.clone();
            self.loaded = None;
            out.signals.push(Signal::EndOfTrack { uri });
            return out;
        }
        out.signals.push(Signal::Position {
            uri: loaded.uri.clone(),
            position_ms: ms(position),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "spotify:track:aaaa";
    const B: &str = "spotify:track:bbbb";

    fn playing(uri: &str, duration_ms: u64, position_ms: u64, at: u64) -> Update {
        Update {
            uri: Some(uri.into()),
            status: Some(Status::Playing),
            duration_ms: Some(duration_ms),
            anchor: Some(Anchor {
                position_ms,
                timestamp_ms: at,
                speed: 1.0,
            }),
        }
    }

    fn status(status: Status) -> Update {
        Update {
            status: Some(status),
            ..Update::default()
        }
    }

    fn started(tracker: &mut Tracker, uri: &str, duration_ms: u64) {
        tracker.load(uri, true, 0, 0);
        tracker.on_update(&playing(uri, duration_ms, 0, 0), 0);
    }

    #[test]
    fn loading_a_song_opens_it_and_reports_it_loading() {
        let mut tracker = Tracker::default();
        let out = tracker.load(A, true, 0, 1_000);
        assert_eq!(out.signals, [Signal::Loading { uri: A.into() }]);
        assert_eq!(out.commands, [protocol::play(Some(A))]);
        assert_eq!(tracker.loaded_uri(), Some(A));
    }

    #[test]
    fn a_song_loaded_paused_says_nothing_to_soloist_until_it_is_played() {
        let mut tracker = Tracker::default();
        let out = tracker.load(A, false, 42_000, 0);
        assert_eq!(
            out.signals,
            [
                Signal::Loading { uri: A.into() },
                Signal::Paused {
                    uri: A.into(),
                    position_ms: 42_000
                }
            ]
        );
        assert!(out.commands.is_empty());
        assert!(tracker.pause().commands.is_empty());
        let seeked = tracker.seek(50_000, 10);
        assert!(seeked.commands.is_empty());
        assert_eq!(
            seeked.signals,
            [Signal::Position {
                uri: A.into(),
                position_ms: 50_000
            }]
        );
        let out = tracker.play(20);
        assert_eq!(out.commands, [protocol::play(Some(A))]);
        // Once it plays, it opens where the listener left it.
        let out = tracker.on_update(&playing(A, 200_000, 0, 30), 30);
        assert_eq!(out.commands, [protocol::seek(50_000)]);
    }

    #[test]
    fn playing_resumes_a_song_that_is_already_open() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        assert_eq!(tracker.play(5).commands, [protocol::play(None)]);
        assert_eq!(tracker.pause().commands, [protocol::command("pause")]);
    }

    #[test]
    fn the_first_playing_report_starts_the_song() {
        let mut tracker = Tracker::default();
        tracker.load(A, true, 0, 0);
        let out = tracker.on_update(&playing(A, 200_000, 0, 100), 100);
        assert_eq!(
            out.signals,
            [Signal::Playing {
                uri: A.into(),
                position_ms: 0
            }]
        );
        assert!(out.commands.is_empty());
    }

    #[test]
    fn a_pause_and_a_resume_are_reported_once_each() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        let out = tracker.on_update(&status(Status::Paused), 5_000);
        assert_eq!(
            out.signals,
            [Signal::Paused {
                uri: A.into(),
                position_ms: 5_000
            }]
        );
        assert!(
            tracker
                .on_update(&status(Status::Paused), 5_100)
                .signals
                .is_empty()
        );
        let out = tracker.on_update(&status(Status::Playing), 6_000);
        assert!(matches!(out.signals[0], Signal::Playing { .. }));
    }

    #[test]
    fn a_song_that_stops_near_its_end_has_finished() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(199_500);
        let out = tracker.on_update(&status(Status::Idle), 200_000);
        assert_eq!(out.signals, [Signal::EndOfTrack { uri: A.into() }]);
        assert_eq!(tracker.loaded_uri(), None);
    }

    #[test]
    fn the_end_is_found_even_when_the_last_report_says_position_zero() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(199_000);
        let idle = Update {
            status: Some(Status::Idle),
            anchor: Some(Anchor {
                position_ms: 0,
                timestamp_ms: 200_000,
                speed: 0.0,
            }),
            ..Update::default()
        };
        assert_eq!(
            tracker.on_update(&idle, 200_000).signals,
            [Signal::EndOfTrack { uri: A.into() }]
        );
    }

    #[test]
    fn stopping_far_from_the_end_is_only_a_pause() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(30_000);
        let out = tracker.on_update(&status(Status::Idle), 30_000);
        assert_eq!(
            out.signals,
            [Signal::Paused {
                uri: A.into(),
                position_ms: 30_000
            }]
        );
        assert_eq!(tracker.loaded_uri(), Some(A));
    }

    #[test]
    fn another_song_taking_over_at_the_end_ends_ours() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(199_800);
        let autoplay = playing(B, 180_000, 0, 200_000);
        let out = tracker.on_update(&autoplay, 200_000);
        assert_eq!(out.signals, [Signal::EndOfTrack { uri: A.into() }]);
        assert_eq!(tracker.loaded_uri(), None);
    }

    #[test]
    fn another_song_appearing_mid_song_is_ignored() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(60_000);
        let out = tracker.on_update(&playing(B, 180_000, 0, 60_000), 60_000);
        assert!(out.signals.is_empty());
        assert_eq!(tracker.loaded_uri(), Some(A));
    }

    #[test]
    fn the_song_before_winding_down_does_not_end_the_new_one() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        tracker.tick(10_000);
        // The listener skips to B while A is still reported.
        tracker.load(B, true, 0, 10_100);
        let stale = Update {
            uri: Some(A.into()),
            status: Some(Status::Idle),
            ..Update::default()
        };
        assert!(tracker.on_update(&stale, 10_200).signals.is_empty());
        // And a bare idle before B has played is the same stale report.
        assert!(
            tracker
                .on_update(&status(Status::Idle), 10_300)
                .signals
                .is_empty()
        );
        assert_eq!(tracker.loaded_uri(), Some(B));
    }

    #[test]
    fn a_song_that_never_starts_fails_after_a_while() {
        let mut tracker = Tracker::default();
        tracker.load(A, true, 0, 0);
        assert!(tracker.tick(5_000).signals.is_empty());
        let out = tracker.tick(21_000);
        assert!(matches!(
            out.signals[..],
            [Signal::Unavailable { ref uri, .. }] if uri == A
        ));
        assert_eq!(tracker.loaded_uri(), None);
    }

    #[test]
    fn an_error_while_opening_a_song_fails_it_at_once() {
        let mut tracker = Tracker::default();
        tracker.load(A, true, 0, 0);
        let out = tracker.on_error();
        assert!(matches!(
            out.signals[..],
            [Signal::Unavailable { ref uri, .. }] if uri == A
        ));
        // An error with nothing opening is not about any song.
        assert!(tracker.on_error().signals.is_empty());
    }

    #[test]
    fn an_error_during_a_song_does_not_end_it() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        assert!(tracker.on_error().signals.is_empty());
        assert_eq!(tracker.loaded_uri(), Some(A));
    }

    #[test]
    fn the_clock_ticks_only_while_playing() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        assert_eq!(
            tracker.tick(3_000).signals,
            [Signal::Position {
                uri: A.into(),
                position_ms: 3_000
            }]
        );
        tracker.on_update(&status(Status::Paused), 3_500);
        assert!(tracker.tick(4_000).signals.is_empty());
    }

    #[test]
    fn a_finish_that_went_unreported_is_still_found() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        let out = tracker.tick(207_000);
        assert_eq!(out.signals, [Signal::EndOfTrack { uri: A.into() }]);
        assert_eq!(tracker.loaded_uri(), None);
    }

    #[test]
    fn a_position_report_moves_the_clock() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        let sync = Update {
            anchor: Some(Anchor {
                position_ms: 90_000,
                timestamp_ms: 20_000,
                speed: 1.0,
            }),
            ..Update::default()
        };
        assert_eq!(
            tracker.on_update(&sync, 20_000).signals,
            [Signal::Position {
                uri: A.into(),
                position_ms: 90_000
            }]
        );
    }

    #[test]
    fn seeking_moves_the_clock_and_tells_soloist() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        let out = tracker.seek(120_000, 1_000);
        assert_eq!(out.commands, [protocol::seek(120_000)]);
        assert_eq!(
            tracker.tick(2_000).signals,
            [Signal::Position {
                uri: A.into(),
                position_ms: 121_000
            }]
        );
    }

    #[test]
    fn stopping_pauses_soloist_and_forgets_the_song() {
        let mut tracker = Tracker::default();
        started(&mut tracker, A, 200_000);
        assert_eq!(tracker.stop().commands, [protocol::command("pause")]);
        assert_eq!(tracker.loaded_uri(), None);
        assert!(tracker.stop().commands.is_empty());
    }
}
