#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Sonder on Rune: the new front end over the existing backend.
//!
//! Design A (refined layout) with design B's Now Playing morph. Run it with
//! `cargo run --release`; headless pictures: `-- --snap out.png --scene liked`.

#![deny(unsafe_code)]

mod art;
mod bridge;
mod disk;
mod icons;
mod memory;
mod sample;
mod search;
mod shell;
mod theme;
mod ui;
mod views;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use cadence::backend::{BackendCommand, BackendEvent};
use cadence::model::{Playlist, Track, UserProfile};
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
    Home,
    Local,
    Liked,
    Playlists,
    Recent,
    Playlist(String),
    Album(String),
    Artist(String),
    Settings,
}

/// What an album or artist page says about itself.
#[derive(Clone, Default)]
pub struct Detail {
    pub title: String,
    pub sub: String,
    pub art: Option<String>,
    /// Artists are pictured in circles.
    pub round: bool,
}

/// Lyrics for the song that is playing, and whether the panel is up.
#[derive(Default)]
pub struct LyricsState {
    pub open: bool,
    /// Which song `data` belongs to.
    pub id: Option<String>,
    pub data: Option<cadence::lyrics::Lyrics>,
    pub loading: bool,
    /// The line being sung, to redraw only when it changes.
    pub line: Option<usize>,
}

/// The credits dialog: whose song, and what was found.
pub struct CreditsState {
    pub track: Track,
    pub data: Option<cadence::credits::Credits>,
    pub loading: bool,
}

/// A small menu for one song, opened by its "more" button or a right click.
pub struct TrackMenu {
    pub track: Track,
    /// Where it opened (window points).
    pub y: f32,
}

#[derive(Default)]
pub struct Home {
    pub range: cadence::model::HomeRange,
    pub data: Option<cadence::model::HomeData>,
    pub loading: bool,
}

