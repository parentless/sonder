//! The Home page. It starts with a greeting and shortcuts back into the
//! library, then what you were playing and what is new from artists you play.
//! Below that: two short rows (an artist to try next, found because they sound
//! like ones you play, and the next announced release).
//!
//! Who sounds like whom comes from ListenBrainz (see `similar.rs`), looked up
//! once every few days and kept on disk. Two small Spotify requests per time
//! range, made only when the page is opened and its data is more than half an
//! hour old. Nothing runs in the background.
use super::page::PageEvent;
use super::top_listening::{TopEvent, TopListening};
use super::*;
use gpui_kit::TestSupportExt as _;
use std::time::{Instant, SystemTime};

/// New releases move more slowly, and finding them costs a request per artist.
/// Announced dates change slowly, and each lookup costs MusicBrainz about 20 requests.
const UPCOMING_STALE_TIME: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASES_STALE_TIME: Duration = Duration::from_secs(6 * 60 * 60);
/// The backend keeps its own answer for days; this only avoids asking it on every visit.
const DISCOVER_STALE_TIME: Duration = Duration::from_secs(6 * 60 * 60);
/// Songs shown for the artist on offer.
const DISCOVER_SONGS: usize = 3;
/// Artists whose songs are kept, so stepping back and forth asks for nothing.
const DISCOVER_SONGS_KEPT: usize = 8;
/// The fewest and most new albums in a row.
const RELEASES_MIN: usize = 2;
const RELEASES_MAX: usize = 6;
/// A shelf cover's width and the space between covers, at 100% zoom.
const SHELF_COVER: f32 = 148.;
const SHELF_GAP: f32 = 12.;
/// The hue of the Liked Songs tile.
const LIKED_HUE: f32 = 340.;
/// Home reads the stretch Spotify calls medium term (about six months).
const RANGE: model::HomeRange = model::HomeRange::Months;

/// One way back into the library, drawn as a tile at the top of Home.
struct Shortcut {
    url: Option<String>,
    icon: CadenceIcon,
    /// The hue of a flat colour standing in for a cover, when there is none.
    tint: Option<f32>,
    round: bool,
    title: SharedString,
    detail: String,
    /// What a play button on the tile starts, when it can start at once.
    play: Option<Arc<[model::Track]>>,
    event: PageEvent,
}

/// What the greeting says at this hour of the day.
fn greeting_for(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=16 => "Good afternoon",
        17..=21 => "Good evening",
        _ => "Late night",
    }
}

/// Tests and screenshots always read the same words, whatever time it is.
fn greeting() -> &'static str {
    if cfg!(test) {
        return greeting_for(18);
    }
    greeting_for(crate::local_time::hour())
}

struct Upcoming {
    releases: Arc<[model::UpcomingRelease]>,
    loaded_at: SystemTime,
}

struct Discover {
    suggestions: Arc<[model::ArtistSuggestion]>,
    loaded_at: SystemTime,
}

struct Releases {
    albums: Arc<[model::Album]>,
    loaded_at: SystemTime,
}

