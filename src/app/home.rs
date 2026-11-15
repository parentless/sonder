//! The Home page: an orbit of the artists you play most, what is new from
//! them, and your top tracks.
//!
//! Two small Spotify requests per time range, made only when the page is opened
//! and its data is more than half an hour old. The range on screen is compared
//! with the next longer one, so one more pair of requests shows what is rising.
//! Nothing runs in the background.
use super::page::PageEvent;
use super::top_listening::{TopEvent, TopListening};
use super::*;
use gpui_kit::TestSupportExt as _;
use std::time::SystemTime;

/// New releases move more slowly, and finding them costs a request per artist.
/// Announced dates change slowly, and each lookup costs MusicBrainz about 20 requests.
const UPCOMING_STALE_TIME: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASES_STALE_TIME: Duration = Duration::from_secs(6 * 60 * 60);
/// How many releases and announcements sit beside the orbit.
const SIDE_RELEASES: usize = 4;
const SIDE_UPCOMING: usize = 3;
/// A rising or falling artist is marked green or red.
const RISING: u32 = 0x2E9E63;

/// Where each artist's bubble sits in the orbit, by rank: centre x, centre y
/// and diameter, in rems. Bigger bubbles are higher ranked; names sit below.
const ORBIT: [(f32, f32, f32); model::HOME_ARTIST_LIMIT] = [
    (5.625, 5.5, 10.0),
    (15.5, 3.6, 7.0),
    (18.5, 13.4, 6.0),
    (24.0, 3.0, 5.25),
    (28.5, 11.5, 4.75),
    (5.5, 18.6, 4.5),
    (12.5, 19.8, 3.75),
    (24.0, 21.0, 3.5),
    (31.0, 20.0, 3.25),
    (32.5, 3.5, 3.0),
];
const ORBIT_WIDTH: Rems = Rems(36.);
const ORBIT_HEIGHT: Rems = Rems(26.);

