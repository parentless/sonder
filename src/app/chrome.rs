use super::*;
use gpui_kit::TestSupportExt as _;

/// What the toolbar asks the workspace to do.
pub(super) enum ToolbarEvent {
    QueryChanged(String),
    SubmitSearch,
    OpenArtist(model::ArtistRef),
    OpenAlbum(model::AlbumRef),
    Navigate(Route),
    OpenSettings,
    Connect,
    Logout,
    /// The account menu opened, so anything else overlaying the page should go.
    MenuOpened,
}

/// How long typing must pause, and how many letters it needs, before the
/// dropdown asks Spotify. Answers are saved per query, so Enter reuses them.
const LIVE_SEARCH_PAUSE: Duration = Duration::from_millis(500);
const LIVE_SEARCH_MIN_CHARS: usize = 3;

/// One Spotify answer offered under "On Spotify".
#[derive(Clone)]
enum LiveResult {
    Artist(model::Artist),
    Album(model::Album),
    Track(model::Track),
}

impl LiveResult {
    /// The few best matches: one artist, two albums, two songs.
    fn pick(results: model::CatalogSearchResults) -> Vec<Self> {
        let mut live = Vec::new();
        live.extend(results.artists.into_iter().take(1).map(Self::Artist));
        live.extend(results.albums.into_iter().take(2).map(Self::Album));
        live.extend(results.tracks.into_iter().take(2).map(Self::Track));
        live
    }

    /// What a row shows: title, the line under it, the quiet label on the
    /// right, the picture (if Spotify gave one) and the shape it sits in.
    fn parts(&self) -> LiveParts {
        match self {
            Self::Artist(artist) => LiveParts {
                title: artist.name.clone(),
                detail: "Artist".into(),
                tag: "Artist",
                artwork_url: artist.artwork_url.clone(),
                icon: CadenceIcon::Person,
                round: true,
            },
            Self::Album(album) => LiveParts {
                title: album.name.clone(),
                detail: album
                    .artists
                    .first()
                    .map(|artist| artist.name.clone())
                    .unwrap_or_else(|| "Album".into()),
                tag: "Album",
                artwork_url: album.artwork_url.clone(),
                icon: CadenceIcon::MusicNote,
                round: false,
            },
            Self::Track(track) => LiveParts {
                title: track.title.clone(),
                detail: track.artist.clone(),
                tag: "Song",
                artwork_url: track.artwork_url.clone(),
                icon: CadenceIcon::MusicNote,
                round: false,
            },
        }
    }
}

struct LiveParts {
    title: SharedString,
    detail: SharedString,
    tag: &'static str,
    artwork_url: Option<String>,
    icon: CadenceIcon,
    round: bool,
}

/// `text` with the letters the listener typed picked out in bold. A word the
/// text does not contain, or one whose letter case changes its length, is
/// shown plain.
fn highlighted(text: &str, query: &str, palette: CadencePalette) -> gpui_kit::AnyElement {
    let lower = text.to_lowercase();
    let wanted = query.trim().to_lowercase();
    let range = (lower.len() == text.len() && !wanted.is_empty())
        .then(|| lower.find(&wanted).map(|start| start..start + wanted.len()))
        .flatten()
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
    let Some(range) = range else {
        return div().child(text.to_owned()).into_any_element();
    };
    div()
        .child(
            gpui_kit::StyledText::new(text.to_owned()).with_highlights([(
                range,
                gpui_kit::HighlightStyle {
                    color: Some(rgb(palette.text_primary).into()),
                    font_weight: Some(gpui_kit::FontWeight::SEMIBOLD),
                    ..gpui_kit::HighlightStyle::default()
                },
            )]),
        )
        .into_any_element()
}

