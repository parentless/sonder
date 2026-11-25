//! Sonder on Rune: the new front end over the existing backend.
//!
//! Design A (refined layout) with design B's Now Playing morph. Run it with
//! `cargo run --release`; headless pictures: `-- --snap out.png --scene liked`.

#![deny(unsafe_code)]

mod art;
mod bridge;
mod icons;
mod memory;
mod sample;
mod theme;
mod ui;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use cadence::backend::{BackendCommand, BackendEvent};
use cadence::model::{Playlist, Track};
use rune::{AppSpec, Cx, El, TextEdit, Tx, Waker};

use bridge::{Bridge, Msg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conn {
    Starting,
    /// First run: Spotify needs an app (a Client ID) before anyone can sign in.
    Setup,
    Login,
    Connecting,
    Ready,
    Premium,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Page {
    Liked,
    Playlists,
    Recent,
    Playlist(String),
}

/// The songs behind one page, shared so playing from a long list copies nothing.
#[derive(Clone)]
pub struct Source {
    pub tracks: Arc<[Track]>,
    pub total: u32,
    pub done: bool,
    pub error: Option<String>,
}

impl Default for Source {
    fn default() -> Self {
        Source {
            tracks: Arc::from(Vec::new()),
            total: 0,
            done: false,
            error: None,
        }
    }
}

pub struct State {
    pub conn: Conn,
    pub message: Option<String>,
    pub client_id: TextEdit,
    pub bridge: Option<Bridge>,
    pub art: Option<art::Art>,
    pub icons: Option<icons::Icons>,

    pub liked: Source,
    pub liked_ids: HashSet<String>,
    pub playlists: Vec<Playlist>,
    pub recent: Source,
    pub opened: HashMap<String, Source>,
    pub loaded: bool,

    pub page: Page,
    pub query: TextEdit,
    /// Indexes into the page's songs that match the search, in order.
    pub shown: Vec<usize>,

    pub now: Option<Track>,
    pub up_next: Vec<Track>,
    pub playing: bool,
    pub anchor_ms: u32,
    pub anchor_at: f64,
    pub clock: f64,
    pub second: u32,
    pub shuffle: bool,

    pub wide: bool,
    pub np: bool,
    pub config_serial: u64,
}

impl Default for State {
    fn default() -> Self {
        State {
            conn: Conn::Starting,
            message: None,
            client_id: TextEdit::new(""),
            bridge: None,
            art: None,
            icons: None,
            liked: Source::default(),
            liked_ids: HashSet::new(),
            playlists: Vec::new(),
            recent: Source::default(),
            opened: HashMap::new(),
            loaded: false,
            page: Page::Liked,
            query: TextEdit::new(""),
            shown: Vec::new(),
            now: None,
            up_next: Vec::new(),
            playing: false,
            anchor_ms: 0,
            anchor_at: 0.0,
            clock: 0.0,
            second: 0,
            shuffle: false,
            wide: true,
            np: false,
            config_serial: 0,
        }
    }
}

impl State {
    pub fn source(&self) -> Source {
        match &self.page {
            Page::Liked => self.liked.clone(),
            Page::Recent => self.recent.clone(),
            Page::Playlist(id) => self.opened.get(id).cloned().unwrap_or_default(),
            Page::Playlists => Source::default(),
        }
    }

    /// Recomputes which songs the search lets through.
    pub fn refresh(&mut self) {
        let source = self.source();
        let query = self.query.text.trim().to_lowercase();
        self.shown = (0..source.tracks.len())
            .filter(|&i| {
                query.is_empty() || {
                    let track = &source.tracks[i];
                    track.title.to_lowercase().contains(&query)
                        || track.artist.to_lowercase().contains(&query)
                        || track.album.to_lowercase().contains(&query)
                }
            })
            .collect();
    }

    pub fn go(&mut self, page: Page) {
        if self.page == page {
            return;
        }
        if let Page::Playlist(id) = &page {
            if !self.opened.contains_key(id) {
                if let Some(playlist) = self.playlists.iter().find(|p| &p.source_id == id).cloned()
                {
                    self.opened.insert(id.clone(), Source::default());
                    if let Some(bridge) = &self.bridge {
                        bridge.load_playlist(playlist);
                    }
                }
            }
        }
        self.page = page;
        self.query = TextEdit::new("");
        self.refresh();
    }

    pub fn send(&self, command: BackendCommand) {
        if let Some(bridge) = &self.bridge {
            bridge.send(command);
        }
    }

    /// Plays the page's songs from `index` (an index into the full list).
    pub fn play_from(&mut self, index: usize) {
        let source = self.source();
        if index < source.tracks.len() {
            self.send(BackendCommand::PlayContext {
                tracks: source.tracks,
                index,
            });
        }
    }

    pub fn toggle_play(&mut self) {
        let command = if self.playing {
            BackendCommand::Pause
        } else {
            BackendCommand::Resume
        };
        self.send(command);
    }

    pub fn like(&mut self, track: Track) {
        let liked = !self.liked_ids.contains(&track.source_id);
        if liked {
            self.liked_ids.insert(track.source_id.clone());
        } else {
            self.liked_ids.remove(&track.source_id);
        }
        if let Some(bridge) = &self.bridge {
            bridge.set_liked(track, liked);
        }
    }

    pub fn progress_ms(&self) -> u32 {
        let Some(now) = &self.now else { return 0 };
        let elapsed = if self.playing {
            ((self.clock - self.anchor_at).max(0.0) * 1000.0) as u32
        } else {
            0
        };
        (self.anchor_ms + elapsed).min(now.duration_ms)
    }

    fn set_library(&mut self, liked: Vec<Track>, playlists: Vec<Playlist>) {
        self.liked_ids = liked.iter().map(|t| t.source_id.clone()).collect();
        self.liked = Source {
            total: liked.len() as u32,
            done: true,
            tracks: liked.into(),
            error: None,
        };
        self.playlists = playlists;
        self.loaded = true;
        self.conn = Conn::Ready;
        self.refresh();
    }

    fn apply(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::SetupRequired => self.conn = Conn::Setup,
            BackendEvent::SpotifyConfigured { generation, .. } => {
                if generation == self.config_serial && generation != 0 {
                    self.conn = Conn::Connecting;
                    self.send(BackendCommand::Authenticate { generation });
                }
            }
            BackendEvent::SpotifyConfigurationFailed { error, .. } => {
                self.conn = Conn::Setup;
                self.message = Some(format!(
                    "Could not set up Spotify. Check the Client ID. {error}"
                ));
            }
            BackendEvent::AuthorizationRequired | BackendEvent::LoggedOut => {
                if self.conn != Conn::Connecting {
                    self.conn = Conn::Login;
                }
            }
            BackendEvent::CatalogReady { .. } => {
                if self.conn != Conn::Premium {
                    self.conn = Conn::Ready;
                    self.message = None;
                }
            }
            BackendEvent::CachedLibrary {
                liked_tracks,
                playlists,
                ..
            }
            | BackendEvent::LibraryLoaded {
                liked_tracks,
                playlists,
                ..
            } => {
                self.set_library(liked_tracks, playlists);
            }
            BackendEvent::LocalStateLoaded {
                recently_played, ..
            } => {
                self.recent = Source {
                    total: recently_played.len() as u32,
                    done: true,
                    tracks: recently_played.into(),
                    error: None,
                };
                self.refresh();
            }
            BackendEvent::PremiumRequired => self.conn = Conn::Premium,
            BackendEvent::AuthorizationFailed(error) => {
                self.conn = Conn::Login;
                self.message = Some(error);
            }
            BackendEvent::FatalError(error) => {
                self.conn = Conn::Failed;
                self.message = Some(error);
            }
            BackendEvent::Playing { .. } => {
                self.playing = true;
                self.anchor_at = self.clock;
            }
            BackendEvent::Paused { .. } => {
                self.anchor_ms = self.progress_ms();
                self.playing = false;
            }
            BackendEvent::EndOfTrack { .. } | BackendEvent::QueueEnded => self.playing = false,
            BackendEvent::PositionChanged { position_ms, .. } => {
                self.anchor_ms = position_ms;
                self.anchor_at = self.clock;
            }
            BackendEvent::PlaybackContext { current, next } => {
                self.now = Some(current);
                self.up_next = next;
                self.anchor_ms = 0;
                self.anchor_at = self.clock;
            }
            BackendEvent::PlaybackSnapshotLoaded {
                current,
                next,
                position_ms,
            } => {
                self.now = Some(current);
                self.up_next = next;
                self.anchor_ms = position_ms;
                self.anchor_at = self.clock;
            }
            BackendEvent::PlaybackFailed(error) | BackendEvent::Error(error) => {
                self.message = Some(error);
            }
            _ => {}
        }
    }

    /// Drains what the backend said since the last frame. True when the screen changed.
    fn drain(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(bridge) = &self.bridge else {
                return changed;
            };
            let Ok(msg) = bridge.inbox.try_recv() else {
                break;
            };
            changed = true;
            self.on_msg(msg);
        }
        changed
    }

    fn on_msg(&mut self, msg: Msg) {
        {
            match msg {
                Msg::Event(event) => self.apply(event),
                Msg::PlaylistPage {
                    id,
                    tracks,
                    total,
                    done,
                } => {
                    let entry = self.opened.entry(id.clone()).or_default();
                    let mut all = entry.tracks.to_vec();
                    all.extend(tracks);
                    *entry = Source {
                        tracks: all.into(),
                        total,
                        done,
                        error: None,
                    };
                    if self.page == Page::Playlist(id) {
                        self.refresh();
                    }
                }
                Msg::PlaylistFailed { id, error } => {
                    self.opened.entry(id).or_default().error = Some(error);
                }
                Msg::LikeResult {
                    source_id,
                    liked,
                    ok,
                } => {
                    if !ok {
                        // Spotify said no: put the heart back as it was.
                        if liked {
                            self.liked_ids.remove(&source_id);
                        } else {
                            self.liked_ids.insert(source_id);
                        }
                        self.message = Some("Could not update your liked songs.".into());
                    }
                }
                Msg::Art { url, bytes } => {
                    if let Some(art) = self.art.as_mut() {
                        art.supply(url, bytes);
                    }
                }
            }
        }
    }
}

