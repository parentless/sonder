//! Voice play: hold the shortcut, say a song and artist, let go, and Sonder
//! finds it on Spotify and plays it.
//!
//! This connects the pieces in `crate::voice` to the player and to Settings.
//! It lives for the whole process, next to the player, so the shortcut works
//! while Sonder is in the tray or something else has the screen.

use super::*;
use crate::voice::{
    Config, HotkeyEvent, Model, capture, hook, model, query, sounds, transcribe, worker,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

/// What Voice play is doing right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Idle,
    /// The shortcut is held and the microphone is open.
    Listening,
    /// The key was let go. Carries what has been worked out so far.
    Working(SharedString),
}

/// What the listening animation shows. Separate from [`Phase`], which is for
/// Settings: this one carries the times the animation moves by.
#[derive(Clone, Debug)]
pub(super) enum Stage {
    /// Nothing is on screen.
    Hidden,
    /// The shortcut is held.
    Listening,
    /// The key was let go and the words are being worked out.
    Working {
        released: std::time::Instant,
        heard: Option<(SharedString, std::time::Instant)>,
    },
    /// A song was found and is playing.
    Found {
        released: std::time::Instant,
        heard: Option<(SharedString, std::time::Instant)>,
        title: SharedString,
        artist: SharedString,
        source: &'static str,
        at: std::time::Instant,
    },
    /// Nothing was found, or nothing was heard.
    Failed {
        released: std::time::Instant,
        heard: Option<(SharedString, std::time::Instant)>,
        message: SharedString,
        at: std::time::Instant,
    },
}

/// A model being downloaded.
pub(super) struct Download {
    pub model: Model,
    pub received: u64,
    cancel: Arc<AtomicBool>,
    _task: gpui_kit::Task<()>,
}

enum DownloadMessage {
    Progress(u64),
    Done(Result<(), String>),
}

pub(super) struct VoicePlay {
    config: Config,
    phase: Phase,
    /// What the last request came to, shown in Settings.
    last: Option<SharedString>,
    download: Option<Download>,
    download_error: Option<SharedString>,
    installed: [bool; Model::ALL.len()],
    listener: Option<hook::Listener>,
    events: async_channel::Sender<HotkeyEvent>,
    recording: Option<capture::Recording>,
    /// The speech-to-text helper, running only while the shortcut is held.
    worker: Option<worker::Worker>,
    /// Bumped by every press, so the end of an older request cannot overwrite
    /// a newer one.
    generation: u64,
    player: Entity<player::Player>,
    library: Entity<library::Library>,
    backend: BackendHandle,
    work: Option<gpui_kit::Task<()>>,
    stage: Stage,
    /// How loud the listener is now, shared with the microphone thread.
    level: Arc<std::sync::atomic::AtomicU32>,
    overlay: Option<gpui_kit::AnyWindowHandle>,
    /// Closes the overlay a moment after the result.
    overlay_timer: Option<gpui_kit::Task<()>>,
    _hotkeys: gpui_kit::Task<()>,
}