/// The bar above the page: search on the left, account on the right.
pub(super) struct Toolbar {
    search_input: Entity<InputState>,
    suggestions: Vec<cached_search::Match>,
    suggestion_query: String,
    suggestion_selection: Option<usize>,
    /// Whether the "Search Spotify for ..." row is offered under the field.
    search_row: bool,
    /// Spotify's answer for the words typed, shown under the library matches.
    live: Vec<LiveResult>,
    live_task: Option<gpui_kit::Task<()>>,
    /// Counts Spotify's answers, so each one can settle in on its own.
    live_serial: u64,
    backend: BackendHandle,
    suggestion_task: Option<gpui_kit::Task<()>>,
    search_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    library: Entity<library::Library>,
    pub(super) artist_header: Option<Entity<artist_header::ArtistHeader>>,
    menu_open: bool,
    /// When a press outside the menu last closed it. The avatar's own click
    /// lands just after, and must not open the menu again.
    menu_dismissed_at: Option<std::time::Instant>,
    /// 0 when the account menu is closed, 1 when open; it eases between.
    menu_motion: gpui_motion_kit::ValueState,
    session: Entity<session::Session>,
    player: Entity<player::Player>,
    route: Route,
    /// Where the back button goes, when the current route has one.
    back_target: Option<Route>,
    /// The workspace's standing failure, shown under the account name.
    error: Option<SharedString>,
    _search_subscription: Subscription,
    _library_events: Subscription,
}

impl EventEmitter<ToolbarEvent> for Toolbar {}

