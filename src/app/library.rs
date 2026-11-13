use super::*;

use std::path::{Path, PathBuf};

/// Debounce between revalidations, so rapid window switches coalesce. Short
/// on purpose: a revalidation is a two-request head probe unless something
/// changed, and Spotify's rate limit is a rolling 30-second window. Wall
/// clock rather than `Instant`, which stops while the machine sleeps.
/// Coming back to the window re-checks the library at most this often; each
/// check is two Spotify requests.
const REVALIDATION_DEBOUNCE: Duration = Duration::from_secs(5 * 60);

/// Opening Local Files rescans the folders at most this often. A rescan of
/// unchanged folders only reads file sizes and dates.
const LOCAL_RESCAN_DEBOUNCE: Duration = Duration::from_secs(60);
/// How long folders stay quiet before a change is scanned, so a copy or a
/// tag editor finishes first and a burst of changes needs one scan.
const LOCAL_CHANGE_SETTLE: Duration = Duration::from_millis(1500);

/// Everything Sonder knows about the listener's music: what Spotify holds for
/// the account, and what Sonder keeps locally about it.
///
/// Owned by the services global so a window can be rebuilt without refetching.
pub(super) struct Library {
    backend: BackendHandle,
    liked_tracks: Arc<[model::Track]>,
    playlists: Arc<[model::Playlist]>,
    loaded: bool,
    favorites: Arc<[model::Track]>,
    pinned_playlists: Arc<[model::Playlist]>,
    recently_played: Arc<[model::Track]>,
    local_loaded: bool,
    reload: Option<gpui_kit::Task<()>>,
    spotify_like_request: Option<gpui_kit::Task<()>>,
    /// The backend is revalidating the cached contents it served at boot.
    boot_refreshing: bool,
    /// When the contents last arrived, so returning to the window repeatedly
    /// does not refetch the whole library every time.
    refreshed_at: Option<SystemTime>,
    local: LocalFiles,
}

/// Music from folders on this computer. Empty, and never scanned, until the
/// listener adds a folder.
#[derive(Default)]
struct LocalFiles {
    folders: Arc<[PathBuf]>,
    tracks: Arc<[model::Track]>,
    unreadable: usize,
    truncated: bool,
    /// The saved index or a scan has answered at least once.
    loaded: bool,
    /// Dropping it stops the backend's walk.
    scan: Option<gpui_kit::Task<()>>,
    scanned_at: Option<SystemTime>,
    /// The folders changed while a scan was running; scan again after it.
    rescan_queued: bool,
    /// Owns the folder watcher; dropping it stops watching.
    watch: Option<gpui_kit::Task<()>>,
}

/// Raised when fresh contents arrived, so stale failures can be cleared.
pub(super) struct LibraryLoaded;

pub(super) struct SpotifyLikeNotice(pub SharedString);
impl EventEmitter<SpotifyLikeNotice> for Library {}

impl EventEmitter<LibraryLoaded> for Library {}

impl Library {
    pub(super) fn new(backend: BackendHandle) -> Self {
        Self {
            backend,
            liked_tracks: Arc::default(),
            playlists: Arc::default(),
            loaded: false,
            favorites: Arc::default(),
            pinned_playlists: Arc::default(),
            recently_played: Arc::default(),
            local_loaded: false,
            reload: None,
            spotify_like_request: None,
            boot_refreshing: false,
            refreshed_at: None,
            local: LocalFiles::default(),
        }
    }

    pub(super) fn reloading(&self) -> bool {
        self.reload.is_some() || self.boot_refreshing
    }