impl VoicePlay {
    pub(super) fn new(
        config: Config,
        player: Entity<player::Player>,
        library: Entity<library::Library>,
        backend: BackendHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let (events, incoming) = async_channel::bounded(16);
        let hotkeys = cx.spawn(async move |this, cx| {
            while let Ok(event) = incoming.recv().await {
                if this
                    .update(cx, |voice, cx| voice.on_hotkey(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut voice = Self {
            config: Config::default(),
            phase: Phase::Idle,
            last: None,
            download: None,
            download_error: None,
            installed: Model::ALL.map(model::is_installed),
            listener: None,
            events,
            recording: None,
            worker: None,
            generation: 0,
            player,
            library,
            backend,
            work: None,
            stage: Stage::Hidden,
            level: Arc::default(),
            overlay: None,
            overlay_timer: None,
            _hotkeys: hotkeys,
        };
        if let Err(error) = voice.apply(config, cx) {
            log::warn!("Voice play could not start: {error:#}");
        }
        voice
    }

    pub(super) fn config(&self) -> &Config {
        &self.config
    }

    pub(super) fn phase(&self) -> &Phase {
        &self.phase
    }

    pub(super) fn stage(&self) -> &Stage {
        &self.stage
    }

    /// How loud the listener is right now, 0 to 1.
    pub(super) fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub(super) fn last(&self) -> Option<&SharedString> {
        self.last.as_ref()
    }

    pub(super) fn download(&self) -> Option<&Download> {
        self.download.as_ref()
    }

    pub(super) fn download_error(&self) -> Option<&SharedString> {
        self.download_error.as_ref()
    }

    pub(super) fn installed(&self, model: Model) -> bool {
        self.installed[Model::ALL.iter().position(|m| *m == model).unwrap_or(0)]
    }

    fn refresh_installed(&mut self) {
        self.installed = Model::ALL.map(model::is_installed);
    }

    /// Applies new settings: starts or stops listening for the shortcut.
    /// Nothing changes if it cannot be done.
    pub(super) fn apply(&mut self, config: Config, cx: &mut Context<Self>) -> anyhow::Result<()> {
        use anyhow::{bail, ensure};
        if config.enabled {
            if !hook::SUPPORTED {
                bail!("Voice play works on Windows only for now.");
            }
            ensure!(
                self.installed(config.model),
                "Download the speech model first."
            );
            ensure!(config.hotkey.is_usable(), "Choose a different shortcut.");
            match self.listener.as_ref() {
                Some(listener) => listener.set_hotkey(&config.hotkey),
                None => {
                    self.listener =
                        Some(hook::Listener::start(&config.hotkey, self.events.clone())?);
                }
            }
        } else {
            self.listener = None;
            self.recording = None;
            self.worker = None;
            self.work = None;
            self.generation += 1;
            self.phase = Phase::Idle;
        }
        if !config.animation || !config.enabled {
            self.close_overlay(cx);
        }
        self.config = config;
        cx.notify();
        Ok(())
    }

    fn on_hotkey(&mut self, event: HotkeyEvent, cx: &mut Context<Self>) {
        match event {
            HotkeyEvent::Pressed => self.start_listening(cx),
            HotkeyEvent::Released => self.stop_listening(cx),
        }
    }

    fn start_listening(&mut self, cx: &mut Context<Self>) {
        if !self.config.enabled || !self.installed(self.config.model) {
            return;
        }
        // A new request replaces whatever the last one was still doing.
        self.generation += 1;
        self.work = None;
        self.worker = None;
        // The helper starts loading the model now, while the listener speaks.
        let started = model::path(self.config.model)
            .ok_or_else(|| anyhow::anyhow!("could not find the speech model"))
            .and_then(|path| worker::Worker::start(&path));
        match started {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                sounds::play(sounds::Sound::Fail);
                self.last = Some(format!("{error:#}").into());
                cx.notify();
                return;
            }
        }
        sounds::play(sounds::Sound::Start);
        let recording = capture::Recording::start();
        self.level = recording.level();
        self.recording = Some(recording);
        self.phase = Phase::Listening;
        self.overlay_timer = None;
        self.stage = Stage::Listening;
        self.show_overlay(cx);
        cx.notify();
    }

    /// Opens the animation, if the listener turned it on and it is not open.
    fn show_overlay(&mut self, cx: &mut Context<Self>) {
        if !self.config.animation || self.overlay.is_some() {
            return;
        }
        let voice = cx.entity();
        let bounds = voice_overlay::bounds(cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: true,
                kind: gpui_kit::WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |window, cx| {
                voice_overlay::leave_input_alone(window);
                cx.new(|cx| voice_overlay::VoiceOverlay::new(voice, cx))
            },
        );
        match opened {
            Ok(handle) => self.overlay = Some(handle.into()),
            Err(error) => log::warn!("could not open the voice animation: {error:#}"),
        }
    }

    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_timer = None;
        self.stage = Stage::Hidden;
        if let Some(handle) = self.overlay.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// Closes the animation after `delay`, unless a newer request has begun.
    fn close_overlay_after(&mut self, delay: std::time::Duration, cx: &mut Context<Self>) {
        let generation = self.generation;
        self.overlay_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |voice, cx| {
                if voice.generation == generation {
                    voice.close_overlay(cx);
                }
            });
        }));
    }