pub(super) struct HomePage {
    backend: BackendHandle,
    session: Entity<session::Session>,
    top: Entity<TopListening>,
    releases: Option<Releases>,
    releases_loading: bool,
    releases_request: Option<gpui_kit::Task<()>>,
    upcoming: Option<Upcoming>,
    upcoming_loading: bool,
    upcoming_request: Option<gpui_kit::Task<()>>,
    discover: Option<Discover>,
    discover_loading: bool,
    discover_request: Option<gpui_kit::Task<()>>,
    /// Which suggestion is on offer.
    discover_index: usize,
    /// The songs of artists on offer, by Spotify id.
    discover_songs: Vec<(String, Arc<[model::Track]>)>,
    discover_songs_request: Option<gpui_kit::Task<()>>,
    error: Option<SharedString>,
    visible: bool,
    scroll: gpui_kit::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for HomePage {}

impl HomePage {
    pub(super) fn new(backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let session = services::AppServices::session(cx);
        let top = TopListening::shared(&backend, cx);
        let subscriptions = vec![
            cx.subscribe(&session, |this, _, event: &session::SessionEvent, cx| {
                this.handle_session_event(event, cx)
            }),
            cx.subscribe(&top, |this, _, event: &TopEvent, cx| {
                this.handle_top_event(event, cx)
            }),
            cx.observe(&top, |_, _, cx| cx.notify()),
            // The shortcuts and the song to continue follow the library and
            // the player.
            cx.observe(&services::AppServices::library(cx), |_, _, cx| cx.notify()),
            cx.observe(&services::AppServices::player(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            backend,
            session,
            top,
            releases: None,
            releases_loading: false,
            releases_request: None,
            upcoming: None,
            upcoming_loading: false,
            upcoming_request: None,
            discover: None,
            discover_loading: false,
            discover_request: None,
            discover_index: 0,
            discover_songs: Vec::new(),
            discover_songs_request: None,
            error: None,
            visible: false,
            scroll: gpui_kit::ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn handle_session_event(&mut self, event: &session::SessionEvent, cx: &mut Context<Self>) {
        match event {
            // A different account, or a fresh sign-in that may now carry the
            // permission: start over.
            session::SessionEvent::LoggedOut | session::SessionEvent::Restarted => self.reset(cx),
            session::SessionEvent::Ready => {
                self.reset(cx);
                self.top.update(cx, |top, cx| top.reset(cx));
                if self.visible {
                    self.load_if_needed(cx);
                    self.load_releases_if_needed(cx);
                    self.load_discover_if_needed(cx);
                }
            }
            _ => {}
        }
    }

    fn handle_top_event(&mut self, event: &TopEvent, cx: &mut Context<Self>) {
        match event {
            TopEvent::Loaded(range) if *range == RANGE => {
                self.error = None;
                cx.emit(PageEvent::Loaded);
            }
            TopEvent::Failed(range, error) if *range == RANGE => {
                self.error = Some(error.clone());
                cx.emit(PageEvent::Failed(error.clone()));
            }
            _ => {}
        }
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.releases = None;
        self.releases_loading = false;
        self.releases_request = None;
        self.upcoming = None;
        self.upcoming_loading = false;
        self.upcoming_request = None;
        self.discover = None;
        self.discover_loading = false;
        self.discover_request = None;
        self.discover_index = 0;
        self.discover_songs.clear();
        self.discover_songs_request = None;
        self.error = None;
        cx.notify();
    }

    /// Called by the workspace as the route changes, so the page only fetches
    /// while it is the one on screen.
    pub(super) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if !visible {
            // The few kilobytes of fetched data stay so coming back needs no
            // new request.
            // The listener has had their look at what was new.
            release_watch::acknowledge_all(cx);
            return;
        }
        self.load_if_needed(cx);
        self.load_releases_if_needed(cx);
        self.load_discover_if_needed(cx);
    }

    fn load_if_needed(&mut self, cx: &mut Context<Self>) {
        let range = RANGE;
        self.error = None;
        self.top.update(cx, |top, cx| top.ensure(range, cx));
    }

    fn load_releases_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.releases.as_ref().is_some_and(|releases| {
            releases
                .loaded_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < RELEASES_STALE_TIME)
        });
        if fresh || self.releases_loading || self.top.read(cx).permission_missing() {
            return;
        }
        self.releases_loading = true;
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadNewReleases {
            respond,
        });
        self.releases_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.releases_request = None;
                page.releases_loading = false;
                match result {
                    Ok(mut albums) => {
                        albums.truncate(model::NEW_RELEASE_LIMIT);
                        page.releases = Some(Releases {
                            albums: albums.into(),
                            loaded_at: SystemTime::now(),
                        });
                    }
                    // A missing section is better than an error banner for
                    // something Home can do without; try again on the next visit.
                    Err(error) => log::warn!("home: could not load new releases: {error}"),
                }
                cx.notify();
                // After the releases, so the page's own data is never kept waiting.
                page.load_upcoming_if_needed(cx);
            });
        }));
    }

    fn load_upcoming_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.upcoming.as_ref().is_some_and(|upcoming| {
            upcoming
                .loaded_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < UPCOMING_STALE_TIME)
        });
        if fresh || self.upcoming_loading || self.top.read(cx).permission_missing() || !self.visible
        {
            return;
        }
        self.upcoming_loading = true;
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadUpcoming {
            respond,
        });
        self.upcoming_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.upcoming_request = None;
                page.upcoming_loading = false;
                match result {
                    Ok(releases) => {
                        page.upcoming = Some(Upcoming {
                            releases: releases.into(),
                            loaded_at: SystemTime::now(),
                        });
                    }
                    Err(error) => log::debug!("home: could not load announced releases: {error}"),
                }
                cx.notify();
            });
        }));
    }

    /// Artists that sound like the listener's top ones. Asked for once the
    /// page's own data is on its way; a failure leaves the page as it was.
    fn load_discover_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.discover.as_ref().is_some_and(|discover| {
            discover
                .loaded_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < DISCOVER_STALE_TIME)
        });
        if fresh || self.discover_loading || self.top.read(cx).permission_missing() || !self.visible
        {
            return;
        }
        self.discover_loading = true;
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadDiscover {
            respond,
        });
        self.discover_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.discover_request = None;
                page.discover_loading = false;
                let suggestions = match result {
                    Ok(suggestions) => suggestions,
                    Err(error) => {
                        log::debug!("home: could not find artists to discover: {error}");
                        Vec::new()
                    }
                };
                page.discover_index = 0;
                page.discover = Some(Discover {
                    suggestions: suggestions.into(),
                    loaded_at: SystemTime::now(),
                });
                page.load_discover_songs(cx);
                cx.notify();
            });
        }));
    }

    /// Puts suggestions and their songs on the page without asking, for
    /// screenshots.
    #[cfg(test)]
    pub(super) fn fixture_discover(
        &mut self,
        suggestions: Vec<model::ArtistSuggestion>,
        songs: Vec<model::Track>,
        cx: &mut Context<Self>,
    ) {
        if let Some(first) = suggestions.first() {
            self.discover_songs
                .push((first.artist.source_id.clone(), songs.into()));
        }
        self.discover = Some(Discover {
            suggestions: suggestions.into(),
            loaded_at: SystemTime::now(),
        });
        self.discover_loading = false;
        cx.notify();
    }

    fn suggestion(&self) -> Option<&model::ArtistSuggestion> {
        self.discover
            .as_ref()
            .and_then(|discover| discover.suggestions.get(self.discover_index))
    }

    fn songs_of(&self, source_id: &str) -> Option<&Arc<[model::Track]>> {
        self.discover_songs
            .iter()
            .find(|(id, _)| id == source_id)
            .map(|(_, songs)| songs)
    }

    /// The songs to hear from the artist on offer, from their artist page.
    fn load_discover_songs(&mut self, cx: &mut Context<Self>) {
        let Some(source_id) = self
            .suggestion()
            .map(|suggestion| suggestion.artist.source_id.clone())
        else {
            return;
        };
        if self.songs_of(&source_id).is_some() {
            return;
        }
        let reply = catalog::request(&self.backend, {
            let source_id = source_id.clone();
            |respond| BackendCommand::LoadArtist { source_id, respond }
        });
        self.discover_songs_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.discover_songs_request = None;
                let songs: Arc<[model::Track]> = match result {
                    Ok((_, tracks, _)) => tracks.into_iter().take(DISCOVER_SONGS).collect(),
                    Err(error) => {
                        log::debug!("home: could not load songs to sample: {error}");
                        Arc::from([])
                    }
                };
                page.discover_songs.push((source_id, songs));
                if page.discover_songs.len() > DISCOVER_SONGS_KEPT {
                    page.discover_songs.remove(0);
                }
                cx.notify();
            });
        }));
    }

    /// "Not for me": remembered for good, and the next artist takes the place.
    fn dismiss_suggestion(&mut self, cx: &mut Context<Self>) {
        let Some(discover) = self.discover.as_mut() else {
            return;
        };
        let index = self.discover_index;
        let Some(gone) = discover.suggestions.get(index).cloned() else {
            return;
        };
        self.backend.send(BackendCommand::DismissSuggestion {
            source_id: gone.artist.source_id.clone(),
        });
        discover.suggestions = discover
            .suggestions
            .iter()
            .filter(|suggestion| suggestion.artist.source_id != gone.artist.source_id)
            .cloned()
            .collect();
        self.discover_index = index.min(discover.suggestions.len().saturating_sub(1));
        self.load_discover_songs(cx);
        cx.notify();
    }

    /// Plays the artist's songs from `first`, so the listener hears them
    /// without leaving Home.
    fn play_sample(&mut self, first: usize, cx: &mut Context<Self>) {
        let Some(songs) = self
            .suggestion()
            .and_then(|suggestion| self.songs_of(&suggestion.artist.source_id))
            .cloned()
        else {
            return;
        };
        if songs.is_empty() {
            return;
        }
        let player = services::AppServices::player(cx);
        player.update(cx, |player, cx| player.play_context(songs, first, cx));
    }

    fn open_suggested_artist(&mut self, cx: &mut Context<Self>) {
        if let Some(suggestion) = self.suggestion() {
            let artist = &suggestion.artist;
            cx.emit(PageEvent::OpenArtist(model::ArtistRef {
                name: artist.name.clone(),
                source_id: Some(artist.source_id.clone()),
                spotify_uri: artist.spotify_uri.clone(),
            }));
        }
    }

    /// The ways back into the library, in the order they are shown. A tile is
    /// left out when there is nothing behind it yet.
    fn shortcuts(&self, cx: &App) -> Vec<Shortcut> {
        let library = services::AppServices::library(cx);
        let library = library.read(cx);
        let playable = |tracks: &Arc<[model::Track]>| (!tracks.is_empty()).then(|| tracks.clone());
        let mut tiles = vec![
            Shortcut {
                url: None,
                icon: CadenceIcon::HeartFilled,
                tint: Some(LIKED_HUE),
                round: false,
                title: "Liked Songs".into(),
                detail: if library.loaded() {
                    format!("{} songs", library.liked_tracks().len())
                } else {
                    "Songs you liked".to_owned()
                },
                play: playable(library.liked_tracks()),
                event: PageEvent::Navigate(Route::LikedSongs),
            },
            Shortcut {
                url: None,
                icon: CadenceIcon::ClockFilled,
                tint: None,
                round: false,
                title: "Recently played".into(),
                detail: "What you played lately".to_owned(),
                play: playable(library.recently_played()),
                event: PageEvent::Navigate(Route::Recent),
            },
        ];
        // The album behind the latest song, when Spotify named one.
        if let Some((track, album)) = library.recently_played().iter().find_map(|track| {
            let album = track.album_ref.as_ref()?;
            album.source_id.as_ref()?;
            Some((track, album))
        }) {
            tiles.push(Shortcut {
                url: album
                    .artwork_url
                    .clone()
                    .or_else(|| track.artwork_url.clone()),
                icon: CadenceIcon::MusicNote,
                tint: None,
                round: false,
                title: album.name.clone(),
                detail: format!("Album · {}", track.artist),
                play: None,
                event: PageEvent::OpenAlbum(album.clone()),
            });
        }
        if let Some(playlist) = library
            .pinned_playlists()
            .first()
            .or_else(|| library.playlists().first())
        {
            tiles.push(Shortcut {
                url: playlist.artwork_url.clone(),
                icon: CadenceIcon::Playlist,
                tint: None,
                round: false,
                title: playlist.name.clone(),
                detail: "Playlist".to_owned(),
                play: None,
                event: PageEvent::OpenPlaylist(playlist.clone()),
            });
        }
        if let Some(artist) = self
            .top
            .read(cx)
            .data(RANGE)
            .and_then(|data| data.artists.first())
        {
            tiles.push(Shortcut {
                url: artist.artwork_url.clone(),
                icon: CadenceIcon::Person,
                tint: None,
                round: true,
                title: artist.name.clone(),
                detail: "Artist · your most played".to_owned(),
                play: None,
                event: PageEvent::OpenArtist(model::ArtistRef {
                    name: artist.name.clone(),
                    source_id: Some(artist.source_id.clone()),
                    spotify_uri: artist.spotify_uri.clone(),
                }),
            });
        }
        tiles.push(if library.has_local_files() {
            Shortcut {
                url: None,
                icon: CadenceIcon::FolderFilled,
                tint: None,
                round: false,
                title: "Local Files".into(),
                detail: format!("{} songs", library.local_tracks().len()),
                play: playable(library.local_tracks()),
                event: PageEvent::Navigate(Route::LocalFiles),
            }
        } else {
            Shortcut {
                url: None,
                icon: CadenceIcon::PlaylistFilled,
                tint: None,
                round: false,
                title: "Playlists".into(),
                detail: "All your playlists".to_owned(),
                play: None,
                event: PageEvent::Navigate(Route::Playlists),
            }
        });
        tiles
    }

    /// One shortcut: a cover flush with the tile's left edge, two lines of
    /// text, and, for what can start at once, a play button on hover.
    fn shortcut_tile(
        &self,
        index: usize,
        shortcut: Shortcut,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cache = services::AppServices::image_cache(cx);
        let dark = palette.canvas == CadencePalette::DARK.canvas;
        let cover: AnyElement = match (shortcut.url.as_deref(), shortcut.tint) {
            (None, Some(hue)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded_tl(rem(TILE_RADIUS))
                .rounded_bl(rem(TILE_RADIUS))
                .bg(gpui_kit::Hsla {
                    h: hue / 360.,
                    s: if dark { 0.18 } else { 0.30 },
                    l: if dark { 0.34 } else { 0.86 },
                    a: 1.,
                })
                .child(div().opacity(0.55).child(components::icon(
                    shortcut.icon,
                    Rems(1.625),
                    palette.text_primary,
                )))
                .into_any_element(),
            _ if shortcut.round => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(components::artwork(
                    palette,
                    &cache,
                    shortcut.url.as_deref(),
                    tokens::ArtworkSize {
                        size: Rems(2.875),
                        radius: Rems(1.4375),
                    },
                    shortcut.icon,
                ))
                .into_any_element(),
            _ => components::artwork_flush_right(
                palette,
                &cache,
                shortcut.url.as_deref(),
                tokens::ArtworkSize {
                    size: Rems(4.),
                    radius: Rems(TILE_RADIUS / 16.),
                },
                shortcut.icon,
            ),
        };
        let group = format!("home-tile-{index}");
        let playable = shortcut.play.is_some();
        let play = shortcut.play.map(|tracks| {
            let player = services::AppServices::player(cx);
            components::button(palette, ("home-shortcut-play", index))
                .test_support()
                .absolute()
                .right(rem(12.))
                .top(rem(16.))
                .size(rem(32.))
                .rounded_full()
                .bg(rgb(palette.text_primary))
                .invisible()
                .group_hover(group.clone(), |style| style.visible())
                .child(components::icon(
                    CadenceIcon::Play,
                    Rems(0.875),
                    palette.on_accent,
                ))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    let tracks = tracks.clone();
                    player.update(cx, |player, cx| {
                        player.play_context(tracks, 0, cx);
                    });
                }))
        });
        let event = shortcut.event;
        components::button(palette, ("home-shortcut", index))
            .test_support()
            .group(group)
            .relative()
            .w_full()
            .min_w_0()
            .h(rem(64.))
            .justify_start()
            .overflow_hidden()
            .rounded(rem(TILE_RADIUS))
            .bg(rgb(palette.card))
            .hover(|style| style.bg(rgb(palette.card_hover)))
            .child(div().size(rem(64.)).flex_none().child(cover))
            .child(
                row_text(palette, shortcut.title, shortcut.detail)
                    .ml(rem(12.))
                    .when(playable, |text| text.pr(rem(52.))),
            )
            .children(play)
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(event.clone())))
            .into_any_element()
    }

    /// The song to carry on with: the one that is playing or paused, or else
    /// the latest in Recently played.
    fn continue_card(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let player = services::AppServices::player(cx);
        let library = services::AppServices::library(cx);
        let (current, playing, position) = {
            let player = player.read(cx);
            (
                player.now_playing().cloned(),
                player.playing(),
                player.position_ms(Instant::now()),
            )
        };
        let resuming = current.is_some();
        let track = match current {
            Some(track) => track,
            None => library.read(cx).recently_played().first().cloned()?,
        };
        let cache = services::AppServices::image_cache(cx);
        let art = components::artwork(
            palette,
            &cache,
            track.artwork_url.as_deref(),
            tokens::ArtworkSize {
                size: Rems(7.625),
                radius: Rems((CARD_RADIUS - CARD_PADDING) / 16.),
            },
            CadenceIcon::MusicNote,
        );
        let (label, icon) = if !resuming {
            ("Play", CadenceIcon::Play)
        } else if playing {
            ("Pause", CadenceIcon::Pause)
        } else {
            ("Resume", CadenceIcon::Play)
        };
        // A running song would need a timer to keep its bar honest, so the bar
        // shows only while paused, when it cannot move.
        let progress = (resuming && !playing && track.duration_ms > 0).then(|| {
            let position = position.min(track.duration_ms);
            (
                position as f32 / track.duration_ms as f32,
                format_duration(position),
                format_duration(track.duration_ms),
            )
        });
        let muted = |text: String, size: f32| {
            div()
                .text_size(rem(size))
                .line_height(rem(size * 1.4))
                .text_color(rgb(palette.text_muted))
                .child(text)
        };
        Some(
            div()
                .id("home-continue")
                .test_support()
                .w_full()
                .p(rem(CARD_PADDING))
                .flex()
                .gap(rem(16.))
                .rounded(rem(CARD_RADIUS))
                .bg(rgb(palette.card))
                .child(art)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .child(
                            div()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(rem(20.))
                                        .line_height(rem(28.))
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette.text_primary))
                                        .child(track.title.clone()),
                                )
                                .child(muted(track.artist.to_string(), 16.).truncate()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_start()
                                .when_some(progress, |column, (fraction, at, total)| {
                                    column
                                        .w_full()
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .justify_between()
                                                .child(muted(at, 12.))
                                                .child(muted(total, 12.)),
                                        )
                                        .child(
                                            div()
                                                .id("home-continue-progress")
                                                .test_support()
                                                .w_full()
                                                .h(rem(5.))
                                                .mt(rem(4.))
                                                .rounded_full()
                                                .bg(rgb(palette.card_hover))
                                                .child(
                                                    div()
                                                        .h_full()
                                                        .w(relative(fraction))
                                                        .rounded_full()
                                                        .bg(rgb(palette.text_primary)),
                                                ),
                                        )
                                })
                                .child(
                                    div().mt(rem(12.)).child(
                                        compact(components::icon_pill(
                                            palette,
                                            "home-continue-play",
                                            icon,
                                            label,
                                            true,
                                        ))
                                        .test_support()
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| {
                                                if resuming {
                                                    player
                                                        .update(cx, |player, cx| player.toggle(cx));
                                                } else {
                                                    let list =
                                                        library.read(cx).recently_played().clone();
                                                    player.update(cx, |player, cx| {
                                                        player.play_context(list, 0, cx);
                                                    });
                                                }
                                            }),
                                        ),
                                    ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The newest albums from artists you play, as covers.
    fn releases_shelf(
        &self,
        count: usize,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let cache = services::AppServices::image_cache(cx);
        let size = tokens::ArtworkSize {
            size: rem(SHELF_COVER),
            radius: Rems(0.875),
        };
        let Some(releases) = self.releases.as_ref() else {
            // Placeholders hold the shelf's height while Spotify answers.
            return self.releases_loading.then(|| {
                div()
                    .id("home-releases-loading")
                    .test_support()
                    .w_full()
                    .flex()
                    .justify_between()
                    .children((0..count).map(|_| {
                        div()
                            .size(size.size)
                            .rounded(size.radius)
                            .bg(rgb(palette.card))
                    }))
                    .into_any_element()
            });
        };
        if releases.albums.is_empty() {
            return None;
        }
        let mut shelf = div()
            .id("home-releases")
            .test_support()
            .w_full()
            .flex()
            .map(|row| {
                // A full row spreads to both edges; a short one keeps its gap.
                if releases.albums.len() >= count {
                    row.justify_between()
                } else {
                    row.gap(rem(SHELF_GAP))
                }
            });
        for (index, album) in releases.albums.iter().take(count).enumerate() {
            let mut artist = album
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if release_watch::is_new(&album.source_id, cx) {
                artist = format!("New · {artist}");
            }
            let target = model::AlbumRef {
                name: album.name.clone(),
                source_id: Some(album.source_id.clone()),
                spotify_uri: album.spotify_uri.clone(),
                artwork_url: album.artwork_url.clone(),
            };
            shelf = shelf.child(
                components::button(palette, ("home-release", index))
                    .test_support()
                    .w(size.size)
                    .flex_none()
                    .flex_col()
                    .items_start()
                    .child(components::artwork(
                        palette,
                        &cache,
                        album.artwork_url.as_deref(),
                        size,
                        CadenceIcon::MusicNote,
                    ))
                    .child(
                        div()
                            .w_full()
                            .mt(rem(8.))
                            .truncate()
                            .text_size(rem(13.5))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(album.name.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(rem(12.))
                            .text_color(rgb(palette.text_muted))
                            .child(artist),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(PageEvent::OpenAlbum(target.clone()))
                    })),
            );
        }
        Some(shelf.into_any_element())
    }

    /// An artist to try and the next announced release, as two short rows.
    fn worth_a_listen(
        &self,
        palette: CadencePalette,
        columns: u16,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let cache = services::AppServices::image_cache(cx);
        // A row fills its grid column, so it ends where the Continue card and
        // the shelf above it end; a lone row keeps the left column's width.
        let card = |id: &'static str| {
            div()
                .id(id)
                .test_support()
                .w_full()
                .min_w_0()
                .h(rem(60.))
                .flex()
                .items_center()
                .rounded(rem(TILE_RADIUS))
                .bg(rgb(palette.card))
        };
        let artist_row = if let Some(suggestion) = self
            .discover
            .as_ref()
            .and_then(|discover| discover.suggestions.get(self.discover_index))
        {
            let artist = &suggestion.artist;
            let because = match suggestion.because.as_slice() {
                [] => "Try this next".to_owned(),
                [first, ..] => format!("Try this next · sounds like {first}"),
            };
            let ready = self
                .songs_of(&artist.source_id)
                .is_some_and(|songs| !songs.is_empty());
            Some(
                card("home-discover")
                    .child(
                        components::button(palette, "home-discover-open")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .justify_start()
                            .rounded(rem(TILE_RADIUS))
                            .hover(|style| style.bg(rgb(palette.card_hover)))
                            .child(div().ml(rem(10.)).child(components::artwork(
                                palette,
                                &cache,
                                artist.artwork_url.as_deref(),
                                tokens::ArtworkSize {
                                    size: Rems(2.5),
                                    radius: Rems(1.25),
                                },
                                CadenceIcon::Person,
                            )))
                            .child(row_text(palette, artist.name.clone(), because).ml(rem(12.)))
                            .on_click(cx.listener(|this, _, _, cx| this.open_suggested_artist(cx))),
                    )
                    .child(div().flex_none().mr(rem(6.)).child(if ready {
                        compact(components::icon_pill(
                            palette,
                            "home-discover-play",
                            CadenceIcon::Play,
                            "Sample",
                            true,
                        ))
                        .test_support()
                        .on_click(cx.listener(|this, _, _, cx| this.play_sample(0, cx)))
                        .into_any_element()
                    } else {
                        compact(components::pending_pill(
                            palette,
                            "home-discover-pending",
                            "Sample",
                        ))
                        .px(rem(14.))
                        .opacity(0.35)
                        .into_any_element()
                    }))
                    .child(
                        components::icon_button_sized(
                            palette,
                            "home-discover-dismiss",
                            CadenceIcon::Close,
                            tokens::INLINE_ICON,
                        )
                        .test_support()
                        .size_8()
                        .mr(rem(10.))
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_suggestion(cx))),
                    )
                    .into_any_element(),
            )
        } else if self.discover.is_none() && self.discover_loading {
            Some(card("home-discover-loading").into_any_element())
        } else {
            None
        };
        let release_row = self
            .upcoming
            .as_ref()
            .and_then(|upcoming| upcoming.releases.first())
            .map(|release| {
                let when = release_label(&release.date).unwrap_or_else(|| release.date.clone());
                card("home-upcoming")
                    .child(
                        div()
                            .ml(rem(10.))
                            .flex_none()
                            .size(rem(40.))
                            .rounded(rem(TILE_RADIUS - 10.))
                            .bg(rgb(palette.card_hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(components::icon(
                                CadenceIcon::MusicNote,
                                Rems(1.0625),
                                palette.text_muted,
                            )),
                    )
                    .child(
                        row_text(
                            palette,
                            format!("{} · new {}", release.artist, release.kind.to_lowercase()),
                            format!("Announced for {when}"),
                        )
                        .ml(rem(12.)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mr(rem(14.))
                            .text_size(rem(12.5))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child("Coming soon"),
                    )
                    .into_any_element()
            });
        if artist_row.is_none() && release_row.is_none() {
            return None;
        }
        Some(
            div()
                .grid()
                .grid_cols(columns)
                .gap(rem(GRID_GAP))
                .children(artist_row)
                .children(release_row)
                .into_any_element(),
        )
    }

    fn permission_prompt(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_3()
            .p_6()
            .rounded(tokens::PANEL_RADIUS)
            .border_1()
            .border_color(rgb(palette.border))
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_primary))
                    .child("One more permission needed"),
            )
            .child(components::page_detail(
                palette,
                "Home reads your top artists and tracks. Sign in again to allow it. Your settings and playlists stay.",
            ))
            .child(
                components::pill(palette, "home-sign-in-again", "Sign in again", true)
                    .test_support()
                    .h_10()
                    .mt_1()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session.update(cx, |session, cx| session.logout(cx));
                    })),
            )
    }
}

