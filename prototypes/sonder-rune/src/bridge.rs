//! The seam between Sonder's backend (threads, Tokio, Spotify) and Rune's UI thread.
//!
//! The backend talks in commands and events. Everything it says is copied into one
//! `std::sync::mpsc` queue and the window is woken; the UI drains the queue in
//! `poll`. Nothing here blocks the UI thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use cadence::backend::{Backend, BackendCommand, BackendEvent, BackendHandle};
use cadence::model::{Playlist, Track};
use rune::Waker;

/// Everything the UI thread hears about.
pub enum Msg {
    Event(BackendEvent),
    /// The saved preferences, read once at start.
    Prefs(cadence::storage::AppPreferences),
    /// One page of a playlist's songs; `done` once the last page arrived.
    PlaylistPage {
        id: String,
        tracks: Vec<Track>,
        total: u32,
        done: bool,
    },
    PlaylistFailed {
        id: String,
        error: String,
    },
    /// Spotify's answer to a like or unlike: the song and whether it now stands.
    LikeResult {
        source_id: String,
        liked: bool,
        ok: bool,
    },
    Album {
        id: String,
        result: Option<cadence::backend::AlbumDetails>,
    },
    Artist {
        id: String,
        result: Option<cadence::backend::ArtistDetails>,
    },
    Lyrics {
        id: String,
        result: Option<cadence::lyrics::Lyrics>,
    },
    Credits {
        id: String,
        result: Option<cadence::credits::Credits>,
    },
    Home {
        range: cadence::model::HomeRange,
        result: Option<cadence::model::HomeData>,
    },
    /// Local Files: the folders the listener added, then what is in them.
    Folders(Vec<std::path::PathBuf>),
    Local(Option<cadence::local_files::LocalLibrary>),
    /// Spotify's catalog answer for a search.
    Search {
        query: String,
        results: Option<cadence::model::CatalogSearchResults>,
    },
    /// An album cover's bytes, fetched off-thread.
    Art {
        url: String,
        bytes: Option<Vec<u8>>,
    },
}

pub struct Bridge {
    /// Dropping the backend stops playback, so the bridge owns it for the process.
    _backend: Backend,
    pub handle: BackendHandle,
    pub inbox: Receiver<Msg>,
    outbox: Sender<Msg>,
    waker: Waker,
}

impl Bridge {
    pub fn start(waker: Waker) -> Bridge {
        let (backend, mut events) = Backend::start();
        let handle = backend.handle();
        let (outbox, inbox) = channel();

        let (tx, wake) = (outbox.clone(), waker.clone());
        thread::Builder::new()
            .name("sonder-events".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("could not start Tokio");
                runtime.block_on(async move {
                    while let Some(event) = events.recv().await {
                        if tx.send(Msg::Event(event)).is_err() {
                            break;
                        }
                        wake();
                    }
                });
            })
            .expect("could not start the event thread");

