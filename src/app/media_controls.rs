use super::*;
use std::time::Instant;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};

/// What the last push to the system told it, so a once-a-second position tick
/// does not re-send artwork and titles that have not changed.
#[derive(PartialEq, Eq)]
struct Published {
    track: Option<(model::Provider, String)>,
    playing: bool,
}

/// How far the player's position may differ from where the system has carried
/// the one it was last told before it is told again. The system advances the
/// position by itself while playing, so only a seek or a real stall gets
/// past this.
const POSITION_DRIFT_MS: u32 = 2_000;

/// Whether the system's position, carried forward from `published` (the
/// position it was told and when), now differs from the player's `anchor_ms`.
fn position_drifted(
    published: Option<(u32, Instant)>,
    playing: bool,
    now: Instant,
    anchor_ms: u32,
) -> bool {
    let Some((told_ms, told_at)) = published else {
        return true;
    };
    let carried = if playing {
        let elapsed = now.saturating_duration_since(told_at).as_millis();
        told_ms.saturating_add(u32::try_from(elapsed).unwrap_or(u32::MAX))
    } else {
        told_ms
    };
    carried.abs_diff(anchor_ms) > POSITION_DRIFT_MS
}

/// Publishes now-playing information to the operating system and accepts the transport
/// commands it sends back, so Sonder can be driven without its window.
pub(super) struct SystemMediaControls {
    controls: MediaControls,
    published: Option<Published>,
    /// The position last sent and when, for [`position_drifted`].
    published_position: Option<(u32, Instant)>,
    _command_forwarder: gpui_kit::Task<()>,
    #[cfg(target_os = "windows")]
    _media_window: MediaWindow,
}

impl SystemMediaControls {
    /// Attaches to the system controls, forwarding their commands to `player`.
    ///
    /// Returns `None` when the platform refuses them, which is not fatal: the
    /// app simply goes without media keys.
    pub(super) fn attach(player: Entity<player::Player>, cx: &mut App) -> Option<Self> {
        #[cfg(target_os = "windows")]
        let media_window = MediaWindow::new()
            .map_err(|error| {
                log::error!("could not create media control window: {error}");
            })
            .ok()?;
        let mut controls = MediaControls::new(PlatformConfig {
            display_name: "Sonder",
            dbus_name: "cadence",
            #[cfg(not(target_os = "windows"))]
            hwnd: None,
            #[cfg(target_os = "windows")]
            hwnd: Some(media_window.0),
        })
        .ok()?;

        let (commands, incoming) = async_channel::bounded(64);
        let saturated = std::sync::atomic::AtomicBool::new(false);
        // The system calls this from its own thread, so hand the event to the
        // foreground executor rather than touching the player here.
        controls
            .attach(move |event| {
                // Native media callbacks can run on the UI thread. Never block
                // that thread waiting for the foreground consumer to drain.
                match commands.try_send(event) {
                    Ok(()) => saturated.store(false, std::sync::atomic::Ordering::Relaxed),
                    Err(async_channel::TrySendError::Full(_))
                        if !saturated.swap(true, std::sync::atomic::Ordering::Relaxed) =>
                    {
                        log::warn!("media controls: command queue full; ignoring excess input");
                    }
                    Err(_) => {}
                }
            })
            .ok()?;

        let command_forwarder = cx.spawn(async move |cx| {
            while let Ok(event) = incoming.recv().await {
                cx.update(|cx| {
                    player.update(cx, |player, cx| apply(event, player, cx));
                });
            }
        });

        Some(Self {
            controls,
            published: None,
            published_position: None,
            _command_forwarder: command_forwarder,
            #[cfg(target_os = "windows")]
            _media_window: media_window,
        })
    }

    /// Pushes the player's state to the system, skipping what it already knows.
    pub(super) fn sync(&mut self, player: &player::Player) {
        let now_playing = player.now_playing();
        let published = Published {
            track: now_playing.map(|track| (track.provider, track.source_id.clone())),
            playing: player.playing(),
        };
        let now = Instant::now();
        let anchor_ms = player.anchor_ms();
        if self.published.as_ref() == Some(&published)
            && !position_drifted(self.published_position, published.playing, now, anchor_ms)
        {
            return;
        }
        self.published_position = Some((anchor_ms, now));
        let track_changed = self
            .published
            .as_ref()
            .is_none_or(|previous| previous.track != published.track);
        self.published = Some(published);

        let Some(track) = now_playing else {
            let _ = self.controls.set_playback(MediaPlayback::Stopped);
            return;
        };
        if track_changed {
            let _ = self.controls.set_metadata(MediaMetadata {
                title: Some(&track.title),
                album: Some(&track.album),
                artist: Some(&track.artist),
                // Embedded covers of local files are read by Sonder's own
                // image cache; the system panel only takes web addresses.
                cover_url: track
                    .artwork_url
                    .as_deref()
                    .filter(|url| !url.starts_with(crate::local_files::ARTWORK_SCHEME)),
                duration: Some(Duration::from_millis(u64::from(track.duration_ms))),
            });
        }
        let progress = Some(MediaPosition(Duration::from_millis(u64::from(
            player.anchor_ms(),
        ))));
        let _ = self.controls.set_playback(if player.playing() {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        });
    }
}

