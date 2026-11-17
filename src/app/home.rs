//! The Home page. It starts with a greeting and shortcuts back into the
//! library, then what you were playing and what is new from artists you play.
//! Below that: one artist to try next, found because they sound like ones you
//! play, beside your top artists; story cards for what is coming and who to come
//! back to; then your top tracks.
//!
//! Who sounds like whom comes from ListenBrainz (see `similar.rs`), looked up
//! once every few days and kept on disk. Two small Spotify requests per time range, made only when the page is opened
//! and its data is more than half an hour old. The range on screen is compared
//! with the next longer one, so one more pair of requests shows what is rising
//! and who has gone quiet. Nothing runs in the background.
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
/// Other artists shown under the one on offer.
const DISCOVER_NEXT_SHOWN: usize = 4;
/// New albums shown under Home's first heading.
const RELEASES_SHOWN: usize = 3;
/// How many stories of each kind the briefing shows.
const STORY_UPCOMING: usize = 2;
const STORY_COME_BACK: usize = 2;
/// A rising or falling artist is marked green or red.
const RISING: u32 = 0x2E9E63;
/// Artists listed before "See all".
const ARTISTS_SHOWN: usize = 5;
/// An artist is "come back to" when they rank this high on the longer chart.
const COME_BACK_RANK: usize = 5;

/// One way back into the library, drawn as a tile at the top of Home.
struct Shortcut {
    url: Option<String>,
    icon: CadenceIcon,
    round: bool,
    title: SharedString,
    detail: String,
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
    track_list: Entity<track_list::TrackList>,
    range: model::HomeRange,
    range_menu_open: bool,
    all_artists: bool,
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
        let track_list = cx.new(|cx| track_list::TrackList::new(cx).with_inline_rows());
        let session = services::AppServices::session(cx);
        let top = TopListening::shared(&backend, cx);
        let subscriptions = vec![
            page::forward(&track_list, cx),
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
            track_list,
            range: model::HomeRange::default(),
            range_menu_open: false,
            all_artists: false,
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
            TopEvent::Loaded(range) if *range == self.range => {
                self.error = None;
                cx.emit(PageEvent::Loaded);
                // The next longer range shows what is rising. It is the page's
                // second request pair, made once the first is on screen.
                if self.visible
                    && let Some(longer) = range.longer()
                {
                    self.top.update(cx, |top, cx| top.ensure(longer, cx));
                }
            }
            TopEvent::Failed(range, error) if *range == self.range => {
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
        self.range_menu_open = false;
        self.track_list.update(cx, |list, cx| list.clear(cx));
        cx.notify();
    }

    /// Called by the workspace as the route changes, so the page only fetches
    /// while it is the one on screen.
    pub(super) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if !visible {
            self.range_menu_open = false;
            // Leaving frees the list's rows; the few kilobytes of fetched data
            // stay so coming back needs no new request.
            self.track_list.update(cx, |list, cx| list.clear(cx));
            // The listener has had their look at what was new.
            release_watch::acknowledge_all(cx);
            return;
        }
        self.load_if_needed(cx);
        self.load_releases_if_needed(cx);
        self.load_discover_if_needed(cx);
    }