#[derive(Default)]
pub struct Local {
    pub folders: Vec<std::path::PathBuf>,
    pub loaded: bool,
    pub unreadable: usize,
    pub truncated: bool,
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
    pub search: search::Search,
    /// The row a single click picked; a double click plays it.
    pub selected: Option<String>,
    pub profile: Option<UserProfile>,
    /// Whether the profile menu is down.
    pub menu: bool,
    pub volume: f32,
    /// Where the progress bar is held while it is being dragged (0..1).
    pub scrub: Option<f32>,
    /// Playlists the listener pinned; the sidebar lists them first.
    pub pinned: HashSet<String>,
    pub details: HashMap<String, Detail>,
    pub artist_albums: HashMap<String, Vec<cadence::model::Album>>,
    pub lyrics: LyricsState,
    pub credits: Option<CreditsState>,
    pub track_menu: Option<TrackMenu>,
    pub home: Home,
    pub local: Local,
    pub local_tracks: Source,
    /// A short note over the player bar, and the clock time it goes.
    pub toast: Option<(String, f64)>,
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
    pub prefs: cadence::storage::AppPreferences,
    pub prefs_loaded: bool,
    pub tray: Option<rune::Tray>,

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
            page: Page::Home,
            query: TextEdit::new(""),
            search: search::Search::default(),
            selected: None,
            profile: None,
            menu: false,
            volume: 0.72,
            scrub: None,
            pinned: HashSet::new(),
            details: HashMap::new(),
            artist_albums: HashMap::new(),
            lyrics: LyricsState::default(),
            credits: None,
            track_menu: None,
            home: Home::default(),
            local: Local::default(),
            local_tracks: Source::default(),
            toast: None,
            shown: Vec::new(),
            now: None,
            up_next: Vec::new(),
            playing: false,
            anchor_ms: 0,
            anchor_at: 0.0,
            clock: 0.0,
            second: 0,
            shuffle: false,
            prefs: cadence::storage::AppPreferences::default(),
            prefs_loaded: false,
            tray: None,
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
            Page::Album(id) => self
                .opened
                .get(&format!("album:{id}"))
                .cloned()
                .unwrap_or_default(),
            Page::Artist(id) => self
                .opened
                .get(&format!("artist:{id}"))
                .cloned()
                .unwrap_or_default(),
            Page::Local => self.local_tracks.clone(),
            Page::Playlists | Page::Settings | Page::Home => Source::default(),
        }
    }

    /// Lists every song of the page.
    pub fn refresh(&mut self) {
        self.shown = (0..self.source().tracks.len()).collect();
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
        match &page {
            Page::Album(id) if !self.opened.contains_key(&format!("album:{id}")) => {
                self.opened.insert(format!("album:{id}"), Source::default());
                if let Some(bridge) = &self.bridge {
                    bridge.load_album(id.clone());
                }
            }
            Page::Artist(id) if !self.opened.contains_key(&format!("artist:{id}")) => {
                self.opened
                    .insert(format!("artist:{id}"), Source::default());
                if let Some(bridge) = &self.bridge {
                    bridge.load_artist(id.clone());
                }
            }
            _ => {}
        }
        self.menu = false;
        self.search.open = false;
        match &page {
            Page::Home => self.ensure_home(),
            Page::Local => {
                if let Some(bridge) = &self.bridge {
                    bridge.load_local(false);
                }
            }
            _ => {}
        }
        self.page = page;
        self.query = TextEdit::new("");
        self.search = search::Search::default();
        self.selected = None;
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

    pub fn ensure_home(&mut self) {
        if self.home.data.is_none() && !self.home.loading {
            if let Some(bridge) = &self.bridge {
                self.home.loading = true;
                bridge.load_home(self.home.range);
            }
        }
    }

    pub fn set_home_range(&mut self, range: cadence::model::HomeRange) {
        if self.home.range != range {
            self.home.range = range;
            self.home.data = None;
            self.home.loading = false;
            self.ensure_home();
        }
    }

    /// Opens or closes the lyrics panel, fetching lyrics for the song on.
    pub fn toggle_lyrics(&mut self) {
        self.lyrics.open = !self.lyrics.open;
        self.sync_lyrics();
    }

    /// Fetches lyrics when the panel is up and the song changed.
    pub fn sync_lyrics(&mut self) {
        let Some(now) = &self.now else { return };
        if !self.lyrics.open || self.lyrics.id.as_deref() == Some(now.source_id.as_str()) {
            return;
        }
        self.lyrics.id = Some(now.source_id.clone());
        self.lyrics.data = None;
        self.lyrics.line = None;
        if let Some(bridge) = &self.bridge {
            self.lyrics.loading = true;
            bridge.load_lyrics(
                now.source_id.clone(),
                cadence::lyrics::Query {
                    title: now.title.to_string(),
                    artist: now.artist.to_string(),
                    album: now.album.to_string(),
                    duration_ms: now.duration_ms,
                },
            );
        }
    }

    pub fn open_credits(&mut self, track: Track) {
        self.track_menu = None;
        let mut state = CreditsState {
            track: track.clone(),
            data: None,
            loading: false,
        };
        if let (Some(isrc), Some(bridge)) = (track.isrc.clone(), &self.bridge) {
            state.loading = true;
            bridge.load_credits(track.source_id.clone(), isrc);
        }
        self.credits = Some(state);
    }

    pub fn open_track_menu(&mut self, track: Track, y: f32) {
        self.track_menu = Some(TrackMenu { track, y });
    }

    pub fn toast(&mut self, text: &str) {
        self.toast = Some((text.to_owned(), self.clock + 2.2));
    }

    pub fn play_next(&mut self, track: Track) {
        self.track_menu = None;
        self.send(BackendCommand::PlayNext(track));
        self.toast("Playing next");
    }

    pub fn add_to_queue(&mut self, track: Track) {
        self.track_menu = None;
        self.send(BackendCommand::AppendToQueue(track));
        self.toast("Added to queue");
    }

    /// Jumps to `fraction` of the current song.
    pub fn seek_to(&mut self, fraction: f32) {
        let Some(now) = &self.now else { return };
        let ms = (now.duration_ms as f32 * fraction.clamp(0.0, 1.0)) as u32;
        self.anchor_ms = ms;
        self.anchor_at = self.clock;
        self.second = ms / 1000;
        self.send(BackendCommand::Seek(ms));
    }

    /// Puts the saved preferences to work: the audio switches, then volume and shuffle.
    fn apply_prefs(&mut self, p: cadence::storage::AppPreferences) {
        self.prefs = p;
        self.sync_tray();
        cadence::playback::set_normalisation(p.normalise_volume);
        cadence::playback::set_streaming_kbps(p.streaming_quality.kbps());
        cadence::playback::set_engine(p.player);
        cadence::equalizer::publish(p.equalizer, false);
        self.volume = f32::from(p.volume_percent) / 100.0;
        self.send(BackendCommand::SetVolume(self.volume));
        if p.shuffle != self.shuffle {
            self.send(BackendCommand::SetShuffle(p.shuffle));
        }
    }

    /// Keeps the tray icon in step with the "keep in tray" preference.
    pub fn sync_tray(&mut self) {
        if self.prefs.keep_in_tray && self.tray.is_none() {
            self.tray = shell::start_tray();
        } else if !self.prefs.keep_in_tray {
            self.tray = None;
        }
    }

    /// Gives back what a hidden window does not need: decoded covers, then Windows' pages.
    pub fn release_memory(&mut self) {
        if let Some(art) = self.art.as_mut() {
            art.release();
        }
        memory::trim();
    }

    /// Applies an equalizer change at once and saves it.
    pub fn set_equalizer(&mut self, eq: cadence::equalizer::Settings) {
        self.prefs.equalizer = eq;
        cadence::equalizer::publish(eq, false);
        if let Some(b) = &self.bridge {
            b.save_pref(move |s| {
                let _ = s.set_equalizer(eq);
            });
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.send(BackendCommand::SetVolume(self.volume));
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
            BackendEvent::SetupRequired => {
                self.conn = Conn::Setup;
                // A reset sends the listener back through setup.
                self.loaded = false;
            }
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
            BackendEvent::LoggedOut => {
                // Back to the sign-in screen with nothing of the last account left.
                let bridge = self.bridge.take();
                let (art, icons) = (self.art.take(), self.icons.take());
                *self = State {
                    bridge,
                    art,
                    icons,
                    conn: Conn::Login,
                    clock: self.clock,
                    ..State::default()
                };
            }
            BackendEvent::AuthorizationRequired => {
                if self.conn != Conn::Connecting {
                    self.conn = Conn::Login;
                }
            }
            BackendEvent::ProfileLoaded { profile, .. } => {
                self.profile = Some(profile);
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
                recently_played,
                pinned_playlists,
                ..
            } => {
                self.pinned = pinned_playlists.into_iter().map(|p| p.source_id).collect();
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
                Msg::Album { id, result } => {
                    let key = format!("album:{id}");
                    match result {
                        Some((album, tracks)) => {
                            let artists: Vec<_> =
                                album.artists.iter().map(|a| a.name.to_string()).collect();
                            let year = album.release_date.as_deref().unwrap_or("").get(..4);
                            let mut sub = artists.join(", ");
                            if let Some(year) = year {
                                sub = format!("{sub} · {year}");
                            }
                            sub = format!("{sub} · {} songs", tracks.len());
                            self.details.insert(
                                id.clone(),
                                Detail {
                                    title: album.name.to_string(),
                                    sub,
                                    art: album.artwork_url.clone(),
                                    round: false,
                                },
                            );
                            self.opened.insert(
                                key,
                                Source {
                                    total: tracks.len() as u32,
                                    done: true,
                                    tracks: tracks.into(),
                                    error: None,
                                },
                            );
                        }
                        None => {
                            self.opened.entry(key).or_default().error =
                                Some("The album could not be loaded.".into());
                        }
                    }
                    self.refresh();
                }
                Msg::Artist { id, result } => {
                    let key = format!("artist:{id}");
                    match result {
                        Some((artist, tracks, albums)) => {
                            self.artist_albums.insert(id.clone(), albums.albums);
                            self.details.insert(
                                id.clone(),
                                Detail {
                                    title: artist.name.to_string(),
                                    sub: "Popular songs".into(),
                                    art: artist.artwork_url.clone(),
                                    round: true,
                                },
                            );
                            self.opened.insert(
                                key,
                                Source {
                                    total: tracks.len() as u32,
                                    done: true,
                                    tracks: tracks.into(),
                                    error: None,
                                },
                            );
                        }
                        None => {
                            self.opened.entry(key).or_default().error =
                                Some("The artist could not be loaded.".into());
                        }
                    }
                    self.refresh();
                }
                Msg::Lyrics { id, result } => {
                    if self.lyrics.id.as_deref() == Some(id.as_str()) {
                        self.lyrics.loading = false;
                        self.lyrics.data = result;
                    }
                }
                Msg::Credits { id, result } => {
                    if let Some(c) = self.credits.as_mut() {
                        if c.track.source_id == id {
                            c.loading = false;
                            c.data = result;
                        }
                    }
                }
                Msg::Home { range, result } => {
                    if range == self.home.range {
                        self.home.loading = false;
                        if result.is_some() {
                            self.home.data = result;
                        }
                    }
                }
                Msg::Prefs(p) => self.apply_prefs(p),
                Msg::Folders(folders) => {
                    self.local.folders = folders;
                    self.local.loaded = true;
                }
                Msg::Local(library) => {
                    let library = library.unwrap_or_default();
                    self.local.unreadable = library.unreadable;
                    self.local.truncated = library.truncated;
                    self.local_tracks = Source {
                        total: library.tracks.len() as u32,
                        done: true,
                        tracks: library.tracks.into(),
                        error: None,
                    };
                    self.refresh();
                }
                Msg::Search { query, results } => self.on_search_results(query, results),
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
    s.sync_lyrics();
    for command in shell::take_commands() {
        match command.as_str() {
            "toggle" => s.toggle_play(),
            "next" => s.send(BackendCommand::Next),
            "previous" => s.send(BackendCommand::Previous),
            _ => {}
        }
    }
    if s.loaded && s.page == Page::Home {
        s.ensure_home();
    }
    if s.loaded && !s.prefs_loaded {
        s.prefs_loaded = true;
        if let Some(bridge) = &s.bridge {
            bridge.load_prefs();
        }
    }
    if s.loaded && !s.local.loaded {
        s.local.loaded = true;
        if let Some(bridge) = &s.bridge {
            bridge.load_local(false);
        }
    }
    if let Some((_, until)) = &s.toast {
        if clock >= *until {
            s.toast = None;
            changed = true;
        } else {
            tx.wake_after(Duration::from_millis(300));
        }
    }
    // The sung line moves on its own timing, not the progress bar's.
    if s.lyrics.open {
        let line = s
            .lyrics
            .data
            .as_ref()
            .and_then(|l| l.current(s.progress_ms()));
        if line != s.lyrics.line {
            s.lyrics.line = line;
            changed = true;
        }
    }
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
    if s.tick_search() {
        tx.wake_after(Duration::from_millis(100));
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

fn on_close(s: &mut State, tx: &mut Tx) {
    if s.tray.is_some() && s.prefs.keep_in_tray {
        s.release_memory();
    } else {
        tx.quit();
    }
}

/// The window is hidden, minimised or has drawn nothing for a while.
fn on_rest(s: &mut State, tx: &mut Tx) -> bool {
    if !tx.window_visible() {
        s.release_memory();
    } else {
        memory::trim();
    }
    false
}

fn main() {
    memory::gpu_policy();
    memory::start();
    // Read the saved preferences first: they decide whether closing keeps Sonder in the tray.
    let early = cadence::storage::Store::open_default()
        .ok()
        .and_then(|s| s.preferences().ok())
        .unwrap_or_default();
    let quiet = early.keep_in_tray && shell::launched_at_startup();
    let spec = AppSpec::<State> {
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
    };
    let mut app = rune::App::new(spec)
        .args(shell::rune_args())
        .keep_running()
        .release_gpu_when_closed()
        .on_close(on_close)
        .on_rest(on_rest)
        .init(move || {
            let mut s = State::default();
            s.prefs = early;
            s.sync_tray();
            s
        });
    if quiet {
        app = app.windowless();
    }
    shell::set_handle(app.handle());
    app.run();
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
    fn a_library_arriving_is_ready_to_show_and_remembers_what_is_liked() {
        let mut s = State::default();
        assert!(!s.loaded);
        s.apply(BackendEvent::CachedLibrary {
            generation: 1,
            liked_tracks: (0..3).map(sample::track).collect(),
            playlists: Vec::new(),
        });
        assert!(s.loaded && s.conn == Conn::Ready);
        s.page = Page::Liked;
        s.refresh();
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