impl Toolbar {
    pub(super) fn new(backend: BackendHandle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search Spotify"));
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |this, input, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    this.queue_suggestions(&query, cx);
                    cx.emit(ToolbarEvent::QueryChanged(query));
                }
                InputEvent::PressEnter { .. } => match this.suggestion_selection {
                    Some(index) if index < this.suggestions.len() => {
                        this.play_suggestion(index, cx)
                    }
                    Some(index) if index < this.suggestions.len() + this.live.len() => {
                        this.open_live(index - this.suggestions.len(), cx)
                    }
                    _ => this.submit_search_from_field(cx),
                },
                _ => {}
            },
        );
        let library = services::AppServices::library(cx);
        // A like made from the dropdown fills its heart once Spotify agrees.
        let library_events = cx.observe(&library, |this: &mut Toolbar, _, cx| {
            if this
                .live
                .iter()
                .any(|found| matches!(found, LiveResult::Track(_)))
            {
                cx.notify();
            }
        });
        Self {
            search_input,
            suggestions: Vec::new(),
            suggestion_query: String::new(),
            suggestion_selection: None,
            search_row: false,
            live: Vec::new(),
            live_task: None,
            live_serial: 0,
            backend,
            suggestion_task: None,
            search_bounds: Rc::default(),
            library,
            artist_header: None,
            menu_open: false,
            menu_dismissed_at: None,
            menu_motion: gpui_motion_kit::ValueState::new(0.),
            session: services::AppServices::session(cx),
            player: services::AppServices::player(cx),
            route: Route::LikedSongs,
            back_target: None,
            error: None,
            _search_subscription: search_subscription,
            _library_events: library_events,
        }
    }

    #[cfg(test)]
    pub(super) fn seed_cached_suggestions(&mut self, cx: &mut Context<Self>) {
        self.dismiss_suggestions(cx);
        let library = self.library.read(cx);
        self.suggestions = cached_search::find(
            "Song",
            &[
                library.favorites().clone(),
                library.recently_played().clone(),
                library.liked_tracks().clone(),
            ],
        );
        self.suggestion_query = "Song".into();
        self.live = LiveResult::pick(model::CatalogSearchResults {
            artists: vec![model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "artist".into(),
                spotify_uri: None,
                name: "Song Exploder".into(),
                artwork_url: None,
            }],
            albums: vec![model::Album {
                provider: model::Provider::Spotify,
                source_id: "album".into(),
                spotify_uri: None,
                name: "Songs for the Deaf".into(),
                artists: vec![model::ArtistRef {
                    name: "Sonder Ensemble".into(),
                    source_id: None,
                    spotify_uri: None,
                }],
                release_date: None,
                artwork_url: None,
                track_count: None,
            }],
            // Not in Liked Songs, so its heart shows empty.
            tracks: vec![test_support::track(100)],
        });
        self.search_row = true;
        self.suggestion_selection = Some(1);
        cx.notify();
    }

    /// Pushes the workspace state the toolbar reflects but does not own.
    pub(super) fn show(
        &mut self,
        route: Route,
        back_target: Option<Route>,
        error: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.route != route || self.back_target != back_target || self.error != error {
            if self.route != route {
                self.dismiss_suggestions(cx);
            }
            self.route = route;
            self.back_target = back_target;
            self.error = error;
            cx.notify();
        }
    }

    pub(super) fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
    }

    pub(super) fn clear_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu_open {
            self.menu_open = false;
            cx.notify();
        }
    }

    /// Asks Spotify, once, for what is typed. This is the only thing that
    /// searches Spotify: typing alone never does.
    fn submit_search_from_field(&mut self, cx: &mut Context<Self>) {
        self.dismiss_suggestions(cx);
        cx.emit(ToolbarEvent::SubmitSearch);
    }

    /// The suggestion list: what the library matched plus the Spotify row.
    fn suggestions_open(&self) -> bool {
        !self.suggestions.is_empty() || !self.live.is_empty() || self.search_row
    }

    pub(super) fn close_suggestions(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.suggestions_open() || self.suggestion_task.is_some();
        self.dismiss_suggestions(cx);
        open
    }

    fn dismiss_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggestion_task = None;
        self.live_task = None;
        self.suggestion_selection = None;
        if self.suggestions_open() {
            self.suggestions.clear();
            self.live.clear();
            self.search_row = false;
            cx.notify();
        }
    }

    fn queue_suggestions(&mut self, query: &str, cx: &mut Context<Self>) {
        self.dismiss_suggestions(cx);
        self.suggestion_query = query.trim().chars().take(96).collect();
        if self.suggestion_query.chars().count() < 2 {
            return;
        }
        let query = self.suggestion_query.clone();
        if query.chars().count() >= LIVE_SEARCH_MIN_CHARS {
            self.queue_live(query.clone(), cx);
        }
        self.suggestion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let Ok(sources) = this.update(cx, |toolbar, cx| {
                let library = toolbar.library.read(cx);
                [
                    library.favorites().clone(),
                    library.recently_played().clone(),
                    library.liked_tracks().clone(),
                ]
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { cached_search::find(&query, &sources) })
                .await;
            let _ = this.update(cx, |toolbar, cx| {
                toolbar.suggestion_task = None;
                toolbar.suggestions = result;
                toolbar.search_row = true;
                cx.notify();
            });
        }));
    }

    /// Asks Spotify once typing has paused. A failure (a rate limit included)
    /// just leaves the section out; the library matches still work.
    fn queue_live(&mut self, query: String, cx: &mut Context<Self>) {
        self.live_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LIVE_SEARCH_PAUSE).await;
            let Ok(reply) = this.update(cx, |toolbar, _| {
                catalog::request(&toolbar.backend, |respond| BackendCommand::SearchCatalog {
                    query,
                    respond,
                })
            }) else {
                return;
            };
            let Ok(results) = reply.await else {
                // No answer: stop waiting for one and keep the library rows.
                let _ = this.update(cx, |toolbar, cx| {
                    toolbar.live_task = None;
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |toolbar, cx| {
                toolbar.live_task = None;
                toolbar.live = LiveResult::pick(results);
                toolbar.live_serial += 1;
                cx.notify();
            });
        }));
    }

    fn open_live(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(found) = self.live.get(index).cloned() else {
            return;
        };
        self.dismiss_suggestions(cx);
        match found {
            LiveResult::Artist(artist) => cx.emit(ToolbarEvent::OpenArtist(model::ArtistRef {
                name: artist.name,
                source_id: Some(artist.source_id),
                spotify_uri: artist.spotify_uri,
            })),
            LiveResult::Album(album) => cx.emit(ToolbarEvent::OpenAlbum(model::AlbumRef {
                name: album.name,
                source_id: Some(album.source_id),
                spotify_uri: album.spotify_uri,
                artwork_url: album.artwork_url,
            })),
            LiveResult::Track(track) => self.player.update(cx, |player, cx| {
                player.play_context(vec![track], 0, cx);
            }),
        }
    }

    fn play_suggestion(&mut self, index: usize, cx: &mut Context<Self>) {
        let found = self.suggestions.get(index).cloned();
        self.play_cached(found.as_ref(), cx);
    }

    fn play_cached(&mut self, found: Option<&cached_search::Match>, cx: &mut Context<Self>) {
        let track = found
            .and_then(|found| found.resolve(self.library.read(cx)))
            .cloned();
        self.dismiss_suggestions(cx);
        if let Some(track) = track {
            self.player.update(cx, |player, cx| {
                player.play_context(vec![track], 0, cx);
            });
        }
    }

    /// One result: a picture, the title with the typed letters in bold, the
    /// line under it and a quiet label on the right.
    #[allow(clippy::too_many_arguments)]
    fn suggestion_row(
        &self,
        palette: CadencePalette,
        id: (&'static str, usize),
        art: gpui_kit::AnyElement,
        title: gpui_kit::AnyElement,
        detail: SharedString,
        tag: &'static str,
        like: Option<gpui_kit::AnyElement>,
        selected: bool,
    ) -> Stateful<Div> {
        components::button(palette, id)
            .w_full()
            .h(px(48.))
            .px(px(10.))
            .gap(px(12.))
            .rounded(px(9.))
            .justify_start()
            .when(selected, |row| row.bg(rgb(palette.control_hover)))
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(art)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(rgb(palette.text))
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(detail),
                    ),
            )
            .child(match like {
                // A heart already says "Song", so it takes the label's place.
                Some(like) => like,
                None => div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(rgb(palette.text_muted))
                    .opacity(0.8)
                    .child(tag)
                    .into_any_element(),
            })
    }

    /// The heart on a Spotify song in the dropdown. Songs already in the
    /// library are not offered one: they sit under "In library" and are
    /// liked or not by the page they came from.
    fn like_button(
        &self,
        palette: CadencePalette,
        index: usize,
        track: &model::Track,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let library = self.library.read(cx);
        let liked = library.is_spotify_liked(track);
        let busy = library.spotify_like_busy();
        let label = if liked {
            "Remove from Liked Songs"
        } else {
            "Add to Liked Songs"
        };
        let track = track.clone();
        gpui_kit::base::Button::new(("live-like", index))
            .cursor_pointer()
            .accessibility_label(label)
            .tab_stop(false)
            .size_8()
            .flex_none()
            .rounded_lg()
            .when(busy, |button| button.opacity(0.6))
            .hover(|style| style.bg(rgb(palette.control)))
            .active(|style| style.bg(rgb(palette.control_hover)))
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| {
                // Keep the typing cursor in the field while the heart is pressed.
                window.prevent_default();
            })
            .child(if liked {
                components::icon(CadenceIcon::HeartFilled, tokens::LIKE_ICON, palette.text)
            } else {
                components::icon(CadenceIcon::Heart, tokens::LIKE_ICON, palette.text)
            })
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    // Liking is not opening: the row behind must not play the song.
                    cx.stop_propagation();
                    if event.is_keyboard() || event.click_count() == 1 {
                        let liked = this.library.read(cx).is_spotify_liked(&track);
                        this.library.update(cx, |library, cx| {
                            library.set_spotify_liked(track.clone(), !liked, cx)
                        });
                    }
                }),
            )
            .into_any_element()
    }

    fn suggestion_heading(palette: CadencePalette, label: &'static str, count: usize) -> Div {
        div()
            .px(px(8.))
            .pt(px(8.))
            .pb(px(4.))
            .flex()
            .justify_between()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(label)
            .when(count > 0, |head| {
                head.child(div().opacity(0.65).child(count.to_string()))
            })
    }

    /// Stand-ins while Spotify answers.
    fn suggestion_placeholders(palette: CadencePalette, count: usize) -> Div {
        let block = |width: f32, height: f32| {
            div()
                .w(px(width))
                .h(px(height))
                .rounded(px(5.))
                .bg(rgb(palette.surface_hover))
        };
        div().flex().flex_col().children((0..count).map(move |_| {
            div()
                .h(px(48.))
                .px(px(10.))
                .gap(px(12.))
                .flex()
                .items_center()
                .child(
                    div()
                        .size(px(34.))
                        .flex_none()
                        .rounded(px(7.))
                        .bg(rgb(palette.surface_hover)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(block(150., 9.))
                        .child(block(90., 7.)),
                )
        }))
    }

    fn suggestion_menu(
        &self,
        palette: CadencePalette,
        width: Rems,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let library = self.library.read(cx);
        let images = services::AppServices::image_cache(cx);
        let motion = !cx.reduce_motion();
        let query = self.suggestion_query.clone();
        let wants_live = query.chars().count() >= LIVE_SEARCH_MIN_CHARS;
        let waiting = wants_live && self.live.is_empty() && self.live_task.is_some();
        let tile = |url: Option<&str>, icon: CadenceIcon, round: bool| {
            components::artwork(
                palette,
                &images,
                url,
                if round {
                    tokens::SUGGESTION_ARTIST_ARTWORK
                } else {
                    tokens::SUGGESTION_ARTWORK
                },
                icon,
            )
        };
        // Solid, so the page title behind it never shows through the rows.
        let mut menu = components::solid_menu_surface(palette)
            .id("cached-search-suggestions")
            .test_support()
            .absolute()
            .top_12()
            .left_0()
            .w(width)
            .rounded(px(14.));

        let found: Vec<(usize, &model::Track)> = self
            .suggestions
            .iter()
            .enumerate()
            .filter_map(|(index, found)| Some((index, found.resolve(library)?)))
            .collect();
        if !found.is_empty() {
            menu = menu.child(Self::suggestion_heading(
                palette,
                "From your library",
                found.len(),
            ));
        }
        for (index, track) in found {
            let found = self.suggestions[index].clone();
            menu = menu.child(
                self.suggestion_row(
                    palette,
                    ("cached-search-track", index),
                    tile(track.artwork_url.as_deref(), CadenceIcon::MusicNote, false),
                    highlighted(&track.title, &query, palette),
                    track.artist.clone(),
                    "In library",
                    None,
                    self.suggestion_selection == Some(index),
                )
                .test_support()
                .on_click(cx.listener(move |this, _, _, cx| this.play_cached(Some(&found), cx))),
            );
        }

        if !self.live.is_empty() {
            let mut group = div().flex().flex_col().child(Self::suggestion_heading(
                palette,
                "On Spotify",
                self.live.len(),
            ));
            for (index, found) in self.live.iter().enumerate() {
                let parts = found.parts();
                let selected = self.suggestion_selection == Some(self.suggestions.len() + index);
                let like = match found {
                    LiveResult::Track(track) => Some(self.like_button(palette, index, track, cx)),
                    _ => None,
                };
                group = group.child(
                    self.suggestion_row(
                        palette,
                        ("live-search", index),
                        tile(parts.artwork_url.as_deref(), parts.icon, parts.round),
                        highlighted(&parts.title, &query, palette),
                        parts.detail,
                        parts.tag,
                        like,
                        selected,
                    )
                    .test_support()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_live(index, cx))),
                );
            }
            // Spotify's answer settles in; the library rows were already there.
            menu = menu.child(if motion {
                group
                    .relative()
                    .with_animation(
                        ("live-in", self.live_serial),
                        Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                        |group, delta| group.opacity(delta).top(px(4. * (1. - delta))),
                    )
                    .into_any_element()
            } else {
                group.into_any_element()
            });
        } else if waiting {
            menu = menu
                .child(Self::suggestion_heading(palette, "On Spotify", 0))
                .child(Self::suggestion_placeholders(
                    palette,
                    if self.suggestions.is_empty() { 3 } else { 2 },
                ));
        }

        if self.suggestions.is_empty() && self.live.is_empty() && !waiting {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(14.))
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(if wants_live {
                        format!("Nothing for “{query}” in your library or on Spotify yet.")
                    } else {
                        format!("Nothing for “{query}” in your library.")
                    }),
            );
        }

        let selected = self.suggestion_selection == Some(self.suggestions.len() + self.live.len());
        menu = menu
            .when(
                !self.suggestions.is_empty() || !self.live.is_empty() || waiting,
                |menu| {
                    menu.child(
                        div()
                            .h(px(1.))
                            .mx(px(6.))
                            .my(px(6.))
                            .bg(rgb(palette.border)),
                    )
                },
            )
            .child(
                components::button(palette, "cached-search-submit")
                    .test_support()
                    .w_full()
                    .h(px(46.))
                    .px(px(16.))
                    .gap(px(10.))
                    .rounded(px(9.))
                    .justify_start()
                    .when(selected, |row| row.bg(rgb(palette.control_hover)))
                    .hover(|style| style.bg(rgb(palette.surface_hover)))
                    .child(components::icon(
                        CadenceIcon::Search,
                        tokens::CONTROL_ICON,
                        palette.text_primary,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(format!("Search Spotify for “{query}”")),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py_0p5()
                            .rounded(px(6.))
                            .bg(gpui_kit::rgba(0xFFFFFF14))
                            .text_color(rgb(palette.text))
                            .text_size(tokens::CAPTION_TEXT)
                            .child("↵"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.submit_search_from_field(cx))),
            );
        let menu = menu.on_mouse_down_out(cx.listener(
            |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                if !this
                    .search_bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&event.position))
                {
                    this.dismiss_suggestions(cx);
                }
            },
        ));
        // The panel fades in and settles down 4px from the field. Closing is
        // instant: the field is where the listener is already looking.
        if motion {
            menu.with_animation(
                "search-suggestions-open",
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                |menu, delta| menu.opacity(delta).top(px(48. + 4. * (1. - delta))),
            )
            .into_any_element()
        } else {
            menu.into_any_element()
        }
    }

    fn search_field(
        &mut self,
        palette: CadencePalette,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = if self.route == Route::Artist {
            Rems(if compact { 13. } else { 23. })
        } else if compact {
            tokens::COMPACT_SEARCH_FIELD_WIDTH
        } else {
            tokens::SEARCH_FIELD_WIDTH
        };
        components::glass_search_frame(palette, &self.search_input, window, cx)
            .id("search-field")
            .test_support()
            .relative()
            .capture_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                // The Spotify row is one more entry after the library matches.
                let entries =
                    this.suggestions.len() + this.live.len() + usize::from(this.search_row);
                if entries == 0 {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => {
                        this.suggestion_selection =
                            Some(this.suggestion_selection.map_or(0, |i| (i + 1) % entries));
                    }
                    "up" => {
                        this.suggestion_selection = Some(
                            this.suggestion_selection
                                .map_or(entries - 1, |i| (i + entries - 1) % entries),
                        );
                    }
                    "escape" => this.dismiss_suggestions(cx),
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .w(width)
            .min_w_0()
            .h_10()
            .justify_start()
            .gap_2p5()
            .text_sm()
            .text_color(rgb(palette.text))
            .child(components::icon(
                CadenceIcon::Search,
                tokens::FIELD_ICON,
                palette.text_primary,
            ))
            .child(
                Input::new(&self.search_input)
                    .id("search-input")
                    .appearance(false)
                    .text_color(rgb(palette.text_primary))
                    .bordered(false)
                    .focus_bordered(false)
                    .flex_1()
                    .min_w_0()
                    .h_full(),
            )
            .child(
                div()
                    .when(self.route == Route::Artist && compact, |hint| hint.hidden())
                    .px_1p5()
                    .py_0p5()
                    .rounded_md()
                    .bg(rgb(palette.surface_raised))
                    .bg(gpui_kit::rgba(0xFFFFFF14))
                    .text_color(rgb(palette.text))
                    .text_size(tokens::CAPTION_TEXT)
                    .child(if cfg!(target_os = "macos") {
                        "⌘ K"
                    } else {
                        "Ctrl K"
                    }),
            )
            .child(
                gpui_kit::canvas(
                    {
                        let bounds = self.search_bounds.clone();
                        move |rect, _, _| bounds.set(Some(rect))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .when(self.suggestions_open(), |field| {
                field.child(deferred(self.suggestion_menu(palette, width, cx)))
            })
    }

    /// The line under the account name: the newest failure if there is one,
    /// otherwise how far the Spotify connection has got.
    fn account_detail(&self, cx: &App) -> SharedString {
        if let Some(error) = self.player.read(cx).error() {
            return error.clone();
        }
        if let Some(error) = &self.error {
            return error.clone();
        }
        match self.session.read(cx).state() {
            ConnectionState::Starting => "Starting Spotify…".into(),
            ConnectionState::Failed => "Backend unavailable".into(),
            ConnectionState::SetupRequired => "Developer app required".into(),
            ConnectionState::AuthorizationRequired => "Not connected".into(),
            ConnectionState::Connecting => "Connecting…".into(),
            ConnectionState::PremiumRequired => "Premium required".into(),
            ConnectionState::Ready => "Spotify connected".into(),
        }
    }

    fn profile_name(&self, cx: &App) -> SharedString {
        self.session.read(cx).profile().map_or_else(
            || SharedString::new_static("Spotify account"),
            |profile| profile.display_name.clone(),
        )
    }

    fn account_menu(
        &self,
        palette: CadencePalette,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let profile_name = self.profile_name(cx);
        let can_connect = matches!(
            self.session.read(cx).state(),
            ConnectionState::AuthorizationRequired
        );

        components::solid_menu_surface(palette)
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.menu_open {
                        this.menu_dismissed_at = Some(std::time::Instant::now());
                    }
                    this.close_menu(cx);
                }),
            )
            .absolute()
            // Fades in and settles down from 4px above, so it reads as coming
            // from the avatar. Fully open it sits where it always did.
            .top(Rems(3. - 0.25 * (1. - progress)))
            .right_0()
            .opacity(progress)
            .child(
                div()
                    .px_2p5()
                    .py_2()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(profile_name),
                    )
                    .child(
                        div()
                            .mt_0p5()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(self.account_detail(cx)),
                    ),
            )
            .when(can_connect, |menu| {
                menu.child(
                    components::menu_item(
                        palette,
                        "account-connect",
                        CadenceIcon::Key,
                        "Log in with Spotify",
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(ToolbarEvent::Connect);
                        cx.notify();
                    })),
                )
            })
            .child(
                div()
                    .mt_1()
                    .pt_1p5()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(
                        components::menu_item(
                            palette,
                            "account-profile",
                            CadenceIcon::Person,
                            "Profile",
                            false,
                        )
                        .test_support()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu_open = false;
                            cx.emit(ToolbarEvent::Navigate(Route::Profile));
                            cx.notify();
                        })),
                    )
                    .child(
                        components::menu_item(
                            palette,
                            "account-settings",
                            CadenceIcon::Settings,
                            "Settings",
                            false,
                        )
                        .test_support()
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(ToolbarEvent::OpenSettings);
                        })),
                    ),
            )
            .when(self.session.read(cx).is_ready(), |menu| {
                menu.child(
                    components::menu_item(
                        palette,
                        "account-logout",
                        CadenceIcon::SignOut,
                        "Logout",
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(ToolbarEvent::Logout);
                        cx.notify();
                    })),
                )
            })
    }
}

