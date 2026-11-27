//! The Home page. It starts with a greeting, then two panes. On the left:
//! shortcuts back into the library, then Spotify's own "Made for you" and
//! "Your top mixes". On the right: a rail of what is new from artists you play.
//!
//! The two mix shelves are read the way Spotify's own apps read them, through
//! the playback session (see `made_for_you.rs`); when that read fails the
//! shelves are simply left out. Two small Spotify requests per time range,
//! made only when the page is opened and its data is more than half an hour
//! old. Nothing runs in the background.
use super::page::PageEvent;
use super::top_listening::{TopEvent, TopListening};
use super::*;
use gpui_kit::TestSupportExt as _;
use std::time::SystemTime;

/// New releases move slowly, and finding them costs a request per artist.
const RELEASES_STALE_TIME: Duration = Duration::from_secs(6 * 60 * 60);
/// The backend keeps its own answer for ten minutes; this only avoids asking
/// it on every visit.
const MIXES_STALE_TIME: Duration = Duration::from_secs(10 * 60);
/// After a failed read the page waits this long before asking again, so
/// opening Home over and over does not hammer a private service that is down.
const MIXES_RETRY: Duration = Duration::from_secs(5 * 60);
/// Releases listed in the rail before "Show all".
const RAIL_ROWS: usize = 4;
/// The rail's width and the gap between the two panes, at 100% zoom.
const RAIL_WIDTH: f32 = 412.;
const PANE_GAP: f32 = 32.;
/// The narrowest a mix cover is allowed to get before a card is dropped.
const MIX_COVER_MIN: f32 = 190.;
const MIX_COVER_MAX_COUNT: usize = 6;
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

struct Releases {
    albums: Arc<[model::Album]>,
    loaded_at: SystemTime,
}

struct Mixes {
    shelves: model::HomeMixes,
    loaded_at: SystemTime,
}

/// The shelves that can be opened out to show every card.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Shelf {
    MadeForYou,
    TopMixes,
    Releases,
}