        Bridge {
            _backend: backend,
            handle,
            inbox,
            outbox,
            waker,
        }
    }

    pub fn send(&self, command: BackendCommand) -> bool {
        self.handle.send(command)
    }

    /// Loads every page of a playlist, reporting each as it arrives so a long one
    /// shows its first rows at once.
    pub fn load_playlist(&self, playlist: Playlist) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let id = playlist.source_id.clone();
            let mut offset = 0;
            loop {
                let (respond, reply) = tokio::sync::oneshot::channel();
                let sent = handle.send(BackendCommand::LoadPlaylist {
                    playlist: playlist.clone(),
                    offset,
                    respond,
                });
                let answer = if sent {
                    reply.blocking_recv().ok()
                } else {
                    None
                };
                match answer {
                    Some(Ok(page)) => {
                        let next = page.next_offset;
                        let _ = tx.send(Msg::PlaylistPage {
                            id: id.clone(),
                            tracks: page.tracks,
                            total: page.total,
                            done: next.is_none(),
                        });
                        wake();
                        match next {
                            Some(next) => offset = next,
                            None => break,
                        }
                    }
                    other => {
                        let error = match other {
                            Some(Err(error)) => error.to_string(),
                            _ => "The playlist could not be loaded.".to_owned(),
                        };
                        let _ = tx.send(Msg::PlaylistFailed { id, error });
                        wake();
                        break;
                    }
                }
            }
        });
    }

    /// Likes or unlikes a song on Spotify and reports whether it took.
    pub fn set_liked(&self, track: Track, liked: bool) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let source_id = track.source_id.clone();
            let (respond, reply) = tokio::sync::oneshot::channel();
            let sent = handle.send(BackendCommand::SetSpotifyLiked {
                track,
                liked,
                respond,
            });
            let ok = sent && matches!(reply.blocking_recv(), Ok(Ok(())));
            let _ = tx.send(Msg::LikeResult {
                source_id,
                liked,
                ok,
            });
            wake();
        });
    }

    /// Sends a command that wants an answer and turns the reply into a message.
    fn ask<T: Send + 'static>(
        &self,
        command: impl FnOnce(cadence::backend::Reply<T>) -> BackendCommand + Send + 'static,
        done: impl FnOnce(Option<T>) -> Msg + Send + 'static,
    ) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let (respond, reply) = tokio::sync::oneshot::channel();
            let result = if handle.send(command(respond)) {
                reply.blocking_recv().ok().and_then(Result::ok)
            } else {
                None
            };
            let _ = tx.send(done(result));
            wake();
        });
    }

    pub fn load_lyrics(&self, id: String, query: cadence::lyrics::Query) {
        self.ask(
            move |respond| BackendCommand::LoadLyrics { query, respond },
            move |result| Msg::Lyrics { id, result },
        );
    }

    pub fn load_credits(&self, id: String, isrc: String) {
        self.ask(
            move |respond| BackendCommand::LoadCredits { isrc, respond },
            move |result| Msg::Credits { id, result },
        );
    }

    pub fn load_home(&self, range: cadence::model::HomeRange) {
        self.ask(
            move |respond| BackendCommand::LoadHome { range, respond },
            move |result| Msg::Home { range, result },
        );
    }

    /// Reads the saved preferences once.
    pub fn load_prefs(&self) {
        let (tx, wake) = (self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            if let Some(p) = cadence::storage::Store::open_default()
                .ok()
                .and_then(|s| s.preferences().ok())
            {
                let _ = tx.send(Msg::Prefs(p));
                wake();
            }
        });
    }

    /// Saves one preference off the UI thread.
    pub fn save_pref(&self, f: impl FnOnce(&mut cadence::storage::Store) + Send + 'static) {
        thread::spawn(move || {
            if let Ok(mut store) = cadence::storage::Store::open_default() {
                f(&mut store);
            }
        });
    }

    /// Reads the saved Local Files folders, then (with any) scans them.
    pub fn load_local(&self, rescan: bool) {
        let me = self.clone_for_local();
        let wake = self.waker.clone();
        thread::spawn(move || {
            me.edit_folders(rescan, |_| {});
            wake();
        });
    }

    /// Asks for a folder, adds it to Local Files and scans.
    pub fn add_local_folder(&self) {
        let (me, wake) = (self.clone_for_local(), self.waker.clone());
        thread::spawn(move || {
            let Some(folder) = rfd::FileDialog::new()
                .set_title("Add to Local Files")
                .pick_folder()
            else {
                return;
            };
            me.edit_folders(true, |folders| {
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
            });
            wake();
        });
    }

    pub fn remove_local_folder(&self, folder: std::path::PathBuf) {
        let (me, wake) = (self.clone_for_local(), self.waker.clone());
        thread::spawn(move || {
            me.edit_folders(true, |folders| folders.retain(|f| f != &folder));
            wake();
        });
    }

    fn clone_for_local(&self) -> LocalEdit {
        LocalEdit {
            handle: self.handle.clone(),
            tx: self.outbox.clone(),
        }
    }

    pub fn load_album(&self, id: String) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let (respond, reply) = tokio::sync::oneshot::channel();
            let sent = handle.send(BackendCommand::LoadAlbum {
                source_id: id.clone(),
                respond,
            });
            let result = if sent {
                reply.blocking_recv().ok().and_then(Result::ok)
            } else {
                None
            };
            let _ = tx.send(Msg::Album { id, result });
            wake();
        });
    }

    pub fn load_artist(&self, id: String) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let (respond, reply) = tokio::sync::oneshot::channel();
            let sent = handle.send(BackendCommand::LoadArtist {
                source_id: id.clone(),
                respond,
            });
            let result = if sent {
                reply.blocking_recv().ok().and_then(Result::ok)
            } else {
                None
            };
            let _ = tx.send(Msg::Artist { id, result });
            wake();
        });
    }

    /// Searches Spotify's catalog; the answer comes back as `Msg::Search`.
    pub fn search(&self, query: String) {
        let (handle, tx, wake) = (self.handle.clone(), self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let (respond, reply) = tokio::sync::oneshot::channel();
            let sent = handle.send(BackendCommand::SearchCatalog {
                query: query.clone(),
                respond,
            });
            let results = if sent {
                reply.blocking_recv().ok().and_then(Result::ok)
            } else {
                None
            };
            let _ = tx.send(Msg::Search { query, results });
            wake();
        });
    }

    /// Fetches an album cover. Four at a time at most; the rest wait their turn.
    pub fn fetch_art(&self, url: String) {
        // Two at a time, like Sonder's two image workers.
        static GATE: Gate = Gate::new(2);
        let (tx, wake) = (self.outbox.clone(), self.waker.clone());
        thread::spawn(move || {
            let _slot = GATE.enter();
            let bytes = crate::disk::load(&url).or_else(|| {
                let bytes = agent().get(&url).call().ok().and_then(|r| {
                    r.into_body()
                        .with_config()
                        .limit(4 << 20)
                        .read_to_vec()
                        .ok()
                })?;
                crate::disk::store(&url, &bytes);
                Some(bytes)
            });
            let _ = tx.send(Msg::Art { url, bytes });
            wake();
        });
    }
}

