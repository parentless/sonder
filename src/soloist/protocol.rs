//! What Soloist's local WebSocket says and understands. Spotify documents it at
//! developer.spotify.com/documentation/soloist/reference/websocket-api: JSON
//! text frames, `{"type":"command",...}` going in and typed events coming out.
//! Everything here is plain data so it can be tested without a player.

use serde_json::{Value, json};

/// Where Soloist is in a song.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Playing,
    Paused,
    Buffering,
}

impl Status {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "idle" => Some(Self::Idle),
            "playing" => Some(Self::Playing),
            "paused" => Some(Self::Paused),
            "buffering" => Some(Self::Buffering),
            _ => None,
        }
    }
}

/// A moment in a song and the clock it was true at. Soloist sends this rather
/// than a tick every few hundred milliseconds, so the position now is worked
/// out from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub position_ms: u64,
    /// Unix epoch, milliseconds.
    pub timestamp_ms: u64,
    pub speed: f64,
}

impl Anchor {
    /// Where the song is at `now_ms`, if it has been playing since the anchor.
    pub fn position_at(&self, now_ms: u64) -> u64 {
        let elapsed = now_ms.saturating_sub(self.timestamp_ms) as f64;
        self.position_ms + (elapsed * self.speed.max(0.0)) as u64
    }
}

/// What one message changes. A full snapshot sets everything it carries; the
/// small events set one field each. Anything a message leaves out stays as it
/// was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Update {
    pub uri: Option<String>,
    pub status: Option<Status>,
    pub duration_ms: Option<u64>,
    pub anchor: Option<Anchor>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Whether Soloist has a Spotify login, and whether it is the active
    /// Connect device.
    Auth { logged_in: bool, is_active: bool },
    /// Something about the song changed.
    Playback(Update),
    /// Soloist refused a command; goes only to the sender.
    Error(String),
    /// Anything else Soloist reports: volume, queue, options.
    Other,
}

/// Reads one text frame. A message that is not JSON, or has a type this
/// version doesn't know, is `Other` so a newer Soloist never breaks playback.
pub fn parse_event(text: &str) -> Event {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Event::Other;
    };
    match message.get("type").and_then(Value::as_str) {
        Some("auth_state") => Event::Auth {
            logged_in: message
                .get("logged_in")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            is_active: message
                .get("is_active")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        Some("error") => Event::Error(
            message
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Soloist reported an error")
                .to_owned(),
        ),
        Some("playback_state") => Event::Playback(Update {
            uri: item_uri(&message),
            status: status_of(&message),
            duration_ms: item_duration(&message),
            anchor: anchor_of(message.get("position")),
        }),
        Some("track_changed") => Event::Playback(Update {
            uri: item_uri(&message),
            duration_ms: item_duration(&message),
            ..Update::default()
        }),
        Some("playback_changed") => Event::Playback(Update {
            status: status_of(&message),
            ..Update::default()
        }),
        Some("position_sync") => Event::Playback(Update {
            anchor: anchor_of(message.get("position")),
            ..Update::default()
        }),
        _ => Event::Other,
    }
}

fn status_of(message: &Value) -> Option<Status> {
    message
        .get("status")
        .and_then(Value::as_str)
        .and_then(Status::parse)
}

fn item_uri(message: &Value) -> Option<String> {
    message.get("item")?.get("uri")?.as_str().map(str::to_owned)
}

fn item_duration(message: &Value) -> Option<u64> {
    message
        .get("item")?
        .get("decorations")?
        .get("playback")?
        .get("duration_ms")?
        .as_u64()
}

fn anchor_of(position: Option<&Value>) -> Option<Anchor> {
    let position = position?;
    Some(Anchor {
        position_ms: position.get("position_ms")?.as_u64()?,
        timestamp_ms: position.get("timestamp_ms")?.as_u64()?,
        speed: position.get("speed").and_then(Value::as_f64).unwrap_or(1.0),
    })
}

/// A command with no arguments, such as `pause`.
pub fn command(name: &str) -> Value {
    json!({ "type": "command", "command": name })
}

/// `play` with a song, or with nothing to carry on where it paused.
pub fn play(uri: Option<&str>) -> Value {
    match uri {
        Some(uri) => json!({ "type": "command", "command": "play", "uri": uri }),
        None => command("play"),
    }
}