impl Render for HomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        // Room for the page in the pixels it comes to at 100% zoom: the window
        // less the sidebar and the page's own margins.
        let room = window.viewport_size().width / window.rem_size() * 16. - PAGE_CHROME;
        let tile_columns: u16 = if room >= 1000. {
            3
        } else if room >= 620. {
            2
        } else {
            1
        };
        let pair_columns: u16 = if room >= 860. { 2 } else { 1 };
        let heading = |id, text: SharedString| {
            div()
                .id(id)
                .test_support()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_primary))
                .child(text)
        };
        // A heading with its content beneath, as in the opening sections.
        let section = |id, title: &str, body: AnyElement| {
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_3()
                .child(heading(id, title.to_owned().into()))
                .child(body)
        };
        let shortcuts: Vec<AnyElement> = self
            .shortcuts(cx)
            .into_iter()
            .enumerate()
            .map(|(index, shortcut)| self.shortcut_tile(index, shortcut, palette, cx))
            .collect();
        let continue_card = self.continue_card(palette, cx);
        let shelf_count = shelf_count(room, pair_columns);
        let shelf = self.releases_shelf(shelf_count, palette, cx);
        let worth = self.worth_a_listen(palette, pair_columns, cx);
        let first = div()
            .flex()
            .flex_col()
            .gap_6()
            .when(!shortcuts.is_empty(), |block| {
                block.child(
                    div()
                        .id("home-shortcuts")
                        .test_support()
                        .grid()
                        .grid_cols(tile_columns)
                        .gap(rem(GRID_GAP))
                        .children(shortcuts),
                )
            })
            .when(continue_card.is_some() || shelf.is_some(), |block| {
                block.child(
                    div()
                        .grid()
                        .grid_cols(pair_columns)
                        .gap(rem(GRID_GAP))
                        .children(
                            continue_card
                                .map(|card| section("home-continue-heading", "Continue", card)),
                        )
                        .children(shelf.map(|shelf| {
                            section("home-releases-heading", "New from artists you play", shelf)
                        })),
                )
            })
            .when_some(worth, |block, worth| {
                block.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(heading("home-discover-heading", "Worth a listen".into()))
                        .child(worth),
                )
            });

        // The only thing below the rows is the ask for the permission they need.
        let permission = self
            .top
            .read(cx)
            .permission_missing()
            .then(|| self.permission_prompt(palette, cx));
        div()
            .id("home-page")
            .size_full()
            .min_h_0()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .child(
                div()
                    .id("home-scroll")
                    .test_support()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .p_8()
                            .pt_3()
                            .pr(px(48.))
                            .w_full()
                            .child(components::page_heading(
                                palette,
                                greeting(),
                                "Pick up where you left off",
                            ))
                            .child(first)
                            .children(permission.map(|prompt| div().mt_6().child(prompt))),
                    ),
            )
    }
}