    fn stop_listening(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        sounds::play(sounds::Sound::Stop);
        self.phase = Phase::Working("Listening…".into());
        let released = std::time::Instant::now();
        self.stage = Stage::Working {
            released,
            heard: None,
        };
        cx.notify();
        let generation = self.generation;
        let answer = recording.finish();
        let worker = self.worker.take();
        let names = self.artist_names(cx);
        let (liked, favorites, recent, local) = {
            let library = self.library.read(cx);
            (
                library.liked_tracks().clone(),
                library.favorites().clone(),
                library.recently_played().clone(),
                library.local_tracks().clone(),
            )
        };
        let backend = self.backend.clone();
        let player = self.player.clone();
        self.work = Some(cx.spawn(async move |this, cx| {
            let outcome: Result<Option<SharedString>, SharedString> = async {
                let samples = answer
                    .recv()
                    .await
                    .map_err(|_| SharedString::from("The microphone stopped unexpectedly."))?
                    .map_err(SharedString::from)?;
                match capture::assess(&samples) {
                    capture::Heard::TooShort => return Ok(None),
                    capture::Heard::Silent => {
                        return Err("Didn't hear anything. Check your microphone.".into());
                    }
                    capture::Heard::Speech => {}
                }
                let worker =
                    worker.ok_or_else(|| SharedString::from("Speech to text was not running."))?;
                let transcript = worker
                    .finish(samples, transcribe::prompt(&names))
                    .recv()
                    .await
                    .map_err(|_| SharedString::from("Speech to text stopped unexpectedly."))?
                    .map_err(SharedString::from)?;
                log::info!(
                    "voice: heard {transcript:?} {:?} after release",
                    released.elapsed()
                );
                let request = query::parse(&transcript).ok_or_else(|| {
                    SharedString::from(if transcript.trim().is_empty() {
                        "Didn't catch that.".to_owned()
                    } else {
                        format!("Didn't catch a song in “{}”.", transcript.trim())
                    })
                })?;
                let heard: SharedString = request.search_text().into();
                let shown: SharedString = transcript
                    .trim()
                    .trim_end_matches(['.', '!', '?'])
                    .to_owned()
                    .into();
                let _ = this.update(cx, |voice, cx| {
                    if voice.generation == generation {
                        voice.phase = Phase::Working(heard.clone());
                        voice.stage = Stage::Working {
                            released,
                            heard: Some((shown.clone(), std::time::Instant::now())),
                        };
                        cx.notify();
                    }
                });
                // What the listener already has comes first: it is what they
                // most likely meant, it needs no search, and local files and an
                // offline or rate-limited Spotify still work.
                let known = query::pick_known(
                    &request,
                    &query::Known {
                        liked: &liked,
                        favorites: &favorites,
                        recent: &recent,
                        local: &local,
                    },
                );
                let (track, source) = match known {
                    Some(track) if track.is_local() => (track, " (local file)"),
                    Some(track) => (track, " (from your library)"),
                    None => {
                        let search = request.search_text();
                        let results =
                            catalog::request(&backend, |respond| BackendCommand::SearchCatalog {
                                query: search,
                                respond,
                            })
                            .await
                            .map_err(|error| {
                                SharedString::from(format!("Spotify search failed: {error}"))
                            })?;
                        let index = query::pick(&request, &results.tracks).ok_or_else(|| {
                            SharedString::from(format!(
                                "Couldn't find “{}” on Spotify.",
                                request.search_text()
                            ))
                        })?;
                        (results.tracks[index].clone(), "")
                    }
                };
                log::info!(
                    "voice: found {} {:?} after release{source}",
                    track.title,
                    released.elapsed()
                );
                let line = format!("Playing {} by {}{source}.", track.title, track.artist);
                let label = match source {
                    " (local file)" => "LOCAL FILE",
                    " (from your library)" => "FROM YOUR LIBRARY",
                    _ => "SPOTIFY",
                };
                let (title, artist) = (track.title.clone(), track.artist.clone());
                let started = this
                    .update(cx, |voice, cx| {
                        let started = voice.generation == generation
                            && player
                                .update(cx, |player, cx| player.play_context(vec![track], 0, cx));
                        if started {
                            let heard = match &voice.stage {
                                Stage::Working { heard, .. } => heard.clone(),
                                _ => None,
                            };
                            voice.stage = Stage::Found {
                                released,
                                heard,
                                title,
                                artist,
                                source: label,
                                at: std::time::Instant::now(),
                            };
                            cx.notify();
                        }
                        started
                    })
                    .map_err(|_| SharedString::default())?;
                if started {
                    Ok(Some(line.into()))
                } else {
                    Err("Playback isn't connected yet.".into())
                }
            }
            .await;
            let _ = this.update(cx, |voice, cx| voice.finish(generation, outcome, cx));
        }));
    }