pub fn seek(position_ms: u64) -> Value {
    json!({ "type": "command", "command": "seek", "position_ms": position_ms })
}

/// Soloist's volume is 0 to 100.
pub fn set_volume(volume: f32) -> Value {
    let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
    json!({ "type": "command", "command": "set_volume", "volume": percent })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_message_says_whether_soloist_is_signed_in() {
        assert_eq!(
            parse_event(
                r#"{"type":"auth_state","logged_in":false,"is_active":false,"device_name":"Sonder"}"#
            ),
            Event::Auth {
                logged_in: false,
                is_active: false
            }
        );
        assert_eq!(
            parse_event(r#"{"type":"auth_state","logged_in":true,"is_active":true}"#),
            Event::Auth {
                logged_in: true,
                is_active: true
            }
        );
    }

    #[test]
    fn a_full_snapshot_carries_the_song_status_length_and_position() {
        let event = parse_event(
            r#"{"type":"playback_state","status":"playing",
                "item":{"uri":"spotify:track:abc","entity_type":"track",
                        "decorations":{"playback":{"duration_ms":210000}}},
                "position":{"position_ms":4000,"timestamp_ms":1700000000000,"speed":1.0},
                "volume":40}"#,
        );
        assert_eq!(
            event,
            Event::Playback(Update {
                uri: Some("spotify:track:abc".into()),
                status: Some(Status::Playing),
                duration_ms: Some(210_000),
                anchor: Some(Anchor {
                    position_ms: 4000,
                    timestamp_ms: 1_700_000_000_000,
                    speed: 1.0
                }),
            })
        );
    }

    #[test]
    fn the_small_events_set_one_thing_each() {
        assert_eq!(
            parse_event(r#"{"type":"playback_changed","status":"paused"}"#),
            Event::Playback(Update {
                status: Some(Status::Paused),
                ..Update::default()
            })
        );
        assert_eq!(
            parse_event(
                r#"{"type":"position_sync","position":{"position_ms":45000,"timestamp_ms":9,"speed":1.0}}"#
            ),
            Event::Playback(Update {
                anchor: Some(Anchor {
                    position_ms: 45_000,
                    timestamp_ms: 9,
                    speed: 1.0
                }),
                ..Update::default()
            })
        );
        assert_eq!(
            parse_event(r#"{"type":"track_changed","item":{"uri":"spotify:track:next"}}"#),
            Event::Playback(Update {
                uri: Some("spotify:track:next".into()),
                ..Update::default()
            })
        );
    }

    #[test]
    fn errors_are_kept_and_unknown_messages_are_ignored() {
        assert_eq!(
            parse_event(r#"{"type":"error","message":"command requires authentication"}"#),
            Event::Error("command requires authentication".into())
        );
        assert_eq!(
            parse_event(r#"{"type":"volume_changed","volume":42}"#),
            Event::Other
        );
        assert_eq!(parse_event("not json"), Event::Other);
        assert_eq!(parse_event(r#"{"no":"type"}"#), Event::Other);
    }

    #[test]
    fn a_position_is_worked_out_from_its_anchor() {
        let anchor = Anchor {
            position_ms: 10_000,
            timestamp_ms: 1_000,
            speed: 1.0,
        };
        assert_eq!(anchor.position_at(1_000), 10_000);
        assert_eq!(anchor.position_at(3_500), 12_500);
        // A clock that reads earlier than the anchor never goes backwards.
        assert_eq!(anchor.position_at(500), 10_000);
    }

    #[test]
    fn commands_match_what_soloist_documents() {
        assert_eq!(
            play(Some("spotify:track:x")),
            json!({"type":"command","command":"play","uri":"spotify:track:x"})
        );
        assert_eq!(play(None), json!({"type":"command","command":"play"}));
        assert_eq!(
            seek(30_000),
            json!({"type":"command","command":"seek","position_ms":30000})
        );
        assert_eq!(
            set_volume(0.5),
            json!({"type":"command","command":"set_volume","volume":50})
        );
        assert_eq!(set_volume(3.0)["volume"], 100);
        assert_eq!(set_volume(-1.0)["volume"], 0);
        assert_eq!(
            command("pause"),
            json!({"type":"command","command":"pause"})
        );
    }
}