/// The sidebar and the page's margins, in pixels at 100% zoom.
const PAGE_CHROME: f32 = 312.;
/// The space between neighbours in every grid, so edges line up.
const GRID_GAP: f32 = 16.;
/// Corner radius of the shortcut tiles and the Worth a listen rows.
const TILE_RADIUS: f32 = 18.;
/// The Continue card: its cover's radius is the card's less the padding, so
/// the corners run parallel.
const CARD_RADIUS: f32 = 24.;
const CARD_PADDING: f32 = 14.;

/// Covers per row: as many as fit the shelf's column, so a wider window shows
/// more albums instead of spreading three apart. `room` is the page's width in
/// pixels at 100% zoom.
fn shelf_count(room: f32, pair_columns: u16) -> usize {
    let width = if pair_columns == 2 {
        (room - GRID_GAP) / 2.
    } else {
        room
    };
    (((width + SHELF_GAP) / (SHELF_COVER + SHELF_GAP)) as usize).clamp(RELEASES_MIN, RELEASES_MAX)
}

/// A pill sized for a card or a row: 32 high, so it sits quietly inside the
/// card it is on instead of out-weighing it.
fn compact(pill: Stateful<Div>) -> Stateful<Div> {
    pill.h(rem(32.))
        .pl(rem(12.))
        .pr(rem(14.))
        .text_size(rem(13.5))
        // Pressing sinks it a little, so it feels like it heard the click.
        .active(|style| style.opacity(0.85))
}

