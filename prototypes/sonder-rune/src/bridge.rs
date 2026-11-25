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