impl Render for Toolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let compact = is_compact_content_layout(window);
        let menu_progress = self
            .menu_motion
            .frame(
                if self.menu_open { 1. } else { 0. },
                super::motion::popover(self.menu_open),
                window,
                cx,
            )
            .value() as f32;
        let menu_progress = menu_progress.clamp(0., 1.);
        let profile_name = self.profile_name(cx);
        let profile = self.session.read(cx).profile().cloned();
        let profile_artwork = profile
            .as_ref()
            .and_then(|profile| profile.artwork_url.as_deref());
        let page_title = match self.route {
            Route::Settings => Some("Settings"),
            Route::WhatsNew => Some("What’s new"),
            _ => None,
        };
        let artist_dock_width = Rems(if compact { 7.5 } else { 10.5 });

        div()
            .h(tokens::TOOLBAR_HEIGHT)
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_7()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .when(self.route == Route::Artist, |group| {
                        group.flex_1().min_w_0()
                    })
                    .when_some(self.back_target, |group, origin| {
                        group.child(
                            components::icon_button(
                                palette,
                                "detail-back",
                                CadenceIcon::ChevronLeft,
                            )
                            .test_support()
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(ToolbarEvent::Navigate(origin));
                                },
                            )),
                        )
                    })
                    .when_some(page_title, |group, title| {
                        group.child(
                            div()
                                .h_10()
                                .flex()
                                .items_center()
                                .text_lg()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(title),
                        )
                    })
                    .when(page_title.is_none(), |group| {
                        group.child(self.search_field(palette, compact, window, cx))
                    }),
            )
            .child(
                div()
                    .relative()
                    .when(self.route == Route::Artist, |anchor| {
                        anchor.ml(artist_dock_width + Rems(3.25)).flex_none()
                    })
                    .when(self.route == Route::Artist, |anchor| {
                        anchor.child(
                            div()
                                .absolute()
                                .right(Rems(3.25))
                                .top(Rems(0.25))
                                .w(artist_dock_width)
                                .h(Rems(2.))
                                .child(
                                    gpui_kit::canvas(
                                        {
                                            let header = self.artist_header.clone();
                                            move |bounds, _, cx| {
                                                if let Some(header) = &header
                                                    && header.read(cx).target_bounds != Some(bounds)
                                                {
                                                    let header = header.clone();
                                                    cx.defer(move |cx| {
                                                        header.update(cx, |header, cx| {
                                                            if header.target_bounds != Some(bounds)
                                                            {
                                                                header.target_bounds = Some(bounds);
                                                                cx.notify();
                                                            }
                                                        });
                                                    });
                                                }
                                            }
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .size_full(),
                                ),
                        )
                    })
                    .child(
                        components::filled_button(palette, "account")
                            .hover(|style| style.opacity(0.9))
                            .test_support()
                            .size_10()
                            .rounded_full()
                            .text_color(rgb(palette.on_accent))
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            // The page you are on is marked, like a selected row.
                            .when(self.route == Route::Profile, |avatar| {
                                avatar.shadow(vec![
                                    gpui_kit::BoxShadow::new(
                                        px(0.),
                                        px(0.),
                                        rgb(palette.text_primary).into(),
                                    )
                                    .spread_radius(px(2.)),
                                ])
                            })
                            .child(components::profile_avatar(
                                profile_artwork,
                                components::initials(&profile_name),
                                window,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // A press on the avatar while the menu is open
                                // already closed it as an outside press.
                                let just_dismissed = this
                                    .menu_dismissed_at
                                    .take()
                                    .is_some_and(|at| at.elapsed() < Duration::from_millis(300));
                                this.menu_open = !this.menu_open && !just_dismissed;
                                if this.menu_open {
                                    cx.emit(ToolbarEvent::MenuOpened);
                                }
                                cx.notify();
                            })),
                    )
                    .when(self.menu_open || menu_progress > 0.001, |anchor| {
                        anchor.child(deferred(self.account_menu(palette, menu_progress, cx)))
                    }),
            )
    }
}