fn setup(s: &mut State, waker: Waker) {
    s.art = Some(art::Art::new(waker.clone()));
    s.icons = Some(icons::Icons::new(waker.clone()));
    // `SONDER_OFFLINE=1` keeps the backend out of it, for pictures taken from sample data.
    if std::env::var_os("SONDER_OFFLINE").is_none() {
        s.bridge = Some(Bridge::start(waker));
    }
}

fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    s.clock = clock;
    let mut changed = s.drain();
    // Covers the last frame asked for and did not have yet.
    if let (Some(art), Some(bridge)) = (s.art.as_mut(), s.bridge.as_ref()) {
        if let Some(now) = &s.now {
            if let Some(url) = &now.artwork_url {
                if s.np {
                    art.pin(url);
                }
            }
        }
        for url in art.take_wanted() {
            bridge.fetch_art(url);
        }
    }
    // The progress bar moves once a second, and only while something plays.
    if s.playing {
        let second = s.progress_ms() / 1000;
        if second != s.second {
            s.second = second;
            changed = true;
        }
        tx.wake_after(Duration::from_millis(250));
    }
    changed
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    ui::root(s, cx)
}

fn main() {
    memory::gpu_policy();
    memory::start();
    rune::run(AppSpec::<State> {
        scenes: sample::SCENES,
        setup: Some(setup),
        poll: Some(poll),
        min_size: Some((900.0, 560.0)),
        // Sonder's own Inter files, so the letters match the GPUI app.
        fonts: &[
            include_bytes!("../../../assets/fonts/Inter-Variable.ttf"),
            include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
            include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
            include_bytes!("../../../assets/fonts/Inter-Bold.ttf"),
        ],
        families: ("Inter", "ui-monospace"),
        ..AppSpec::new("Sonder", (1180.0, 760.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> State {
        let mut s = State::default();
        (sample::SCENES[0].1)(&mut s);
        s
    }

    #[test]
    fn the_search_keeps_only_songs_that_match() {
        let mut s = loaded();
        let all = s.shown.len();
        s.query = TextEdit::new("glass");
        s.refresh();
        assert!(!s.shown.is_empty() && s.shown.len() < all);
        let tracks = s.source().tracks;
        assert!(s.shown.iter().all(|&i| {
            let t = &tracks[i];
            [t.title.as_ref(), t.artist.as_ref(), t.album.as_ref()]
                .iter()
                .any(|f| f.to_lowercase().contains("glass"))
        }));
    }

    #[test]
    fn a_library_arriving_is_ready_to_show_and_remembers_what_is_liked() {
        let mut s = State::default();
        assert!(!s.loaded);
        s.apply(BackendEvent::CachedLibrary {
            generation: 1,
            liked_tracks: (0..3).map(sample::track).collect(),
            playlists: Vec::new(),
        });
        assert!(s.loaded && s.conn == Conn::Ready);
        assert_eq!(s.shown.len(), 3);
        assert!(s.liked_ids.contains("sample1"));
    }

    #[test]
    fn a_heart_flips_at_once_and_flips_back_if_spotify_refuses() {
        let mut s = loaded();
        let song = sample::track(1); // not liked in the sample
        s.like(song.clone());
        assert!(s.liked_ids.contains(&song.source_id));
        // Spotify refuses: the heart goes back to how it was.
        s.on_msg(Msg::LikeResult {
            source_id: song.source_id.clone(),
            liked: true,
            ok: false,
        });
        assert!(!s.liked_ids.contains(&song.source_id));
        assert!(s.message.is_some());
    }

    #[test]
    fn progress_follows_the_clock_only_while_playing() {
        let mut s = loaded();
        s.anchor_ms = 10_000;
        s.anchor_at = 100.0;
        s.clock = 103.0;
        s.playing = true;
        assert_eq!(s.progress_ms(), 13_000);
        s.playing = false;
        assert_eq!(s.progress_ms(), 10_000);
    }

    #[test]
    fn the_sign_in_flow_follows_the_backend() {
        let mut s = State::default();
        s.apply(BackendEvent::SetupRequired);
        assert_eq!(s.conn, Conn::Setup);
        s.apply(BackendEvent::AuthorizationRequired);
        assert_eq!(s.conn, Conn::Login);
        s.conn = Conn::Connecting;
        s.apply(BackendEvent::CatalogReady { generation: 1 });
        assert_eq!(s.conn, Conn::Ready);
    }
}