    /// Refetches the library, leaving the current contents visible until the
    /// answer arrives. Does nothing when one is already running; the backend
    /// additionally answers Unchanged while the boot load owns the first
    /// fetch, so this cannot race it into a doubled walk.
    pub(super) fn revalidate(&mut self, cx: &mut Context<Self>) {
        let fresh = self.refreshed_at.is_some_and(|refreshed_at| {
            refreshed_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < REVALIDATION_DEBOUNCE)
        });
        if self.spotify_like_request.is_some() || self.reload.is_some() || fresh {
            return;
        }
        let (respond, reply) = tokio::sync::oneshot::channel();
        if !self.backend.send(BackendCommand::ReloadLibrary { respond }) {
            return;
        }
        self.reload = Some(cx.spawn(async move |this, cx| {
            let contents = reply.await;
            let _ = this.update(cx, |library, cx| {
                library.reload = None;
                match contents {
                    Ok(Ok(LibraryReload::Fresh((liked_tracks, playlists)))) => {
                        library.liked_tracks = liked_tracks.into();
                        library.playlists = playlists.into();
                        library.loaded = true;
                        library.refreshed_at = Some(SystemTime::now());
                        cx.emit(LibraryLoaded);
                    }
                    // The head probes matched: the contents on screen are the
                    // contents on Spotify.
                    Ok(Ok(LibraryReload::Unchanged)) => {
                        library.refreshed_at = Some(SystemTime::now());
                    }
                    Ok(Err(error)) => match spotify::classify_error(&error) {
                        // The gate holds automatic refreshes back; the next
                        // timer or activation retries after the cooldown.
                        spotify::ErrorKind::RateLimited { .. } => {
                            log::info!("library: rate limited; refresh deferred")
                        }
                        _ => log::warn!("library: refresh failed: {error:#}"),
                    },
                    Err(_) => log::warn!("library: backend stopped before answering"),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn liked_tracks(&self) -> &Arc<[model::Track]> {
        &self.liked_tracks
    }

    pub(super) fn playlists(&self) -> &Arc<[model::Playlist]> {
        &self.playlists
    }

    pub(super) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(super) fn favorites(&self) -> &Arc<[model::Track]> {
        &self.favorites
    }

    pub(super) fn pinned_playlists(&self) -> &Arc<[model::Playlist]> {
        &self.pinned_playlists
    }

    pub(super) fn recently_played(&self) -> &Arc<[model::Track]> {
        &self.recently_played
    }

    pub(super) fn local_loaded(&self) -> bool {
        self.local_loaded
    }

    /// Whether the listener uses Local Files: at least one folder is added.
    pub(super) fn has_local_files(&self) -> bool {
        !self.local.folders.is_empty()
    }

    pub(super) fn local_folders(&self) -> &Arc<[PathBuf]> {
        &self.local.folders
    }

    pub(super) fn local_tracks(&self) -> &Arc<[model::Track]> {
        &self.local.tracks
    }

    pub(super) fn local_files_loaded(&self) -> bool {
        self.local.loaded
    }

    pub(super) fn local_files_scanning(&self) -> bool {
        self.local.scan.is_some()
    }

    /// Music files found in the folders that could not be read.
    pub(super) fn local_unreadable(&self) -> usize {
        self.local.unreadable
    }

    /// The folders hold more than Local Files lists.
    pub(super) fn local_truncated(&self) -> bool {
        self.local.truncated
    }

    /// Starts Local Files with the saved folders: the saved index first, so
    /// the page fills at once, then a scan for what changed while closed.
    pub(super) fn open_local_files(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.local.folders = folders.into();
        self.watch_local_folders(cx);
        self.scan_local_files(true, cx);
    }

    /// Rescans when Local Files is opened, unless a scan just ran.
    pub(super) fn refresh_local_files(&mut self, cx: &mut Context<Self>) {
        let recent = self.local.scanned_at.is_some_and(|scanned_at| {
            scanned_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < LOCAL_RESCAN_DEBOUNCE)
        });
        if self.has_local_files() && self.local.scan.is_none() && !recent {
            self.scan_local_files(false, cx);
        }
    }

    /// Adds folders the listener picked, ignoring ones already added, saves
    /// them and scans. Returns the save error, if any.
    pub(super) fn add_local_folders(
        &mut self,
        picked: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<SharedString> {
        let mut folders = self.local.folders.to_vec();
        for folder in picked {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        if folders.len() == self.local.folders.len() {
            return None;
        }
        self.set_local_folders(folders, cx)
    }

    pub(super) fn remove_local_folder(
        &mut self,
        folder: &Path,
        cx: &mut Context<Self>,
    ) -> Option<SharedString> {
        let folders: Vec<_> = self
            .local
            .folders
            .iter()
            .filter(|known| known.as_path() != folder)
            .cloned()
            .collect();
        self.set_local_folders(folders, cx)
    }

    fn set_local_folders(
        &mut self,
        folders: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<SharedString> {
        let saved = services::require_settings_storage(services::AppServices::set_local_folders(
            &folders, cx,
        ));
        if let Err(error) = saved {
            return Some(format!("Could not save Local Files folders: {error}").into());
        }
        self.local.folders = folders.into();
        self.watch_local_folders(cx);
        if self.local.folders.is_empty() {
            // Nothing to show; the scan below also empties the saved index.
            self.local.tracks = Arc::default();
            self.local.unreadable = 0;
            self.local.truncated = false;
        }
        self.scan_local_files(false, cx);
        None
    }

    /// Watches the current folders, replacing any earlier watch. The watcher
    /// is made off the UI thread, since some systems walk every subfolder.
    fn watch_local_folders(&mut self, cx: &mut Context<Self>) {
        self.local.watch = None;
        if self.local.folders.is_empty() {
            return;
        }
        let folders = self.local.folders.to_vec();
        let (changed, changes) = async_channel::bounded::<()>(1);
        self.local.watch = Some(cx.spawn(async move |this, cx| {
            let watcher = cx
                .background_executor()
                .spawn(async move {
                    crate::local_files::FolderWatcher::new(&folders, move || {
                        // A full channel already holds a pending change.
                        let _ = changed.try_send(());
                    })
                })
                .await;
            let Some(_watcher) = watcher else {
                return;
            };
            while changes.recv().await.is_ok() {
                cx.background_executor().timer(LOCAL_CHANGE_SETTLE).await;
                while changes.try_recv().is_ok() {}
                if this
                    .update(cx, |library, cx| library.rescan_after_change(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// Something changed in the folders: scan now, or right after the scan
    /// that is running, which may have walked past the change.
    pub(super) fn rescan_after_change(&mut self, cx: &mut Context<Self>) {
        if !self.has_local_files() {
            return;
        }
        if self.local.scan.is_some() {
            self.local.rescan_queued = true;
        } else {
            self.scan_local_files(false, cx);
        }
    }

    /// Replaces any scan in flight, whose folders are now out of date.
    fn scan_local_files(&mut self, saved_first: bool, cx: &mut Context<Self>) {
        self.local.rescan_queued = false;
        let backend = self.backend.clone();
        let folders = self.local.folders.to_vec();
        self.local.scan = Some(cx.spawn(async move |this, cx| {
            let ask = |rescan: bool| {
                let (respond, reply) = tokio::sync::oneshot::channel();
                backend
                    .send(BackendCommand::LoadLocalFiles {
                        folders: folders.clone(),
                        rescan,
                        respond,
                    })
                    .then_some(reply)
            };
            if saved_first && let Some(reply) = ask(false) {
                let saved = reply.await;
                let _ = this.update(cx, |library, cx| {
                    if let Ok(Ok(saved)) = saved {
                        library.show_local_files(saved, cx);
                    }
                });
            }
            let scanned = match ask(true) {
                Some(reply) => reply.await.ok(),
                None => None,
            };
            let _ = this.update(cx, |library, cx| {
                library.local.scan = None;
                library.local.scanned_at = Some(SystemTime::now());
                match scanned {
                    Some(Ok(scanned)) => library.show_local_files(scanned, cx),
                    Some(Err(error)) => log::warn!("local files: scan failed: {error:#}"),
                    None => log::warn!("local files: backend stopped before answering"),
                }
                library.local.loaded = true;
                if std::mem::take(&mut library.local.rescan_queued) {
                    library.scan_local_files(false, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn show_local_files(
        &mut self,
        found: crate::local_files::LocalLibrary,
        cx: &mut Context<Self>,
    ) {
        self.local.loaded = true;
        self.local.unreadable = found.unreadable;
        self.local.truncated = found.truncated;
        // An unchanged rescan keeps the same Arc, so the list keeps its rows
        // and scroll position without rebuilding.
        if self.local.tracks.as_ref() != found.tracks.as_slice() {
            self.local.tracks = found.tracks.into();
        }
        cx.notify();
    }

    pub(super) fn is_playlist_pinned(&self, playlist: &model::Playlist) -> bool {
        self.pinned_playlists.iter().any(|candidate| {
            candidate.provider == playlist.provider && candidate.source_id == playlist.source_id
        })
    }

    /// Marks the catalog as settled without contents, for when the fetch failed.
    pub(super) fn mark_loaded(&mut self, cx: &mut Context<Self>) {
        self.loaded = true;
        self.boot_refreshing = false;
        cx.notify();
    }

    pub(super) fn is_spotify_liked(&self, track: &model::Track) -> bool {
        self.liked_tracks.iter().any(|candidate| {
            candidate.provider == track.provider && candidate.source_id == track.source_id
        })
    }

    /// Serialize writes and refreshes, keeping one bounded click-driven task.
    pub(super) fn spotify_like_busy(&self) -> bool {
        !self.loaded || self.reloading() || self.spotify_like_request.is_some()
    }

    pub(super) fn set_spotify_liked(
        &mut self,
        track: model::Track,
        liked: bool,
        cx: &mut Context<Self>,
    ) {
        if track.provider != model::Provider::Spotify || self.spotify_like_busy() {
            return;
        }
        let (respond, reply) = tokio::sync::oneshot::channel();
        if !self.backend.send(BackendCommand::SetSpotifyLiked {
            track: track.clone(),
            liked,
            respond,
        }) {
            cx.emit(SpotifyLikeNotice(BACKEND_UNAVAILABLE.into()));
            return;
        }
        self.spotify_like_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |library, cx| {
                library.spotify_like_request = None;
                match result {
                    Ok(Ok(())) => {
                        let mut tracks = Vec::with_capacity(library.liked_tracks.len() + usize::from(liked));
                        if liked { tracks.push(track.clone()); }
                        tracks.extend(library.liked_tracks.iter().filter(|candidate| candidate.provider != track.provider || candidate.source_id != track.source_id).cloned());
                        library.liked_tracks = tracks.into();
                        library.refreshed_at = Some(SystemTime::now());
                        cx.emit(LibraryLoaded);
                    }
                    Ok(Err(error)) => cx.emit(SpotifyLikeNotice(format!("Spotify Liked Songs: {error:#}").into())),
                    Err(_) => cx.emit(SpotifyLikeNotice("Spotify library update did not finish. Check your connection and try again.".into())),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn set_playlist_pinned(
        &mut self,
        playlist: model::Playlist,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        cx.notify();
        self.backend
            .send(BackendCommand::SetPlaylistPinned { playlist, pinned })
    }

    /// Forgets the account's catalog. Locally-owned state stays: it is not tied
    /// to the Spotify account and the backend re-sends it regardless.
    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.reload = None;
        self.spotify_like_request = None;
        self.boot_refreshing = false;
        self.refreshed_at = None;
        self.liked_tracks = Arc::default();
        self.playlists = Arc::default();
        self.loaded = false;
        cx.notify();
    }

    /// Applies the library half of a backend event, returning the event when the
    /// surrounding app still has its own work to do for it.
    pub(super) fn handle_backend_event(
        &mut self,
        event: BackendEvent,
        generation: u64,
        cx: &mut Context<Self>,
    ) -> Option<BackendEvent> {
        match event {
            BackendEvent::LibraryLoaded {
                generation: loaded_generation,
                liked_tracks,
                playlists,
            } => {
                if loaded_generation == generation {
                    self.liked_tracks = liked_tracks.into();
                    self.playlists = playlists.into();
                    self.loaded = true;
                    self.boot_refreshing = false;
                    self.refreshed_at = Some(SystemTime::now());
                    cx.emit(LibraryLoaded);
                }
            }
            // The cache is the library until Spotify says otherwise: usable,
            // shown as refreshing until the boot revalidation answers.
            BackendEvent::CachedLibrary {
                generation: cached_generation,
                liked_tracks,
                playlists,
            } => {
                if cached_generation == generation {
                    self.liked_tracks = liked_tracks.into();
                    self.playlists = playlists.into();
                    self.loaded = true;
                    self.boot_refreshing = true;
                    cx.emit(LibraryLoaded);
                }
            }
            BackendEvent::LibraryUnchanged {
                generation: unchanged_generation,
            } => {
                if unchanged_generation == generation {
                    self.boot_refreshing = false;
                    self.refreshed_at = Some(SystemTime::now());
                }
            }
            BackendEvent::LocalStateLoaded {
                favorites,
                pinned_playlists,
                recently_played,
            } => {
                self.favorites = favorites.into();
                self.pinned_playlists = pinned_playlists.into();
                self.recently_played = recently_played.into();
                self.local_loaded = true;
                cx.emit(LibraryLoaded);
            }
            event => return Some(event),
        }
        cx.notify();
        None
    }
}
