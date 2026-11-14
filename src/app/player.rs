use super::*;
use std::time::Instant;

/// How far playback may drift from the saved position before it is written back.
const POSITION_SAVE_INTERVAL_MS: u32 = 5_000;

/// Reported when a playback command could not be delivered to the backend.
pub(super) struct PlaybackUnavailable;

impl EventEmitter<PlaybackUnavailable> for Player {}

/// After this many songs in a row fail, playback stops instead of skipping
/// on through the queue.
const FAILURES_BEFORE_STOPPING: u8 = 3;

/// Something only the listener can act on, shown over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlaybackProblem {
    /// Playback could not connect; `detail` is the reason, for support.
    NotConnected { detail: SharedString },
    /// The playback sign-in page is open, waiting for approval.
    AwaitingApproval { url: SharedString },
    /// Spotify won't unlock songs for this account.
    AccountRefused,
}

/// Playback state that belongs to the process rather than to a window.
pub(super) struct Player {
    backend: BackendHandle,
    now_playing: Option<Arc<model::Track>>,
    context: Arc<[model::Track]>,
    playing: bool,
    shuffle: bool,
    loading: bool,
    /// Position and play state to reapply once a reconnected player is ready.
    restore: Option<(u32, bool)>,
    /// The current track still owes a history entry once it plays.
    history_pending: bool,
    clock: playback_clock::PlaybackClock,
    saved_position_ms: u32,
    /// The whole second last drawn. Reports arrive several times a second, but
    /// the clock text and bar only change on the second, so the rest repaint
    /// nothing.
    painted_second: u32,
    volume: f32,
    volume_before_mute: f32,
    volume_dragging: bool,
    error: Option<SharedString>,
    problem: Option<PlaybackProblem>,
    /// The last connection attempt failed; a play can't start until one works.
    connect_failed: bool,
    /// Songs that failed since the last one played.
    failures_in_a_row: u8,
    /// Playback stopped itself after too many failures.
    gave_up: bool,
}

impl Player {
    pub(super) fn new(backend: BackendHandle, now: Instant) -> Self {
        Self {
            backend,
            now_playing: None,
            context: Arc::default(),
            playing: false,
            shuffle: false,
            loading: false,
            restore: None,
            history_pending: false,
            clock: playback_clock::PlaybackClock::new(now),
            saved_position_ms: 0,
            painted_second: u32::MAX,
            volume: 0.72,
            volume_before_mute: 0.72,
            volume_dragging: false,
            error: None,
            problem: None,
            connect_failed: false,
            failures_in_a_row: 0,
            gave_up: false,
        }
    }

    pub(super) fn problem(&self) -> Option<&PlaybackProblem> {
        self.problem.as_ref()
    }

    /// Whether the last failure made playback stop rather than skip on.
    pub(super) fn gave_up(&self) -> bool {
        self.gave_up
    }

    /// Hides the problem; the account menu still says what is wrong.
    pub(super) fn dismiss_problem(&mut self, cx: &mut Context<Self>) -> bool {
        if self.problem.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    pub(super) fn retry_playback(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.send(BackendCommand::RetryPlayback, cx);
        cx.notify();
    }

    pub(super) fn reset_playback(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.send(BackendCommand::ResetPlayback, cx);
        cx.notify();
    }

    pub(super) fn now_playing(&self) -> Option<&model::Track> {
        self.now_playing.as_deref()
    }

    pub(super) fn shared_now_playing(&self) -> Option<Arc<model::Track>> {
        self.now_playing.clone()
    }

    pub(super) fn context(&self) -> &Arc<[model::Track]> {
        &self.context
    }

    #[cfg(test)]
    pub(super) fn queue(&self) -> &[model::Track] {
        self.context.get(1..).unwrap_or_default()
    }

    pub(super) fn playing(&self) -> bool {
        self.playing
    }

    #[cfg(test)]
    pub(super) fn loading(&self) -> bool {
        self.loading
    }

    /// Where playback has reached, extrapolated between backend reports.
    pub(super) fn position_ms(&self, now: Instant) -> u32 {
        self.clock.position_ms(now)
    }

    /// The last confirmed or requested position, without extrapolation.
    pub(super) fn anchor_ms(&self) -> u32 {
        self.clock.anchor_ms()
    }

    /// Moves play state and re-anchors the clock together, so the extrapolated
    /// position cannot keep running after playback stops.
    fn set_playback(&mut self, playing: bool, cx: &App) {
        self.playing = playing;
        self.clock
            .set_playing(playing, cx.background_executor().now());
    }

    pub(super) fn volume(&self) -> f32 {
        self.volume
    }

    pub(super) fn volume_dragging(&self) -> bool {
        self.volume_dragging
    }

    pub(super) fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub(super) fn is_current_track(&self, track: &model::Track) -> bool {
        self.now_playing.as_ref().is_some_and(|playing| {
            playing.provider == track.provider && playing.source_id == track.source_id
        })
    }

    fn live_track_matches(&self, spotify_uri: &str) -> bool {
        self.now_playing
            .as_ref()
            .and_then(|track| track.spotify_uri.as_deref())
            == Some(spotify_uri)
    }

    /// History records what actually played, not what was loaded: librespot
    /// only confirms a track opened once it reports Playing.
    fn record_first_play(&mut self) {
        if !self.history_pending {
            return;
        }
        if let Some(track) = self.now_playing.clone() {
            self.backend
                .send(BackendCommand::MarkPlayed((*track).clone()));
        }
        self.history_pending = false;
    }

    /// Playback commands are dropped while a restore is in flight so they cannot
    /// race the position the backend is about to reapply.
    fn send(&self, command: BackendCommand, cx: &mut Context<Self>) -> bool {
        if self.restore.is_some() {
            return false;
        }
        self.deliver(command, cx)
    }

    /// Sends `command` and reports the failure when the backend cannot take it,
    /// so a dead or saturated worker does not leave controls silently inert.
    fn deliver(&self, command: BackendCommand, cx: &mut Context<Self>) -> bool {
        if self.backend.send(command) {
            return true;
        }
        cx.emit(PlaybackUnavailable);
        false
    }

    pub(super) fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.now_playing.is_none() {
            return;
        }
        let playing = !self.playing;
        if self.send(
            if self.playing {
                BackendCommand::Pause
            } else {
                BackendCommand::Resume
            },
            cx,
        ) {
            self.set_playback(playing, cx);
            self.loading = playing;
        }
        cx.notify();
    }