/// Asks before throwing away the saved Spotify developer app, which cannot be
/// undone from inside Sonder.
pub(super) fn spotify_app_change_confirmation(
    palette: CadencePalette,
    signed_in: bool,
    cancel: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    confirm: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let consequence = if signed_in {
        "This signs you out, removes the saved Client ID, and restarts Spotify setup. Your Sonder favorites and settings stay."
    } else {
        "This removes the saved Client ID and restarts Spotify setup. Your Sonder favorites and settings stay."
    };
    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .occlude()
        .bg(palette.scrim)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(tokens::DIALOG_WIDTH)
                .p_6()
                .rounded_2xl()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .shadow_lg()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.text_primary))
                        .child("Change Spotify developer app?"),
                )
                .child(
                    div()
                        .mt_2p5()
                        .text_sm()
                        .line_height(relative(1.5))
                        .text_color(rgb(palette.text))
                        .child(consequence),
                )
                .child(
                    div()
                        .mt_6()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            components::settings_button(
                                palette,
                                "cancel-spotify-app-change",
                                "Cancel",
                            )
                            .test_support()
                            .on_click(cancel),
                        )
                        .child(
                            components::filled_button(palette, "confirm-spotify-app-change")
                                .h_10()
                                .px_3p5()
                                .rounded(tokens::CONTROL_RADIUS)
                                .bg(rgb(palette.destructive))
                                .text_size(tokens::BODY_TEXT)
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.on_destructive))
                                .hover(|style| style.opacity(0.88))
                                .child("Change developer app")
                                .on_click(confirm),
                        ),
                ),
        )
}