struct Upcoming {
    releases: Arc<[model::UpcomingRelease]>,
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
    releases: Option<Releases>,
    releases_loading: bool,
    releases_request: Option<gpui_kit::Task<()>>,
    upcoming: Option<Upcoming>,
    upcoming_loading: bool,
    upcoming_request: Option<gpui_kit::Task<()>>,
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
        ];
        Self {
            backend,
            session,
            top,
            track_list,
            range: model::HomeRange::default(),
            range_menu_open: false,
            releases: None,
            releases_loading: false,
            releases_request: None,
            upcoming: None,
            upcoming_loading: false,
            upcoming_request: None,
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

    /// A release beside the orbit: small cover, name, artist and a NEW mark.
    fn release_row(
        &self,
        album: &model::Album,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = appearance::Appearance::palette(cx);
        let cache = services::AppServices::image_cache(cx);
        let is_new = release_watch::is_new(&album.source_id, cx);
        let mut detail = album
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(date) = album.release_date.as_deref().and_then(release_label) {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&date);
        }
        components::button(palette, ("home-release", index))
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
                album.artwork_url.as_deref(),
                tokens::ArtworkSize {
                    size: Rems(3.),
                    radius: Rems(0.5),
                },
                CadenceIcon::MusicNote,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(album.name.clone()),
                    )
                    .child(components::page_detail(palette, detail).w_full().truncate()),
            )
            .when(is_new, |row| {
                row.child(
                    div()
                        .id(("home-release-new", index))
                        .flex_none()
                        .px_1p5()
                        .rounded_md()
                        .bg(rgb(palette.link))
                        .text_color(rgb(palette.on_accent))
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("NEW"),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(album) = this
                    .releases
                    .as_ref()
                    .and_then(|releases| releases.albums.get(index))
                {
                    cx.emit(PageEvent::OpenAlbum(model::AlbumRef {
                        name: album.name.clone(),
                        source_id: Some(album.source_id.clone()),
                        spotify_uri: album.spotify_uri.clone(),
                        artwork_url: album.artwork_url.clone(),
                    }));
                }
            }))
            .into_any_element()
    }

    /// One artist as a bubble: bigger for a higher rank, with the way their
    /// rank moved against the longer chart.
    fn orbit_bubble(
        &self,
        artist: &model::Artist,
        index: usize,
        change: Option<i32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = appearance::Appearance::palette(cx);
        let cache = services::AppServices::image_cache(cx);
        let (x, y, diameter) = ORBIT[index];
        let width = diameter.max(7.);
        let artwork = components::rectangular_artwork(
            palette,
            &cache,
            artist.artwork_url.as_deref(),
            tokens::ArtworkSize {
                size: Rems(diameter),
                radius: Rems(diameter / 2.),
            },
            Rems(diameter),
            CadenceIcon::Person,
        );
        components::button(palette, ("home-artist", index))
            .test_support()
            .absolute()
            .left(Rems(x - width / 2.))
            .top(Rems(y - diameter / 2.))
            .w(Rems(width))
            .min_w_0()
            .flex_col()
            .items_center()
            .justify_start()
            .gap_1p5()
            .py_0p5()
            .rounded_xl()
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(artwork)
                    .when_some(change, |wrap, change| {
                        let (glyph, color) = if change > 0 {
                            (CadenceIcon::ChevronUp, RISING)
                        } else {
                            (CadenceIcon::ChevronDown, palette.danger)
                        };
                        wrap.child(
                            div()
                                .id(("home-artist-change", index))
                                .test_support()
                                .absolute()
                                .top_0()
                                .right(Rems(-0.25))
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
                    }),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(artist.name.clone()),
                    )
                    .child(components::page_detail(
                        palette,
                        if index == 0 {
                            "#1 · Most played".to_owned()
                        } else {
                            format!("#{}", index + 1)
                        },
                    )),
            )
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

    /// The time range as one button that opens a short menu.
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
                            .left_0()
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
        let heading = |id, text| {
            div()
                .id(id)
                .test_support()
                .text_xl()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_primary))
                .child(text)
        };
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
            let mut orbit = div().relative().flex_none().w(ORBIT_WIDTH).h(ORBIT_HEIGHT);
            for (index, artist) in artists.iter().take(ORBIT.len()).enumerate() {
                let change = compare.as_ref().and_then(|longer| {
                    let before = longer
                        .iter()
                        .position(|other| other.source_id == artist.source_id)?;
                    let change = before as i32 - index as i32;
                    (change != 0).then_some(change)
                });
                orbit = orbit.child(self.orbit_bubble(artist, index, change, cx));
            }
            self.track_list.update(cx, |list, cx| {
                list.show(("home-tracks", self.range.index()), tracks.clone(), cx);
            });
            let releases: Arc<[model::Album]> = self
                .releases
                .as_ref()
                .map(|releases| releases.albums.clone())
                .unwrap_or_default();
            let upcoming = self
                .upcoming
                .as_ref()
                .map_or_else(|| Arc::from([]), |upcoming| upcoming.releases.clone());
            let releases_pending = self.releases.is_none() && self.releases_loading;
            let upcoming_pending = self.upcoming.is_none() && self.upcoming_loading;
            // Shown while looking, and once looked even when nothing turned up,
            // so an empty row is explained rather than missing.
            let upcoming_shown = upcoming_pending || self.upcoming.is_some();
            let side = div()
                .flex_1()
                .min_w(Rems(20.))
                .flex()
                .flex_col()
                .gap_6()
                .when(releases_pending, |side| {
                    side.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading("home-releases-heading", "New from your orbit"))
                            .child(release_skeleton(palette)),
                    )
                })
                .when(!releases.is_empty(), |side| {
                    side.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading("home-releases-heading", "New from your orbit"))
                            .children(
                                releases
                                    .iter()
                                    .take(SIDE_RELEASES)
                                    .enumerate()
                                    .map(|(index, album)| self.release_row(album, index, cx)),
                            ),
                    )
                })
                .when(upcoming_shown, |side| {
                    side.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading("home-upcoming-heading", "Coming soon"))
                            .child(if upcoming_pending {
                                release_skeleton(palette).into_any_element()
                            } else if upcoming.is_empty() {
                                components::page_detail(
                                    palette,
                                    "Nothing announced yet for your top artists. Dates come from MusicBrainz, so some may be missing.",
                                )
                                .into_any_element()
                            } else {
                                div()
                                    .flex()
                                    .flex_col()
                                    .children(
                                        upcoming
                                            .iter()
                                            .take(SIDE_UPCOMING)
                                            .enumerate()
                                            .map(|(index, release)| {
                                                upcoming_row(palette, release, index)
                                            }),
                                    )
                                    .into_any_element()
                            }),
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
                        .when(!artists.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(heading("home-artists-heading", "Your top artists"))
                                    .child(orbit),
                            )
                        })
                        .child(side),
                )
                .when(!tracks.is_empty(), |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(heading("home-tracks-heading", "Your top tracks"))
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
                                "Home",
                                "Based on what you play most",
                            ))
                            .when(!permission_missing, |page| {
                                page.child(self.range_menu(palette, cx))
                            })
                            .child(div().mt_5().child(content)),
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

/// A release that is announced but not out: no artwork or link yet, just when.
fn upcoming_row(
    palette: CadencePalette,
    release: &model::UpcomingRelease,
    index: usize,
) -> impl IntoElement {
    let when = release_label(&release.date).unwrap_or_else(|| release.date.clone());
    div()
        .id(("home-upcoming", index))
        .test_support()
        .w_full()
        .min_w_0()
        .p_1p5()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .size(Rems(3.))
                .rounded(Rems(0.5))
                .bg(rgb(palette.control))
                .flex()
                .items_center()
                .justify_center()
                .child(components::icon(
                    CadenceIcon::MusicNote,
                    Rems(1.),
                    palette.text_muted,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(release.title.clone()),
                )
                .child(
                    components::page_detail(
                        palette,
                        format!("{} · {}", release.artist, release.kind),
                    )
                    .w_full()
                    .truncate(),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.link))
                .child(when.to_uppercase()),
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
    use super::release_label;

    #[test]
    fn release_dates_read_as_month_and_day() {
        assert_eq!(release_label("2026-11-07").as_deref(), Some("Nov 7"));
        assert_eq!(release_label("2026-01-31").as_deref(), Some("Jan 31"));
        assert_eq!(release_label("2026-11"), None);
        assert_eq!(release_label("2026"), None);
        assert_eq!(release_label("2026-13-01"), None);
    }
}