pub(super) struct HomePage {
    backend: BackendHandle,
    session: Entity<session::Session>,
    top: Entity<TopListening>,
    releases: Option<Releases>,
    releases_loading: bool,
    releases_request: Option<gpui_kit::Task<()>>,
    mixes: Option<Mixes>,
    mixes_loading: bool,
    /// When the last read failed, so the next visit does not retry at once.
    mixes_failed_at: Option<SystemTime>,
    mixes_request: Option<gpui_kit::Task<()>>,
    /// Which shelves "Show all" has opened out.
    expanded: Vec<Shelf>,
    /// The album a rail row's play button is fetching, so a second click
    /// while it loads does not start it twice.
    playing_release: Option<(String, gpui_kit::Task<()>)>,
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
            // The shortcuts follow the library and the player.
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
            mixes: None,
            mixes_loading: false,
            mixes_failed_at: None,
            mixes_request: None,
            expanded: Vec::new(),
            playing_release: None,
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
                    self.load_mixes_if_needed(cx);
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
        self.mixes = None;
        self.mixes_loading = false;
        self.mixes_failed_at = None;
        self.mixes_request = None;
        self.expanded.clear();
        self.playing_release = None;
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
        self.load_mixes_if_needed(cx);
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
            });
        }));
    }

    /// Made for you and Your top mixes. A failure leaves both shelves out and
    /// the next visit asks again; the backend paces itself if Spotify asked it
    /// to slow down.
    fn load_mixes_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.mixes.as_ref().is_some_and(|mixes| {
            mixes
                .loaded_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < MIXES_STALE_TIME)
        });
        let failed_lately = self
            .mixes_failed_at
            .is_some_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed < MIXES_RETRY));
        if fresh || failed_lately || self.mixes_loading || !self.visible {
            return;
        }
        self.mixes_loading = true;
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadMadeForYou {
            force: false,
            respond,
        });
        self.mixes_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.mixes_request = None;
                page.mixes_loading = false;
                match result {
                    Ok(shelves) => {
                        page.mixes_failed_at = None;
                        page.mixes = Some(Mixes {
                            shelves,
                            loaded_at: SystemTime::now(),
                        });
                    }
                    Err(error) => {
                        page.mixes_failed_at = Some(SystemTime::now());
                        log::warn!("home: could not load Made for you: {error:#}");
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Starts an album from the rail without leaving Home.
    fn play_release(&mut self, source_id: String, cx: &mut Context<Self>) {
        if self
            .playing_release
            .as_ref()
            .is_some_and(|(loading, _)| *loading == source_id)
        {
            return;
        }
        let reply = catalog::request(&self.backend, {
            let source_id = source_id.clone();
            |respond| BackendCommand::LoadAlbum { source_id, respond }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.playing_release = None;
                match result {
                    Ok((_, tracks)) if !tracks.is_empty() => {
                        let player = services::AppServices::player(cx);
                        player.update(cx, |player, cx| player.play_context(tracks, 0, cx));
                    }
                    Ok(_) => {}
                    Err(error) => log::warn!("home: could not start the release: {error}"),
                }
                cx.notify();
            });
        });
        self.playing_release = Some((source_id, task));
    }

    /// Puts the two mix shelves on the page without asking, for screenshots.
    #[cfg(test)]
    pub(super) fn fixture_mixes(&mut self, shelves: model::HomeMixes, cx: &mut Context<Self>) {
        self.mixes = Some(Mixes {
            shelves,
            loaded_at: SystemTime::now(),
        });
        self.mixes_loading = false;
        cx.notify();
    }

    fn is_expanded(&self, shelf: Shelf) -> bool {
        self.expanded.contains(&shelf)
    }

    /// "Show all" opens a shelf out; the same link closes it again.
    fn toggle_expanded(&mut self, shelf: Shelf, cx: &mut Context<Self>) {
        if let Some(index) = self.expanded.iter().position(|open| *open == shelf) {
            self.expanded.remove(index);
        } else {
            self.expanded.push(shelf);
        }
        cx.notify();
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

    /// "Show all" beside a shelf's heading, which opens it out and closes it.
    fn show_all(
        &self,
        id: &'static str,
        shelf: Shelf,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .test_support()
            .cursor_pointer()
            .text_size(rem(14.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(rgb(palette.text_muted))
            .hover(|style| style.text_color(rgb(palette.text_primary)))
            .child(if self.is_expanded(shelf) {
                "Show less"
            } else {
                "Show all"
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(shelf, cx)))
    }

    /// A shelf of Spotify's mixes: a heading with "Show all", and covers that
    /// fit the column, or every cover in rows once it is opened out.
    #[allow(clippy::too_many_arguments)]
    fn mix_shelf(
        &self,
        id: &'static str,
        title: &'static str,
        shelf: Shelf,
        mixes: &[model::HomeMix],
        loading: bool,
        count: usize,
        cover: f32,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if mixes.is_empty() && !loading {
            return None;
        }
        let cache = services::AppServices::image_cache(cx);
        let size = tokens::ArtworkSize {
            size: rem(cover),
            radius: rem(14.),
        };
        let expanded = self.is_expanded(shelf);
        let cards: Vec<AnyElement> = if mixes.is_empty() {
            // Placeholders hold the shelf's height while Spotify answers.
            (0..count)
                .map(|_| {
                    div()
                        .size(size.size)
                        .flex_none()
                        .rounded(size.radius)
                        .bg(rgb(palette.card))
                        .into_any_element()
                })
                .collect()
        } else {
            mixes
                .iter()
                .take(if expanded { mixes.len() } else { count })
                .enumerate()
                .map(|(index, mix)| {
                    let playlist = mix.playlist.clone();
                    components::button(palette, (id, index))
                        .test_support()
                        .w(size.size)
                        .flex_none()
                        .flex_col()
                        .items_start()
                        .child(components::artwork(
                            palette,
                            &cache,
                            mix.playlist.artwork_url.as_deref(),
                            size,
                            CadenceIcon::Playlist,
                        ))
                        .child(
                            div()
                                .w_full()
                                .mt(rem(12.))
                                .truncate()
                                .text_size(rem(16.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(mix.playlist.name.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .mt(rem(3.))
                                .truncate()
                                .text_size(rem(14.))
                                .text_color(rgb(palette.text_muted))
                                .child(mix.description.clone().unwrap_or_default()),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(PageEvent::OpenPlaylist(playlist.clone()))
                        }))
                        .into_any_element()
                })
                .collect()
        };
        let more = mixes.len() > count;
        Some(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .mb(rem(12.))
                        .child(heading(palette, format!("{id}-heading"), title.into(), 22.))
                        .when(more, |header| {
                            header.child(self.show_all(
                                if shelf == Shelf::MadeForYou {
                                    "home-made-show-all"
                                } else {
                                    "home-top-show-all"
                                },
                                shelf,
                                palette,
                                cx,
                            ))
                        }),
                )
                .child(
                    div()
                        .id(format!("{id}-row"))
                        .test_support()
                        .w_full()
                        .flex()
                        .flex_wrap()
                        .gap(rem(GRID_GAP))
                        .children(cards),
                )
                .into_any_element(),
        )
    }

    /// What is new from artists you play, as a panel of rows with a date and
    /// a play button.
    fn releases_rail(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cache = services::AppServices::image_cache(cx);
        let cover = tokens::ArtworkSize {
            size: rem(56.),
            radius: rem(10.),
        };
        let panel = || {
            div()
                .w_full()
                .flex()
                .flex_col()
                .rounded(rem(22.))
                .bg(rgb(palette.card))
                .pt(rem(18.))
                .px(rem(18.))
                .pb(rem(10.))
        };
        let Some(releases) = self.releases.as_ref() else {
            // Placeholders hold the panel's height while Spotify answers.
            return self.releases_loading.then(|| {
                panel()
                    .id("home-releases-loading")
                    .test_support()
                    .gap(rem(12.))
                    .pb(rem(18.))
                    .child(heading(
                        palette,
                        "home-releases-heading".to_owned(),
                        "New from artists you play".into(),
                        20.,
                    ))
                    .children((0..RAIL_ROWS).map(|_| {
                        div()
                            .h(cover.size)
                            .w_full()
                            .rounded(rem(12.))
                            .bg(rgb(palette.card_hover))
                    }))
                    .into_any_element()
            });
        };
        if releases.albums.is_empty() {
            return None;
        }
        let expanded = self.is_expanded(Shelf::Releases);
        let shown = if expanded {
            releases.albums.len()
        } else {
            RAIL_ROWS
        };
        let rows: Vec<AnyElement> =
            releases
                .albums
                .iter()
                .take(shown)
                .enumerate()
                .map(|(index, album)| {
                    let mut detail = album
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(date) = album.release_date.as_deref().and_then(release_label) {
                        detail = format!("{detail} · {date}");
                    }
                    if release_watch::is_new(&album.source_id, cx) {
                        detail = format!("New · {detail}");
                    }
                    let target = model::AlbumRef {
                        name: album.name.clone(),
                        source_id: Some(album.source_id.clone()),
                        spotify_uri: album.spotify_uri.clone(),
                        artwork_url: album.artwork_url.clone(),
                    };
                    let source_id = album.source_id.clone();
                    components::button(palette, ("home-release", index))
                        .test_support()
                        .relative()
                        .w_full()
                        .min_w_0()
                        .justify_start()
                        .gap(rem(14.))
                        .px(rem(6.))
                        .py(rem(9.))
                        .rounded(rem(12.))
                        .hover(|style| style.bg(rgb(palette.card_hover)))
                        .child(components::artwork(
                            palette,
                            &cache,
                            album.artwork_url.as_deref(),
                            cover,
                            CadenceIcon::MusicNote,
                        ))
                        .child(row_text(palette, album.name.clone(), detail).pr(rem(44.)))
                        .child(
                            components::button(palette, ("home-release-play", index))
                                .test_support()
                                .absolute()
                                .right(rem(6.))
                                .top(rem(21.))
                                .size(rem(32.))
                                .rounded_full()
                                .bg(rgb(palette.card_hover))
                                .child(components::icon(
                                    CadenceIcon::Play,
                                    Rems(0.6875),
                                    palette.text_primary,
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.play_release(source_id.clone(), cx);
                                })),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(PageEvent::OpenAlbum(target.clone()))
                        }))
                        .into_any_element()
                })
                .collect();
        Some(
            panel()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .mb(rem(10.))
                        .child(heading(
                            palette,
                            "home-releases-heading".to_owned(),
                            "New from artists you play".into(),
                            20.,
                        ))
                        .when(releases.albums.len() > RAIL_ROWS, |header| {
                            header.child(self.show_all(
                                "home-releases-show-all",
                                Shelf::Releases,
                                palette,
                                cx,
                            ))
                        }),
                )
                .child(
                    div()
                        .id("home-releases")
                        .test_support()
                        .flex()
                        .flex_col()
                        .children(rows),
                )
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
        let layout = Layout::for_room(room);
        let shortcuts: Vec<AnyElement> = self
            .shortcuts(cx)
            .into_iter()
            .enumerate()
            .map(|(index, shortcut)| self.shortcut_tile(index, shortcut, palette, cx))
            .collect();
        let (made_for_you, top_mixes, mixes_loading) = match self.mixes.as_ref() {
            Some(mixes) => (
                mixes.shelves.made_for_you.clone(),
                mixes.shelves.top_mixes.clone(),
                false,
            ),
            None => (Vec::new(), Vec::new(), self.mixes_loading),
        };
        let made = self.mix_shelf(
            "home-made",
            "Made for you",
            Shelf::MadeForYou,
            &made_for_you,
            mixes_loading,
            layout.mix_count,
            layout.mix_cover,
            palette,
            cx,
        );
        let top = self.mix_shelf(
            "home-top",
            "Your top mixes",
            Shelf::TopMixes,
            &top_mixes,
            // Only Made for you holds a place while loading; Your top mixes
            // appears with it, or not at all.
            false,
            layout.mix_count,
            layout.mix_cover,
            palette,
            cx,
        );
        let rail = self.releases_rail(palette, cx);

        let main = div()
            .min_w_0()
            .when(!layout.stacked, |main| main.flex_1())
            .when(layout.stacked, |main| main.w_full())
            .flex()
            .flex_col()
            .gap(rem(22.))
            .when(!shortcuts.is_empty(), |main| {
                main.child(
                    div()
                        .id("home-shortcuts")
                        .test_support()
                        .grid()
                        .grid_cols(layout.tile_columns)
                        .gap(rem(12.))
                        .children(shortcuts),
                )
            })
            .children(made)
            .children(top);
        let panes = div()
            .id("home-panes")
            .test_support()
            .w_full()
            .flex()
            .when(layout.stacked, |panes| panes.flex_col())
            .gap(rem(if layout.stacked { 22. } else { PANE_GAP }))
            .child(main)
            .children(rail.map(|rail| {
                div()
                    .flex_none()
                    .when(!layout.stacked, |rail_column| {
                        rail_column.w(rem(RAIL_WIDTH))
                    })
                    .when(layout.stacked, |rail_column| rail_column.w_full())
                    .child(rail)
            }));

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
                            .child(
                                div()
                                    .mb(rem(14.))
                                    .child(components::page_title(palette, greeting())),
                            )
                            .child(panes)
                            .children(permission.map(|prompt| div().mt_6().child(prompt))),
                    ),
            )
    }
}