/// What a folder change needs: the backend to rescan and the queue to report on.
struct LocalEdit {
    handle: BackendHandle,
    tx: Sender<Msg>,
}

impl LocalEdit {
    /// Applies `change` to the saved folders, reports them, then reads the index
    /// (scanning first when `rescan`). Without folders there is nothing to read.
    fn edit_folders(self, rescan: bool, change: impl FnOnce(&mut Vec<std::path::PathBuf>)) {
        let Ok(mut storage) = cadence::storage::Store::open_default() else {
            return;
        };
        let mut folders = storage.local_folders().unwrap_or_default();
        let before = folders.clone();
        change(&mut folders);
        if folders != before {
            let _ = storage.set_local_folders(&folders);
        }
        let _ = self.tx.send(Msg::Folders(folders.clone()));
        if folders.is_empty() {
            let _ = self.tx.send(Msg::Local(None));
            return;
        }
        let (respond, reply) = tokio::sync::oneshot::channel();
        let library = if self.handle.send(BackendCommand::LoadLocalFiles {
            folders,
            rescan,
            respond,
        }) {
            reply.blocking_recv().ok().and_then(Result::ok)
        } else {
            None
        };
        let _ = self.tx.send(Msg::Local(library));
    }
}

/// One HTTP client for every cover, so its TLS setup and sockets are built once.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(ureq::Agent::new_with_defaults)
}

/// A tiny counting semaphore, so a screenful of covers does not open a screenful of sockets.
struct Gate {
    free: std::sync::Mutex<usize>,
    freed: std::sync::Condvar,
}

struct Slot<'a>(&'a Gate);

impl Gate {
    const fn new(limit: usize) -> Gate {
        Gate {
            free: std::sync::Mutex::new(limit),
            freed: std::sync::Condvar::new(),
        }
    }
    fn enter(&self) -> Slot<'_> {
        let mut free = self
            .free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *free == 0 {
            free = self
                .freed
                .wait(free)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *free -= 1;
        Slot(self)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self
            .0
            .free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        self.0.freed.notify_one();
    }
}