/// A length that follows the interface zoom, written in the pixels it comes to
/// at 100%. Everything on this page is sized this way so covers, cards and
/// text keep their proportions when the zoom changes.
fn rem(pixels: f32) -> Rems {
    Rems(pixels / 16.)
}

/// Two lines of text for a row: a title and a quieter detail beneath it.
fn row_text(
    palette: CadencePalette,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_start()
        .child(
            div()
                .max_w_full()
                .truncate()
                .text_size(rem(15.5))
                .line_height(rem(21.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_primary))
                .child(title.into()),
        )
        .child(
            div()
                .max_w_full()
                .truncate()
                .mt(rem(1.))
                .text_size(rem(13.5))
                .line_height(rem(19.))
                .text_color(rgb(palette.text_muted))
                .child(detail.into()),
        )
}

fn release_label(date: &str) -> Option<String> {
    let mut parts = date.split('-');
    let _year: u32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    Some(format!("{} {day}", MONTHS.get(month.checked_sub(1)?)?))
}

#[cfg(test)]
mod tests {
    use super::{greeting_for, release_label, shelf_count};

    #[test]
    fn the_greeting_follows_the_hour() {
        assert_eq!(greeting_for(5), "Good morning");
        assert_eq!(greeting_for(11), "Good morning");
        assert_eq!(greeting_for(12), "Good afternoon");
        assert_eq!(greeting_for(16), "Good afternoon");
        assert_eq!(greeting_for(17), "Good evening");
        assert_eq!(greeting_for(21), "Good evening");
        assert_eq!(greeting_for(22), "Late night");
        assert_eq!(greeting_for(0), "Late night");
        assert_eq!(greeting_for(4), "Late night");
    }

    #[test]
    fn the_shelf_shows_more_albums_as_the_window_widens() {
        // A 1280 window: two columns of about 476, so three covers.
        assert_eq!(shelf_count(968., 2), 3);
        // The window Sonder opens with shows at least four.
        assert!(
            shelf_count(
                super::super::windows::DEFAULT_WINDOW_WIDTH - super::PAGE_CHROME,
                2
            ) >= 4
        );
        // Wider windows add covers rather than gaps.
        assert_eq!(shelf_count(1400., 2), 4);
        assert_eq!(shelf_count(2200., 2), 6);
        assert_eq!(shelf_count(5000., 2), 6);
        // One column holds the whole width.
        assert_eq!(shelf_count(700., 1), 4);
        // Never fewer than two, however narrow.
        assert_eq!(shelf_count(200., 1), 2);
    }

    #[test]
    fn release_dates_read_as_month_and_day() {
        assert_eq!(release_label("2026-11-07").as_deref(), Some("Nov 7"));
        assert_eq!(release_label("2026-01-31").as_deref(), Some("Jan 31"));
        assert_eq!(release_label("2026-11"), None);
        assert_eq!(release_label("2026"), None);
        assert_eq!(release_label("2026-13-01"), None);
    }
}