/// How the page divides its room, from the width left for it.
struct Layout {
    /// The rail sits under the main pane instead of beside it.
    stacked: bool,
    tile_columns: u16,
    /// Mixes per row and how wide each cover is, so a row fills the pane.
    mix_count: usize,
    mix_cover: f32,
}

impl Layout {
    /// `room` is the page's width in pixels at 100% zoom.
    fn for_room(room: f32) -> Self {
        let stacked = room < STACK_BELOW;
        let main = if stacked {
            room
        } else {
            room - PANE_GAP - RAIL_WIDTH
        };
        let mix_count = (((main + GRID_GAP) / (MIX_COVER_MIN + GRID_GAP)) as usize)
            .clamp(2, MIX_COVER_MAX_COUNT);
        let mix_cover = ((main - GRID_GAP * (mix_count - 1) as f32) / mix_count as f32).floor();
        Self {
            stacked,
            tile_columns: if main >= 560. { 2 } else { 1 },
            mix_count,
            mix_cover,
        }
    }
}

/// Under this the rail no longer fits beside the main pane.
const STACK_BELOW: f32 = 1000.;
/// The sidebar and the page's margins, in pixels at 100% zoom.
const PAGE_CHROME: f32 = 312.;
/// The space between neighbours in every grid, so edges line up.
const GRID_GAP: f32 = 16.;
/// Corner radius of the shortcut tiles.
const TILE_RADIUS: f32 = 18.;