    /// Moves to `playing`, doing nothing if playback is already in that state.
    pub(super) fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        if self.playing != playing {
            self.toggle(cx);
        }
    }

    pub(super) fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub(super) fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.shuffle;
        if self.deliver(BackendCommand::SetShuffle(enabled), cx) {
            self.shuffle = enabled;
            cx.notify();
        }
    }

    pub(super) fn next(&mut self, cx: &mut Context<Self>) {
        if self.now_playing.is_some() && self.send(BackendCommand::Next, cx) {
            self.loading = true;
        }
        cx.notify();
    }

    pub(super) fn previous(&mut self, cx: &mut Context<Self>) {
        if self.now_playing.is_some() && self.send(BackendCommand::Previous, cx) {
            self.loading = true;
        }
        cx.notify();
    }

    pub(super) fn seek(&mut self, position_ms: u32, cx: &mut Context<Self>) {
        if self.send(BackendCommand::Seek(position_ms), cx) {
            self.clock.seek(position_ms, cx.background_executor().now());
        } else if self.restore.is_some() {
            // A reconnect drops commands silently. Left unreported, the scrub
            // the listener just finished would simply spring back.
            cx.emit(PlaybackUnavailable);
        }
        cx.notify();
    }

    pub(super) fn play_context(
        &mut self,
        tracks: Vec<model::Track>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let started = self.send(BackendCommand::PlayContext { tracks, index }, cx);
        if started {
            self.clock.reset(0, cx.background_executor().now());
            self.set_playback(false, cx);
            self.loading = true;
        }
        cx.notify();
        started
    }

    pub(super) fn play_queue_item(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.send(BackendCommand::PlayQueueItem { offset }, cx) {
            self.loading = true;
        }
        cx.notify();
    }

    pub(super) fn play_playlist(
        &mut self,
        playlist: model::Playlist,
        tracks: Vec<model::Track>,
        index: usize,
        next_offset: Option<u32>,
        cx: &mut Context<Self>,
    ) -> bool {
        let started = self.send(
            BackendCommand::PlayPlaylistContext {
                playlist,
                tracks,
                index,
                next_offset,
            },
            cx,
        );
        if started {
            self.clock.reset(0, cx.background_executor().now());
            self.set_playback(false, cx);
            self.loading = true;
        }
        cx.notify();
        started
    }

    pub(super) fn play_next(&mut self, track: model::Track, cx: &mut Context<Self>) -> bool {
        self.deliver(BackendCommand::PlayNext(track), cx)
    }

    pub(super) fn append_to_queue(&mut self, track: model::Track, cx: &mut Context<Self>) -> bool {
        self.deliver(BackendCommand::AppendToQueue(track), cx)
    }

    pub(super) fn start_radio(
        &mut self,
        request_id: u64,
        seed: model::Track,
        cx: &mut Context<Self>,
    ) -> bool {
        self.deliver(BackendCommand::StartRadio { request_id, seed }, cx)
    }

    pub(super) fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.loading = loading;
        cx.notify();
    }

    pub(super) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.volume > 0. {
            self.volume_before_mute = self.volume;
            self.volume = 0.;
        } else {
            self.volume = self.volume_before_mute.max(0.2);
        }
        self.deliver(BackendCommand::SetVolume(self.volume), cx);
        cx.notify();
    }

    pub(super) fn begin_volume_drag(
        &mut self,
        pointer_x: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.volume_dragging = true;
        self.drag_volume(pointer_x, window, cx);
    }

    pub(super) fn drag_volume(
        &mut self,
        pointer_x: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let window_width = window.viewport_size().width;
        self.volume = volume_for_pointer(pointer_x, window_width, window.rem_size());
        if self.volume > 0. {
            self.volume_before_mute = self.volume;
        }
        self.deliver(BackendCommand::SetVolume(self.volume), cx);
        cx.notify();
    }

    pub(super) fn end_volume_drag(&mut self, cx: &mut Context<Self>) {
        if self.volume_dragging {
            self.volume_dragging = false;
            cx.notify();
        }
    }

    /// Writes the live position back so a restart resumes where the listener left off.
    /// The position to persist for the current track, if one is playing.
    pub(super) fn position_snapshot(&self) -> Option<u32> {
        self.now_playing
            .as_ref()
            .and_then(|track| track.spotify_uri.as_ref())
            .map(|_| self.clock.anchor_ms())
    }

    pub(super) fn save_position(&self) {
        if let Some(spotify_uri) = self
            .now_playing
            .as_ref()
            .and_then(|track| track.spotify_uri.clone())
        {
            self.backend.send(BackendCommand::SavePlaybackPosition {
                spotify_uri,
                position_ms: self.clock.anchor_ms(),
            });
        }
    }

    fn save_position_if_moved(&mut self, spotify_uri: String, position_ms: u32) {
        if position_ms.abs_diff(self.saved_position_ms) < POSITION_SAVE_INTERVAL_MS {
            return;
        }
        self.backend.send(BackendCommand::SavePlaybackPosition {
            spotify_uri,
            position_ms,
        });
        self.saved_position_ms = position_ms;
    }

    fn adopt_context(&mut self, current: model::Track, next: Vec<model::Track>) {
        self.context = std::iter::once(current.clone())
            .chain(next)
            .collect::<Vec<_>>()
            .into();
        self.now_playing = Some(Arc::new(current));
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.now_playing = None;
        self.context = Arc::default();
        self.set_playback(false, cx);
        self.loading = false;
        self.restore = None;
        self.history_pending = false;
        self.clock.reset(0, cx.background_executor().now());
        self.saved_position_ms = 0;
        self.error = None;
        self.problem = None;
        self.connect_failed = false;
        self.failures_in_a_row = 0;
        self.gave_up = false;
        cx.notify();
    }

    /// Applies the playback half of a backend event, returning the event when the
    /// surrounding app still has its own work to do for it.
    pub(super) fn handle_backend_event(
        &mut self,
        event: BackendEvent,
        cx: &mut Context<Self>,
    ) -> Option<BackendEvent> {
        match event {
            BackendEvent::PlaybackReady => {
                self.error = None;
                self.connect_failed = false;
                self.failures_in_a_row = 0;
                if matches!(
                    self.problem,
                    Some(
                        PlaybackProblem::NotConnected { .. }
                            | PlaybackProblem::AwaitingApproval { .. }
                    )
                ) {
                    self.problem = None;
                }
                self.backend.send(BackendCommand::SetVolume(self.volume));
            }
            BackendEvent::PlaybackConnectFailed(detail) => {
                self.connect_failed = true;
                self.loading = false;
                self.error = Some("Spotify playback isn’t connected".into());
                self.problem = Some(PlaybackProblem::NotConnected {
                    detail: detail.into(),
                });
            }
            BackendEvent::PlaybackApprovalRequested { url } => {
                self.problem = Some(PlaybackProblem::AwaitingApproval { url: url.into() });
            }
            BackendEvent::StreamingRefused => {
                self.error = Some("Spotify won’t stream to this account".into());
                self.problem = Some(PlaybackProblem::AccountRefused);
                self.loading = false;
                if self.playing || self.now_playing.is_some() {
                    self.set_playback(false, cx);
                    // The player would otherwise spend seconds on a song it
                    // can't unlock.
                    self.backend.send(BackendCommand::Pause);
                }
            }
            BackendEvent::PlaybackReconnecting => {
                if self.restore.is_none() && self.now_playing.is_some() {
                    self.restore = Some((self.clock.anchor_ms(), self.playing));
                }
                self.loading = true;
            }
            BackendEvent::PlaybackReconnected => {
                self.error = None;
                self.backend.send(BackendCommand::SetVolume(self.volume));
                if let Some((position_ms, playing)) = self.restore {
                    self.backend.send(BackendCommand::RestorePlayback {
                        position_ms,
                        playing,
                    });
                } else {
                    self.loading = false;
                }
            }
            BackendEvent::PlaybackRestored {
                position_ms,
                playing,
            } => {
                self.clock
                    .reset(position_ms, cx.background_executor().now());
                self.saved_position_ms = position_ms;
                self.set_playback(playing, cx);
                self.loading = false;
                self.restore = None;
            }
            BackendEvent::PlaybackSettled => {
                self.loading = false;
                self.restore = None;
            }
            BackendEvent::QueueEnded => {
                self.set_playback(false, cx);
                self.loading = false;
                // A full bar with an armed Play button would lie: Play
                // starts this track over (or from wherever the seeker goes).
                self.clock.reset(0, cx.background_executor().now());
                self.saved_position_ms = 0;
            }
            BackendEvent::Playing { spotify_uri } => {
                if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    self.set_playback(true, cx);
                    self.loading = false;
                    self.failures_in_a_row = 0;
                    self.gave_up = false;
                    // A song that plays answers whatever the menu last said.
                    self.error = None;
                    self.record_first_play();
                }
            }
            BackendEvent::Loading { spotify_uri } => {
                if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    self.loading = true;
                }
            }
            BackendEvent::Paused { spotify_uri } => {
                if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    self.set_playback(false, cx);
                    self.loading = false;
                    if self.clock.anchor_ms() != self.saved_position_ms {
                        let position_ms = self.clock.anchor_ms();
                        self.backend.send(BackendCommand::SavePlaybackPosition {
                            spotify_uri,
                            position_ms,
                        });
                        self.saved_position_ms = position_ms;
                    }
                }
            }
            BackendEvent::EndOfTrack { spotify_uri } => {
                if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    self.set_playback(false, cx);
                    self.loading = false;
                    self.backend.send(BackendCommand::Next);
                }
            }
            BackendEvent::PositionChanged {
                spotify_uri,
                position_ms,
            } => {
                if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    let now = cx.background_executor().now();
                    self.clock.report(position_ms, now);
                    self.save_position_if_moved(spotify_uri, position_ms);
                    let second = self.clock.position_ms(now) / 1_000;
                    if second == self.painted_second {
                        return None;
                    }
                    self.painted_second = second;
                } else {
                    return None;
                }
            }
            BackendEvent::PlaybackSnapshotLoaded {
                current,
                next,
                position_ms,
            } => {
                self.adopt_context(current, next);
                self.clock
                    .reset(position_ms, cx.background_executor().now());
                self.saved_position_ms = position_ms;
                self.set_playback(false, cx);
                self.loading = false;
                self.history_pending = true;
            }
            BackendEvent::PlaybackContext { current, next } => {
                let changed = self.now_playing.as_ref().is_none_or(|track| {
                    track.provider != current.provider || track.source_id != current.source_id
                });
                self.adopt_context(current, next);
                if changed {
                    self.loading = true;
                    self.history_pending = true;
                    self.clock.reset(0, cx.background_executor().now());
                    self.saved_position_ms = 0;
                    self.restore = None;
                }
            }
            BackendEvent::PlaybackFailed(error) => {
                self.error = Some(error.into());
            }
            BackendEvent::TrackFailed { spotify_uri, error } => {
                self.gave_up = false;
                if self.connect_failed {
                    // Nothing can play until playback connects; skipping on
                    // would only fail every song the same way.
                    self.set_playback(false, cx);
                    self.loading = false;
                    if self.problem.is_none() {
                        self.problem = Some(PlaybackProblem::NotConnected {
                            detail: error.clone().into(),
                        });
                    }
                } else if self.restore.is_none() && self.live_track_matches(&spotify_uri) {
                    // librespot parks on a track it cannot open rather than
                    // skipping it, so move on the way a finished track does and
                    // keep the queue the backend still holds, unless songs keep
                    // failing or the account can't stream at all.
                    self.set_playback(false, cx);
                    self.loading = false;
                    self.failures_in_a_row = self.failures_in_a_row.saturating_add(1);
                    let account_refused = self.problem == Some(PlaybackProblem::AccountRefused);
                    if account_refused {
                        // The dialog says why; nothing else would play either.
                    } else if self.failures_in_a_row >= FAILURES_BEFORE_STOPPING {
                        self.failures_in_a_row = 0;
                        self.gave_up = true;
                    } else {
                        self.backend.send(BackendCommand::Next);
                    }
                }
                cx.notify();
                return Some(BackendEvent::TrackFailed { spotify_uri, error });
            }
            event => return Some(event),
        }
        cx.notify();
        None
    }
}