    fn finish(
        &mut self,
        generation: u64,
        outcome: Result<Option<SharedString>, SharedString>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.phase = Phase::Idle;
        self.work = None;
        match outcome {
            Ok(Some(line)) => {
                self.last = Some(line);
                self.close_overlay_after(voice_overlay::CLOSE_AFTER, cx);
            }
            Ok(None) => {
                // A tap, not a request.
                self.close_overlay(cx);
            }
            Err(error) => {
                if error.is_empty() {
                    self.close_overlay(cx);
                } else {
                    sounds::play(sounds::Sound::Fail);
                    let (released, heard) = match &self.stage {
                        Stage::Working { released, heard } => (*released, heard.clone()),
                        _ => (std::time::Instant::now(), None),
                    };
                    self.stage = Stage::Failed {
                        released,
                        heard,
                        message: error.clone(),
                        at: std::time::Instant::now(),
                    };
                    self.last = Some(error);
                    self.close_overlay_after(voice_overlay::CLOSE_AFTER, cx);
                }
            }
        }
        cx.notify();
    }

    /// The artists the listener plays most, so the model spells them as
    /// Spotify does.
    fn artist_names(&self, cx: &App) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let library = self.library.read(cx);
        for track in library.liked_tracks().iter() {
            if track.artists.is_empty() {
                *counts.entry(&track.artist).or_default() += 1;
            }
            for artist in &track.artists {
                *counts.entry(&artist.name).or_default() += 1;
            }
        }
        let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        ranked
            .into_iter()
            .take(15)
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    /// Starts downloading a model, if none is being downloaded.
    pub(super) fn start_download(&mut self, model: Model, cx: &mut Context<Self>) {
        if self.download.is_some() || self.installed(model) {
            return;
        }
        self.download_error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let (messages, incoming) = async_channel::bounded(64);
        std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                let progress = messages.clone();
                let result = (|| -> anyhow::Result<()> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime.block_on(async {
                        let client = reqwest::Client::builder()
                            .user_agent("Sonder")
                            .timeout(Duration::from_secs(30 * 60))
                            .build()?;
                        let mut last_reported = 0u64;
                        model::download(&client, model, cancel, move |received| {
                            // A message for every megabyte is plenty for a bar.
                            if received - last_reported >= 1_000_000 {
                                last_reported = received;
                                let _ = progress.try_send(DownloadMessage::Progress(received));
                            }
                        })
                        .await
                    })
                })();
                let _ = messages.send_blocking(DownloadMessage::Done(
                    result.map_err(|error| format!("{error:#}")),
                ));
            }
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(message) = incoming.recv().await {
                let finished = matches!(message, DownloadMessage::Done(_));
                let alive = this.update(cx, |voice, cx| voice.on_download(message, cx));
                if finished || alive.is_err() {
                    break;
                }
            }
        });
        self.download = Some(Download {
            model,
            received: 0,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn on_download(&mut self, message: DownloadMessage, cx: &mut Context<Self>) {
        match message {
            DownloadMessage::Progress(received) => {
                if let Some(download) = self.download.as_mut() {
                    download.received = received;
                }
            }
            DownloadMessage::Done(result) => {
                let cancelled = self
                    .download
                    .take()
                    .is_some_and(|download| download.cancel.load(Ordering::Relaxed));
                self.refresh_installed();
                if let Err(error) = result
                    && !cancelled
                {
                    log::warn!("speech model download failed: {error}");
                    self.download_error = Some(error.into());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn cancel_download(&mut self, cx: &mut Context<Self>) {
        if let Some(download) = self.download.as_ref() {
            download.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Deletes a model from disk.
    pub(super) fn remove_model(&mut self, model: Model, cx: &mut Context<Self>) {
        self.download_error = match model::remove(model) {
            Ok(()) => None,
            Err(error) => Some(format!("{error:#}").into()),
        };
        self.refresh_installed();
        cx.notify();
    }
}