/// A shelf or panel heading at `size` pixels.
fn heading(palette: CadencePalette, id: String, text: SharedString, size: f32) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .test_support()
        .text_size(rem(size))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_primary))
        .child(text)
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
    use super::{GRID_GAP, Layout, PAGE_CHROME, PANE_GAP, RAIL_WIDTH, greeting_for, release_label};

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
    fn the_layout_follows_the_window() {
        // The window Sonder opens with: the rail beside the main pane, and
        // four mixes across it.
        let wide = Layout::for_room(super::super::windows::DEFAULT_WINDOW_WIDTH - PAGE_CHROME);
        assert!(!wide.stacked);
        assert_eq!(wide.tile_columns, 2);
        assert_eq!(wide.mix_count, 4);
        // A row of covers fills the pane exactly, to the pixel.
        let main =
            super::super::windows::DEFAULT_WINDOW_WIDTH - PAGE_CHROME - PANE_GAP - RAIL_WIDTH;
        let row = wide.mix_cover * wide.mix_count as f32 + GRID_GAP * (wide.mix_count - 1) as f32;
        assert!(
            (main - row).abs() < wide.mix_count as f32,
            "{main} vs {row}"
        );
        // Wider windows add covers rather than stretching them.
        assert!(Layout::for_room(2200.).mix_count > wide.mix_count);
        assert!(Layout::for_room(5000.).mix_count <= 6);
        // Narrower ones put the rail under the main pane.
        let narrow = Layout::for_room(800.);
        assert!(narrow.stacked);
        assert!(narrow.mix_count >= 2);
        assert_eq!(Layout::for_room(400.).tile_columns, 1);
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