    fn load_if_needed(&mut self, cx: &mut Context<Self>) {
        let range = self.range;
        self.error = None;
        self.top.update(cx, |top, cx| top.ensure(range, cx));
        // Returning to a fresh range still compares it with the longer one.
        if let Some(longer) = range.longer() {
            let loaded = self.top.read(cx).data(range).is_some();
            if loaded && self.visible {
                self.top.update(cx, |top, cx| top.ensure(longer, cx));
            }
        }
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

    fn show_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        let count = self
            .discover
            .as_ref()
            .map_or(0, |discover| discover.suggestions.len());
        if index < count && index != self.discover_index {
            self.discover_index = index;
            self.load_discover_songs(cx);
            cx.notify();
        }
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

    fn select_range(&mut self, range: model::HomeRange, cx: &mut Context<Self>) {
        self.range_menu_open = false;
        if range != self.range {
            self.range = range;
            self.load_if_needed(cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn open_range_menu(&mut self, cx: &mut Context<Self>) {
        self.range_menu_open = true;
        cx.notify();
    }

    /// Closes the time-range menu, reporting whether it was open.
    pub(super) fn close_range_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = std::mem::take(&mut self.range_menu_open);
        if was_open {
            cx.notify();
        }
        was_open
    }

    pub(super) fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.track_list.update(cx, |list, cx| list.close_menu(cx));
        self.close_range_menu(cx);
    }

    /// One story in the briefing: cover, a small tag, a title and a line of
    /// detail. Opens `target` when there is one.
    #[allow(clippy::too_many_arguments)]
    fn story_card(
        &self,
        index: usize,
        artwork: Option<&str>,
        tag: String,
        accent: bool,
        title: String,
        detail: String,
        target: Option<model::ArtistRef>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = appearance::Appearance::palette(cx);
        let cache = services::AppServices::image_cache(cx);
        let cover = tokens::ArtworkSize {
            size: Rems(5.),
            radius: Rems(0.75),
        };
        let tag = div()
            .id(("home-story-tag", index))
            .flex_none()
            .px_1p5()
            .rounded_md()
            .text_xs()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .when(accent, |tag| {
                tag.bg(rgb(palette.link)).text_color(rgb(palette.on_accent))
            })
            .when(!accent, |tag| {
                tag.bg(rgb(palette.control))
                    .text_color(rgb(palette.text_muted))
            })
            .child(tag);
        let body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(tag)
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_primary))
                    .child(title),
            )
            .child(
                components::page_detail(palette, detail)
                    .max_w_full()
                    .truncate(),
            );
        let cover = if artwork.is_some() || target.is_some() {
            components::artwork(palette, &cache, artwork, cover, CadenceIcon::MusicNote)
                .into_any_element()
        } else {
            div()
                .flex_none()
                .size(cover.size)
                .rounded(cover.radius)
                .bg(rgb(palette.control))
                .flex()
                .items_center()
                .justify_center()
                .child(components::icon(
                    CadenceIcon::MusicNote,
                    Rems(1.5),
                    palette.text_muted,
                ))
                .into_any_element()
        };
        let Some(target) = target else {
            return div()
                .id(("home-story", index))
                .test_support()
                .w_full()
                .min_w_0()
                .p_3()
                .flex()
                .items_center()
                .gap_4()
                .rounded_2xl()
                .border_1()
                .border_color(rgb(palette.border))
                .child(cover)
                .child(body)
                .into_any_element();
        };
        components::button(palette, ("home-story", index))
            .test_support()
            .w_full()
            .min_w_0()
            .p_3()
            .gap_4()
            .items_center()
            .justify_start()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(cover)
            .child(body)
            .on_click(
                cx.listener(move |_, _, _, cx| cx.emit(PageEvent::OpenArtist(target.clone()))),
            )
            .into_any_element()
    }

    /// One artist in the list: rank, face, name, and the way their rank moved
    /// against the longer chart.
    fn artist_row(
        &self,
        artist: &model::Artist,
        index: usize,
        change: Option<i32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = appearance::Appearance::palette(cx);
        let cache = services::AppServices::image_cache(cx);
        let artwork = components::rectangular_artwork(
            palette,
            &cache,
            artist.artwork_url.as_deref(),
            tokens::ArtworkSize {
                size: Rems(2.75),
                radius: Rems(1.375),
            },
            Rems(2.75),
            CadenceIcon::Person,
        );
        components::button(palette, ("home-artist", index))
            .test_support()
            .w_full()
            .min_w_0()
            .p_1p5()
            .gap_3()
            .items_center()
            .justify_start()
            .rounded_xl()
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(
                div()
                    .flex_none()
                    .w(Rems(1.25))
                    .text_color(rgb(palette.text_muted))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child((index + 1).to_string()),
            )
            .child(artwork)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(artist.name.clone()),
            )
            .when_some(change, |row, change| {
                let (glyph, color) = if change > 0 {
                    (CadenceIcon::ChevronUp, RISING)
                } else {
                    (CadenceIcon::ChevronDown, palette.danger)
                };
                row.child(
                    div()
                        .id(("home-artist-change", index))
                        .test_support()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .pl_1()
                        .pr_1p5()
                        .rounded_full()
                        .bg(rgb(palette.surface_raised))
                        .border_1()
                        .border_color(rgb(palette.border))
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(rgb(color))
                        .child(components::icon(glyph, Rems(0.625), color))
                        .child(change.unsigned_abs().to_string()),
                )
            })
            // Capture just the index, resolving the current data on activation.
            .on_click(cx.listener(move |this, _, _, cx| {
                let artist = this
                    .top
                    .read(cx)
                    .data(this.range)
                    .and_then(|data| data.artists.get(index).cloned());
                if let Some(artist) = artist {
                    cx.emit(PageEvent::OpenArtist(model::ArtistRef {
                        name: artist.name.clone(),
                        source_id: Some(artist.source_id.clone()),
                        spotify_uri: artist.spotify_uri.clone(),
                    }));
                }
            }))
            .into_any_element()
    }

    /// The artist on offer: why they were picked, who they are, songs to hear
    /// right here, and the few after them.
    fn discover_card(&self, palette: CadencePalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(discover) = self.discover.as_ref() else {
            return div().into_any_element();
        };
        let Some(suggestion) = discover.suggestions.get(self.discover_index) else {
            return div().into_any_element();
        };
        let cache = services::AppServices::image_cache(cx);
        let artist = &suggestion.artist;
        let songs = self.songs_of(&artist.source_id).cloned();
        let because = match suggestion.because.as_slice() {
            [] => String::new(),
            [only] => only.clone(),
            [first, second] => format!("{first} and {second}"),
            [first, second, rest @ ..] => format!("{first}, {second} and {} more", rest.len()),
        };
        let faint = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_muted))
                .child(text)
        };

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_md()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .bg(rgb(palette.link))
                    .text_color(rgb(palette.on_accent))
                    .child("BECAUSE YOU PLAY"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(because),
            )
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(format!(
                        "{} of {}",
                        self.discover_index + 1,
                        discover.suggestions.len()
                    )),
            );

        let play = match songs.as_deref() {
            None => Some(
                components::pending_pill(palette, "home-discover-play-pending", "Play sample")
                    .opacity(0.5)
                    .into_any_element(),
            ),
            Some([]) => None,
            Some(_) => Some(
                components::pill(palette, "home-discover-play", "Play sample", true)
                    .test_support()
                    .on_click(cx.listener(|this, _, _, cx| this.play_sample(0, cx)))
                    .into_any_element(),
            ),
        };
        let buttons = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(play)
            .child(
                components::pill(palette, "home-discover-open", "Open artist", false)
                    .test_support()
                    .on_click(cx.listener(|this, _, _, cx| this.open_suggested_artist(cx))),
            )
            .child(
                components::pill(palette, "home-discover-dismiss", "Not for me", false)
                    .test_support()
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_suggestion(cx))),
            );
        let identity = div()
            .flex()
            .items_center()
            .gap_4()
            .child(components::rectangular_artwork(
                palette,
                &cache,
                artist.artwork_url.as_deref(),
                tokens::ArtworkSize {
                    size: Rems(7.),
                    radius: Rems(3.5),
                },
                Rems(7.),
                CadenceIcon::Person,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .id("home-discover-name")
                            .test_support()
                            .max_w_full()
                            .truncate()
                            .text_2xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(artist.name.clone()),
                    )
                    .child(components::page_detail(
                        palette,
                        format!("{}% match", suggestion.match_percent),
                    )),
            );

        let song_rows: Vec<AnyElement> = songs
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, track)| {
                components::button(palette, ("home-discover-song", index))
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .p_1p5()
                    .gap_3()
                    .items_center()
                    .justify_start()
                    .rounded_xl()
                    .hover(|style| style.bg(rgb(palette.surface_hover)))
                    .child(components::artwork(
                        palette,
                        &cache,
                        track.artwork_url.as_deref(),
                        tokens::ArtworkSize {
                            size: Rems(2.5),
                            radius: Rems(0.5),
                        },
                        CadenceIcon::MusicNote,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(format_duration(track.duration_ms)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.play_sample(index, cx)))
                    .into_any_element()
            })
            .collect();

        let next: Vec<AnyElement> = discover
            .suggestions
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.discover_index)
            .take(DISCOVER_NEXT_SHOWN)
            .map(|(index, other)| {
                components::button(palette, ("home-discover-next", index))
                    .test_support()
                    .w(Rems(5.5))
                    .flex_none()
                    .flex_col()
                    .items_center()
                    .gap_1p5()
                    .p_1p5()
                    .rounded_xl()
                    .hover(|style| style.bg(rgb(palette.surface_hover)))
                    .child(components::rectangular_artwork(
                        palette,
                        &cache,
                        other.artist.artwork_url.as_deref(),
                        tokens::ArtworkSize {
                            size: Rems(3.25),
                            radius: Rems(1.625),
                        },
                        Rems(3.25),
                        CadenceIcon::Person,
                    ))
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(palette.text))
                            .child(other.artist.name.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show_suggestion(index, cx)))
                    .into_any_element()
            })
            .collect();

        div()
            .id("home-discover")
            .test_support()
            .w_full()
            .min_w_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .child(header)
            .child(identity)
            .child(buttons)
            .when(!song_rows.is_empty(), |card| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(faint("THEIR TOP SONGS"))
                        .children(song_rows),
                )
            })
            .when(!next.is_empty(), |card| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(faint("UP NEXT"))
                        .child(div().flex().flex_wrap().gap_1().children(next)),
                )
            })
            .into_any_element()
    }

    /// The time range as one button that opens a short menu.
    /// The ways back into the library, in the order they are shown. A tile is
    /// left out when there is nothing behind it yet.
    fn shortcuts(&self, cx: &App) -> Vec<Shortcut> {
        let library = services::AppServices::library(cx);
        let library = library.read(cx);
        let mut tiles = Vec::new();
        tiles.push(Shortcut {
            url: None,
            icon: CadenceIcon::HeartFilled,
            round: false,
            title: "Liked Songs".into(),
            detail: if library.loaded() {
                format!("{} songs", library.liked_tracks().len())
            } else {
                "Songs you liked".to_owned()
            },
            event: PageEvent::Navigate(Route::LikedSongs),
        });
        tiles.push(Shortcut {
            url: None,
            icon: CadenceIcon::ClockFilled,
            round: false,
            title: "Recently played".into(),
            detail: "What you played lately".to_owned(),
            event: PageEvent::Navigate(Route::Recent),
        });
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
                round: false,
                title: album.name.clone(),
                detail: format!("Album · {}", track.artist),
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
                round: false,
                title: playlist.name.clone(),
                detail: "Playlist".to_owned(),
                event: PageEvent::OpenPlaylist(playlist.clone()),
            });
        }
        if let Some(artist) = self
            .top
            .read(cx)
            .data(self.range)
            .and_then(|data| data.artists.first())
        {
            tiles.push(Shortcut {
                url: artist.artwork_url.clone(),
                icon: CadenceIcon::Person,
                round: true,
                title: artist.name.clone(),
                detail: "Artist · your most played".to_owned(),
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
                round: false,
                title: "Local Files".into(),
                detail: format!("{} songs", library.local_tracks().len()),
                event: PageEvent::Navigate(Route::LocalFiles),
            }
        } else {
            Shortcut {
                url: None,
                icon: CadenceIcon::PlaylistFilled,
                round: false,
                title: "Playlists".into(),
                detail: "All your playlists".to_owned(),
                event: PageEvent::Navigate(Route::Playlists),
            }
        });
        tiles
    }

    fn shortcut_tile(
        &self,
        index: usize,
        shortcut: Shortcut,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cache = services::AppServices::image_cache(cx);
        let art = components::artwork(
            palette,
            &cache,
            shortcut.url.as_deref(),
            tokens::ArtworkSize {
                size: Rems(3.5),
                radius: if shortcut.round {
                    Rems(1.75)
                } else {
                    Rems(0.625)
                },
            },
            shortcut.icon,
        );
        let event = shortcut.event;
        components::button(palette, ("home-shortcut", index))
            .test_support()
            .w(Rems(18.))
            .flex_none()
            .p_2()
            .gap_3()
            .items_center()
            .justify_start()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(art)
            .child(
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
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(shortcut.title),
                    )
                    .child(
                        components::page_detail(palette, shortcut.detail)
                            .max_w_full()
                            .truncate(),
                    ),
            )
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
                size: Rems(7.),
                radius: Rems(0.875),
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
                format!(
                    "{} / {}",
                    format_duration(position),
                    format_duration(track.duration_ms)
                ),
            )
        });
        Some(
            div()
                .id("home-continue")
                .test_support()
                .w_full()
                .p_4()
                .flex()
                .gap_4()
                .rounded_2xl()
                .border_1()
                .border_color(rgb(palette.border))
                .child(art)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .child(
                                    div()
                                        .truncate()
                                        .text_lg()
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette.text_primary))
                                        .child(track.title.clone()),
                                )
                                .child(
                                    components::page_detail(palette, track.artist.to_string())
                                        .truncate(),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap_2()
                                .when_some(progress, |column, (fraction, text)| {
                                    column
                                        .w_full()
                                        .child(
                                            div()
                                                .id("home-continue-progress")
                                                .test_support()
                                                .w_full()
                                                .h(px(4.))
                                                .rounded_full()
                                                .bg(rgb(palette.control))
                                                .child(
                                                    div()
                                                        .h_full()
                                                        .w(relative(fraction))
                                                        .rounded_full()
                                                        .bg(rgb(palette.text_primary)),
                                                ),
                                        )
                                        .child(components::page_detail(palette, text))
                                })
                                .child(
                                    components::icon_pill(
                                        palette,
                                        "home-continue-play",
                                        icon,
                                        label,
                                        true,
                                    )
                                    .test_support()
                                    .on_click(cx.listener(
                                        move |_, _, _, cx| {
                                            if resuming {
                                                player.update(cx, |player, cx| player.toggle(cx));
                                            } else {
                                                let list =
                                                    library.read(cx).recently_played().clone();
                                                player.update(cx, |player, cx| {
                                                    player.play_context(list, 0, cx);
                                                });
                                            }
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The newest albums from artists you play, as covers.
    fn releases_shelf(
        &self,
        palette: CadencePalette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let cache = services::AppServices::image_cache(cx);
        let size = tokens::ArtworkSize {
            size: Rems(8.),
            radius: Rems(0.875),
        };
        let Some(releases) = self.releases.as_ref() else {
            // Placeholders hold the shelf's height while Spotify answers.
            return self.releases_loading.then(|| {
                div()
                    .id("home-releases-loading")
                    .test_support()
                    .flex()
                    .gap_4()
                    .children((0..RELEASES_SHOWN).map(|_| {
                        div()
                            .size(size.size)
                            .rounded(size.radius)
                            .bg(rgb(palette.control))
                    }))
                    .into_any_element()
            });
        };
        if releases.albums.is_empty() {
            return None;
        }
        let mut shelf = div().id("home-releases").test_support().flex().gap_4();
        for (index, album) in releases.albums.iter().take(RELEASES_SHOWN).enumerate() {
            let is_new = release_watch::is_new(&album.source_id, cx);
            let artist = album
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
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
                    .gap_1p5()
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
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(album.name.clone()),
                    )
                    .child(
                        components::page_detail(
                            palette,
                            if is_new {
                                format!("New · {artist}")
                            } else {
                                artist
                            },
                        )
                        .w_full()
                        .truncate(),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(PageEvent::OpenAlbum(target.clone()))
                    })),
            );
        }
        Some(shelf.into_any_element())
    }

    fn range_menu(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let selected = self.range;
        div().flex().child(
            div()
                .relative()
                .flex_none()
                .child(
                    components::pill(palette, "home-range-menu", selected.label(), false)
                        .test_support()
                        .gap_2()
                        .child(components::icon(
                            if self.range_menu_open {
                                CadenceIcon::ChevronUp
                            } else {
                                CadenceIcon::ChevronDown
                            },
                            tokens::INLINE_ICON,
                            palette.text_primary,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.range_menu_open = !this.range_menu_open;
                            cx.notify();
                        })),
                )
                .when(self.range_menu_open, |anchor| {
                    anchor.child(deferred(
                        components::solid_menu_surface(palette)
                            .on_mouse_up_out(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.close_menus(cx)),
                            )
                            .absolute()
                            .top(Rems(2.75))
                            .right_0()
                            .w(Rems(13.))
                            .children(model::HomeRange::ALL.into_iter().map(|range| {
                                let chosen = range == selected;
                                components::button(palette, ("home-range", range.index()))
                                    .test_support()
                                    .w_full()
                                    .h_9()
                                    .px_2p5()
                                    .justify_between()
                                    .rounded_lg()
                                    .text_size(tokens::BODY_TEXT)
                                    .text_color(rgb(palette.text_primary))
                                    .when(chosen, |row| {
                                        row.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    })
                                    .hover(|style| style.bg(rgb(palette.control_hover)))
                                    .child(range.label())
                                    .when(chosen, |row| {
                                        row.child(components::icon(
                                            CadenceIcon::Check,
                                            tokens::MENU_ICON,
                                            palette.text_primary,
                                        ))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_range(range, cx)
                                    }))
                            })),
                    ))
                }),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let heading = |id, text: SharedString| {
            div()
                .id(id)
                .test_support()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_primary))
                .child(text)
        };
        let shortcuts: Vec<AnyElement> = self
            .shortcuts(cx)
            .into_iter()
            .enumerate()
            .map(|(index, shortcut)| self.shortcut_tile(index, shortcut, palette, cx))
            .collect();
        let continue_card = self.continue_card(palette, cx);
        let shelf = self.releases_shelf(palette, cx);
        let column = |heading_id, title: &str, body: AnyElement| {
            div()
                .flex_1()
                .min_w(Rems(22.))
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .h_10()
                        .flex()
                        .items_center()
                        .child(heading(heading_id, title.to_owned().into())),
                )
                .child(body)
        };
        let first_row = div()
            .flex()
            .flex_col()
            .gap_6()
            .mb_6()
            .when(!shortcuts.is_empty(), |block| {
                block.child(
                    div()
                        .id("home-shortcuts")
                        .test_support()
                        .flex()
                        .flex_wrap()
                        .gap_3()
                        .children(shortcuts),
                )
            })
            .when(continue_card.is_some() || shelf.is_some(), |block| {
                block.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x_10()
                        .gap_y_6()
                        .children(
                            continue_card
                                .map(|card| column("home-continue-heading", "Continue", card)),
                        )
                        .children(shelf.map(|shelf| {
                            column("home-releases-heading", "New from artists you play", shelf)
                        })),
                )
            });
        let (permission_missing, current, compare) = {
            let top = self.top.read(cx);
            (
                top.permission_missing(),
                top.data(self.range)
                    .map(|data| (data.artists.clone(), data.tracks.clone())),
                self.range
                    .longer()
                    .and_then(|longer| top.data(longer))
                    .map(|data| data.artists.clone()),
            )
        };
        let loaded = current.is_some();
        let (artists, tracks) = current.unwrap_or_default();
        // The range lives beside the top artists; until there is a page to
        // hold it, and when there is nothing to list, it stands alone.
        let range_alone =
            !permission_missing && !(loaded && !(artists.is_empty() && tracks.is_empty()));
        let content = if permission_missing {
            self.permission_prompt(palette, cx)
        } else if self.error.is_some() && !loaded {
            div().child(components::empty_state(
                palette,
                CadenceIcon::MusicNote,
                "Couldn’t load your top artists and tracks",
                Some("Spotify didn’t answer. Try again in a moment.".into()),
            ))
        } else if !loaded {
            div().child(components::list_skeleton_rows(palette, 4))
        } else if artists.is_empty() && tracks.is_empty() {
            div().child(components::empty_state(
                palette,
                CadenceIcon::MusicNote,
                "Nothing here yet",
                Some(
                    "Listen to more music and your top artists and tracks will show up here."
                        .into(),
                ),
            ))
        } else {
            self.track_list.update(cx, |list, cx| {
                list.show(("home-tracks", self.range.index()), tracks.clone(), cx);
            });
            let upcoming = self
                .upcoming
                .as_ref()
                .map_or_else(|| Arc::from([]), |upcoming| upcoming.releases.clone());
            let upcoming_pending = self.upcoming.is_none() && self.upcoming_loading;
            let mut stories: Vec<AnyElement> = Vec::new();
            for release in upcoming.iter().take(STORY_UPCOMING) {
                let when = release_label(&release.date).unwrap_or_else(|| release.date.clone());
                let index = stories.len();
                stories.push(self.story_card(
                    index,
                    None,
                    format!("COMING {}", when.to_uppercase()),
                    false,
                    release.title.clone(),
                    format!("{} · {}", release.artist, release.kind),
                    None,
                    cx,
                ));
            }
            if let (Some(longer_range), Some(longer)) = (self.range.longer(), compare.as_ref()) {
                let quiet = longer
                    .iter()
                    .take(COME_BACK_RANK)
                    .enumerate()
                    .filter(|(_, artist)| {
                        !artists
                            .iter()
                            .any(|current| current.source_id == artist.source_id)
                    })
                    .take(STORY_COME_BACK);
                for (rank, artist) in quiet {
                    let index = stories.len();
                    stories.push(self.story_card(
                        index,
                        artist.artwork_url.as_deref(),
                        "COME BACK TO".to_owned(),
                        false,
                        format!("{}, quiet lately", artist.name),
                        format!(
                            "#{} for {} · not in {}",
                            rank + 1,
                            longer_range.short_label(),
                            self.range.short_label()
                        ),
                        Some(model::ArtistRef {
                            name: artist.name.clone(),
                            source_id: Some(artist.source_id.clone()),
                            spotify_uri: artist.spotify_uri.clone(),
                        }),
                        cx,
                    ));
                }
            }
            let stories_pending = upcoming_pending && stories.is_empty();
            let shown_artists = if self.all_artists {
                artists.len()
            } else {
                artists.len().min(ARTISTS_SHOWN)
            };
            let mut artist_rows = div().flex().flex_col();
            for (index, artist) in artists.iter().take(shown_artists).enumerate() {
                let change = compare.as_ref().and_then(|longer| {
                    let before = longer
                        .iter()
                        .position(|other| other.source_id == artist.source_id)?;
                    let change = before as i32 - index as i32;
                    (change != 0).then_some(change)
                });
                artist_rows = artist_rows.child(self.artist_row(artist, index, change, cx));
            }
            let more = artists.len() > ARTISTS_SHOWN;
            let all_artists = self.all_artists;
            let offered = self
                .discover
                .as_ref()
                .is_some_and(|discover| !discover.suggestions.is_empty());
            let discover_pending = self.discover.is_none() && self.discover_loading;
            let stories_empty = stories.is_empty();
            // Stories keep the left of the top row until there is an artist to
            // try, which takes their place and sends the stories below.
            let stories_heading = div()
                .h_10()
                .flex()
                .items_center()
                .child(heading("home-stories-heading", "For you".into()));
            let stories_body = div()
                .flex()
                .flex_col()
                .gap_3()
                .when(stories_pending, |column| {
                    column.child(release_skeleton(palette))
                })
                .when(!stories_pending && stories_empty, |column| {
                    column.child(components::page_detail(
                        palette,
                        "Nothing announced for your top artists yet. Dates come from MusicBrainz, so some may be missing.",
                    ))
                });
            let left: Option<AnyElement> =
                if offered {
                    Some(
                        div()
                            .flex_1()
                            .min_w(Rems(24.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div().h_10().flex().items_center().child(heading(
                                    "home-discover-heading",
                                    "Try this next".into(),
                                )),
                            )
                            .child(self.discover_card(palette, cx))
                            .into_any_element(),
                    )
                } else if discover_pending {
                    Some(
                        div()
                            .flex_1()
                            .min_w(Rems(24.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div().h_10().flex().items_center().child(heading(
                                    "home-discover-heading",
                                    "Try this next".into(),
                                )),
                            )
                            .child(
                                div()
                                    .id("home-discover-loading")
                                    .test_support()
                                    .h(Rems(22.))
                                    .rounded_2xl()
                                    .bg(rgb(palette.control)),
                            )
                            .into_any_element(),
                    )
                } else {
                    None
                };
            let stories_in_top = left.is_none();
            let stories_below = !stories_in_top;
            let top_stories = stories_in_top.then(|| {
                div()
                    .flex_1()
                    .min_w(Rems(22.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(stories_heading)
                    .children((stories_pending || stories_empty).then_some(stories_body))
                    .children(std::mem::take(&mut stories))
                    .into_any_element()
            });
            let below_stories = (stories_below && !stories.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .h_10()
                            .flex()
                            .items_center()
                            .child(heading("home-stories-heading", "For you".into())),
                    )
                    .child(
                        div().flex().flex_wrap().gap_3().children(
                            stories
                                .drain(..)
                                .map(|story| {
                                    div()
                                        .flex_1()
                                        .min_w(Rems(24.))
                                        .child(story)
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>(),
                        ),
                    )
            });
            div()
                .flex()
                .flex_col()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x_10()
                        .gap_y_6()
                        .children(left)
                        .children(top_stories)
                        .when(!permission_missing, |row| {
                            row.child(
                                div()
                                    .flex_1()
                                    .min_w(Rems(20.))
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(
                                        div()
                                            .h_10()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_3()
                                            .child(heading(
                                                "home-artists-heading",
                                                if all_artists || artists.is_empty() {
                                                    SharedString::from("Your top artists")
                                                } else {
                                                    SharedString::from(format!(
                                                        "Your top {}",
                                                        artists.len().min(ARTISTS_SHOWN)
                                                    ))
                                                },
                                            ))
                                            .child(self.range_menu(palette, cx)),
                                    )
                                    .child(artist_rows)
                                    .when(artists.is_empty(), |column| {
                                        column.child(components::page_detail(
                                            palette,
                                            "No top artists for this stretch yet.",
                                        ))
                                    })
                                    .when(more, |column| {
                                        column.child(
                                            div().flex().child(
                                                components::pill(
                                                    palette,
                                                    "home-artists-more",
                                                    if all_artists {
                                                        "Show less".to_owned()
                                                    } else {
                                                        format!("See all {}", artists.len())
                                                    },
                                                    false,
                                                )
                                                .test_support()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.all_artists = !this.all_artists;
                                                    cx.notify();
                                                })),
                                            ),
                                        )
                                    })
                                    .when(compare.is_some() && !artists.is_empty(), |column| {
                                        column.child(components::page_detail(
                                            palette,
                                            "Arrows compare with a longer stretch.",
                                        ))
                                    }),
                            )
                        }),
                )
                .children(below_stories)
                .when(!tracks.is_empty(), |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(heading("home-tracks-heading", "Your top tracks".into()))
                            .child(self.track_list.clone()),
                    )
                })
        };
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
                            .child(first_row)
                            .when(range_alone, |page| {
                                page.child(div().mb_5().child(self.range_menu(palette, cx)))
                            })
                            .child(content),
                    ),
            )
    }
}

/// Placeholder rows the size of a release row, so the section does not change
/// height when the data arrives.
fn release_skeleton(palette: CadencePalette) -> Div {
    div().flex().flex_col().children((0..3).map(|_| {
        div()
            .p_1p5()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(Rems(3.))
                    .rounded(Rems(0.5))
                    .bg(rgb(palette.control)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .h_3()
                            .w(relative(0.6))
                            .rounded_md()
                            .bg(rgb(palette.control)),
                    )
                    .child(
                        div()
                            .h_2()
                            .w(relative(0.4))
                            .rounded_md()
                            .bg(rgb(palette.control)),
                    ),
            )
    }))
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
    use super::{greeting_for, release_label};

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
    fn release_dates_read_as_month_and_day() {
        assert_eq!(release_label("2026-11-07").as_deref(), Some("Nov 7"));
        assert_eq!(release_label("2026-01-31").as_deref(), Some("Jan 31"));
        assert_eq!(release_label("2026-11"), None);
        assert_eq!(release_label("2026"), None);
        assert_eq!(release_label("2026-13-01"), None);
    }
}