/// The player action a system command asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Transport {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek(u32),
}

/// Translates a system command, ignoring the ones Sonder does not offer.
fn transport_for(event: MediaControlEvent) -> Option<Transport> {
    match event {
        MediaControlEvent::Play => Some(Transport::Play),
        MediaControlEvent::Pause | MediaControlEvent::Stop => Some(Transport::Pause),
        MediaControlEvent::Toggle => Some(Transport::Toggle),
        MediaControlEvent::Next => Some(Transport::Next),
        MediaControlEvent::Previous => Some(Transport::Previous),
        MediaControlEvent::SetPosition(MediaPosition(position)) => Some(Transport::Seek(
            position.as_millis().min(u128::from(u32::MAX)) as u32,
        )),
        _ => None,
    }
}

fn apply(event: MediaControlEvent, player: &mut player::Player, cx: &mut Context<player::Player>) {
    match transport_for(event) {
        Some(Transport::Play) => player.set_playing(true, cx),
        Some(Transport::Pause) => player.set_playing(false, cx),
        Some(Transport::Toggle) => player.toggle(cx),
        Some(Transport::Next) => player.next(cx),
        Some(Transport::Previous) => player.previous(cx),
        Some(Transport::Seek(position_ms)) => player.seek(position_ms, cx),
        None => {}
    }
}

/// SMTC needs a real HWND that survives closing and reopening the UI. This
/// hidden top-level window lives on GPUI's UI thread, as do its controls.
#[cfg(target_os = "windows")]
struct MediaWindow(windows_sys::Win32::Foundation::HWND);

#[cfg(target_os = "windows")]
impl MediaWindow {
    fn new() -> std::io::Result<Self> {
        use windows_sys::{Win32::UI::WindowsAndMessaging::*, core::w};
        // STATIC is a system window class; no custom WndProc is needed.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Sonder"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(hwnd))
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for MediaWindow {
    fn drop(&mut self) {
        // `controls` is declared before this field, so SMTC detaches first.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaControlEvent, MediaPosition, Transport, position_drifted, transport_for};
    use std::time::{Duration, Instant};

    #[test]
    fn system_commands_map_to_transport_actions() {
        assert_eq!(
            transport_for(MediaControlEvent::Play),
            Some(Transport::Play)
        );
        assert_eq!(
            transport_for(MediaControlEvent::Pause),
            Some(Transport::Pause)
        );
        assert_eq!(
            transport_for(MediaControlEvent::Stop),
            Some(Transport::Pause)
        );
        assert_eq!(
            transport_for(MediaControlEvent::Toggle),
            Some(Transport::Toggle)
        );
        assert_eq!(
            transport_for(MediaControlEvent::Next),
            Some(Transport::Next)
        );
        assert_eq!(
            transport_for(MediaControlEvent::Previous),
            Some(Transport::Previous)
        );
    }

    #[test]
    fn seek_position_is_converted_to_milliseconds() {
        assert_eq!(
            transport_for(MediaControlEvent::SetPosition(MediaPosition(
                Duration::from_millis(4_500)
            ))),
            Some(Transport::Seek(4_500))
        );
    }

    #[test]
    fn seek_position_is_clamped_rather_than_wrapping() {
        assert_eq!(
            transport_for(MediaControlEvent::SetPosition(MediaPosition(
                Duration::from_secs(u64::from(u32::MAX))
            ))),
            Some(Transport::Seek(u32::MAX))
        );
    }

    #[test]
    fn commands_cadence_does_not_offer_are_ignored() {
        assert_eq!(transport_for(MediaControlEvent::Raise), None);
        assert_eq!(transport_for(MediaControlEvent::SetVolume(0.5)), None);
    }

    #[test]
    fn the_system_is_told_the_position_again_only_after_a_jump() {
        let told_at = Instant::now();
        let later = |seconds| told_at + Duration::from_secs(seconds);
        // Nothing sent yet.
        assert!(position_drifted(None, true, told_at, 0));
        // Playing on: ten seconds on, the player reports about where the
        // system has carried the position, so nothing is sent.
        assert!(!position_drifted(
            Some((30_000, told_at)),
            true,
            later(10),
            40_000
        ));
        assert!(!position_drifted(
            Some((30_000, told_at)),
            true,
            later(10),
            41_500
        ));
        // A seek forward or back is sent.
        assert!(position_drifted(
            Some((30_000, told_at)),
            true,
            later(10),
            90_000
        ));
        assert!(position_drifted(
            Some((30_000, told_at)),
            true,
            later(10),
            5_000
        ));
        // Paused: the position stays put, however long it has been.
        assert!(!position_drifted(
            Some((30_000, told_at)),
            false,
            later(60),
            30_000
        ));
        assert!(position_drifted(
            Some((30_000, told_at)),
            false,
            later(60),
            90_000
        ));
    }
}
