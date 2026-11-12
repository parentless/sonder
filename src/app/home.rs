//! The Home page: the listener's most played artists and tracks.
//!
//! Two small Spotify requests, made only when the page is opened and its data
//! is more than half an hour old. Nothing runs in the background.
use super::page::PageEvent;
use super::*;
use gpui_kit::TestSupportExt as _;
use std::time::SystemTime;

/// Top items drift over weeks, so there is no reason to fetch them often.
const HOME_STALE_TIME: Duration = Duration::from_secs(30 * 60);
/// New releases move more slowly, and finding them costs a request per artist.
/// Announced dates change slowly, and each lookup costs MusicBrainz about 20 requests.
const UPCOMING_STALE_TIME: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASES_STALE_TIME: Duration = Duration::from_secs(6 * 60 * 60);
const RELEASE_CARD_WIDTH: Rems = Rems(9.5);

/// One time range's fetched lists, kept so switching back needs no request.
struct RangeData {
    artists: Arc<[model::Artist]>,
    tracks: Arc<[model::Track]>,
    loaded_at: SystemTime,
}

impl RangeData {
    fn is_fresh(&self) -> bool {
        self.loaded_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed < HOME_STALE_TIME)
    }
}

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
    track_list: Entity<track_list::TrackList>,
    range: model::HomeRange,
    data: [Option<RangeData>; 3],
    releases: Option<Releases>,
    releases_loading: bool,
    releases_request: Option<gpui_kit::Task<()>>,
    upcoming: Option<Upcoming>,
    upcoming_loading: bool,
    upcoming_request: Option<gpui_kit::Task<()>>,
    loading: bool,
    error: Option<SharedString>,
    /// The saved sign-in lacks the permission to read top items.
    permission_missing: bool,
    visible: bool,
    request: Option<gpui_kit::Task<()>>,
    scroll: gpui_kit::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for HomePage {}

impl HomePage {
    pub(super) fn new(backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let track_list = cx.new(|cx| track_list::TrackList::new(cx).with_inline_rows());
        let session = services::AppServices::session(cx);
        let subscriptions = vec![
            page::forward(&track_list, cx),
            cx.subscribe(&session, |this, _, event: &session::SessionEvent, cx| {
                this.handle_session_event(event, cx)
            }),
        ];
        Self {
            backend,
            session,
            track_list,
            range: model::HomeRange::default(),
            data: [None, None, None],
            releases: None,
            releases_loading: false,
            releases_request: None,
            upcoming: None,
            upcoming_loading: false,
            upcoming_request: None,
            loading: false,
            error: None,
            permission_missing: false,
            visible: false,
            request: None,
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
                if self.visible {
                    self.load(cx);
                    self.load_releases_if_needed(cx);
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.request = None;
        self.loading = false;
        self.data = [None, None, None];
        self.releases = None;
        self.releases_loading = false;
        self.releases_request = None;
        self.upcoming = None;
        self.upcoming_loading = false;
        self.upcoming_request = None;
        self.error = None;
        self.permission_missing = false;
        self.track_list.update(cx, |list, cx| list.clear(cx));
        cx.notify();
    }

    /// Called by the workspace as the route changes, so the page only fetches
    /// while it is the one on screen.
    pub(super) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if !visible {
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

    fn current(&self) -> Option<&RangeData> {
        self.data[self.range.index()].as_ref()
    }

    fn load_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.current().is_some_and(RangeData::is_fresh);
        if !fresh && !self.loading && !self.permission_missing {
            self.load(cx);
        }
    }

    fn load_releases_if_needed(&mut self, cx: &mut Context<Self>) {
        let fresh = self.releases.as_ref().is_some_and(|releases| {
            releases
                .loaded_at
                .elapsed()
                .is_ok_and(|elapsed| elapsed < RELEASES_STALE_TIME)
        });
        if fresh || self.releases_loading || self.permission_missing {
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
        if fresh || self.upcoming_loading || self.permission_missing || !self.visible {
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
        if range == self.range {
            return;
        }
        self.range = range;
        self.error = None;
        // A request for the previous range is no longer wanted.
        self.request = None;
        self.loading = false;
        self.load_if_needed(cx);
        cx.notify();
    }

    pub(super) fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.track_list.update(cx, |list, cx| list.close_menu(cx));
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let range = self.range;
        let reply = catalog::request(&self.backend, move |respond| BackendCommand::LoadHome {
            range,
            respond,
        });
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.request = None;
                page.loading = false;
                match result {
                    Ok(mut home) => {
                        home.artists.truncate(model::HOME_ARTIST_LIMIT);
                        home.tracks.truncate(model::HOME_TRACK_LIMIT);
                        page.data[range.index()] = Some(RangeData {
                            artists: home.artists.into(),
                            tracks: home.tracks.into(),
                            loaded_at: SystemTime::now(),
                        });
                        page.error = None;
                        cx.emit(PageEvent::Loaded);
                    }
                    Err(error) => {
                        page.permission_missing =
                            error.as_str() == model::HomePermissionMissing.to_string();
                        if !page.permission_missing {
                            page.error = Some(error.clone());
                            cx.emit(PageEvent::Failed(error));
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn release_card(
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
            .w(RELEASE_CARD_WIDTH + Rems(0.75))
            .min_w_0()
            .p_1p5()
            .flex_col()
            .items_start()
            .justify_start()
            .gap_1p5()
            .rounded_xl()
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(components::artwork(
                palette,
                &cache,
                album.artwork_url.as_deref(),
                tokens::ArtworkSize {
                    size: RELEASE_CARD_WIDTH,
                    radius: Rems(0.625),
                },
                CadenceIcon::MusicNote,
            ))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
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
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(album.name.clone()),
                            ),
                    )
                    .child(components::page_detail(palette, detail).w_full().truncate()),
            )
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

    fn artist_card(
        &self,
        artist: &model::Artist,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = appearance::Appearance::palette(cx);
        let cache = services::AppServices::image_cache(cx);
        components::button(palette, ("home-artist", index))
            .test_support()
            .w(HOME_CARD_WIDTH + Rems(0.75))
            .min_w_0()
            .p_1p5()
            .flex_col()
            .items_start()
            .justify_start()
            .gap_1p5()
            .rounded_xl()
            .hover(|style| style.bg(rgb(palette.surface_hover)))
            .child(components::rectangular_artwork(
                palette,
                &cache,
                artist.artwork_url.as_deref(),
                tokens::SEARCH_ARTIST_ARTWORK,
                HOME_CARD_WIDTH,
                CadenceIcon::Person,
            ))
            .child(
                // Name and rank sit close together, away from the picture.
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
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
                if let Some(artist) = this.current().and_then(|data| data.artists.get(index)) {
                    cx.emit(PageEvent::OpenArtist(model::ArtistRef {
                        name: artist.name.clone(),
                        source_id: Some(artist.source_id.clone()),
                        spotify_uri: artist.spotify_uri.clone(),
                    }));
                }
            }))
            .into_any_element()
    }

    /// Last 4 weeks, 6 months or all time; the chosen one is filled.
    fn range_chips(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let mut chips = div().mt_4().flex().flex_wrap().gap_2();
        for range in model::HomeRange::ALL {
            chips = chips.child(
                components::pill(
                    palette,
                    ("home-range", range.index()),
                    range.label(),
                    range == self.range,
                )
                .test_support()
                .on_click(cx.listener(move |this, _, _, cx| this.select_range(range, cx))),
            );
        }
        chips
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

/// Wide enough for five across at the default window width.
const HOME_CARD_WIDTH: Rems = Rems(9.5);

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
        let (artists, tracks) = self
            .current()
            .map(|data| (data.artists.clone(), data.tracks.clone()))
            .unwrap_or_default();
        let loaded = self.current().is_some();
        let content = if self.permission_missing {
            self.permission_prompt(palette, cx)
        } else if self.error.is_some() && !loaded {
            div().child(components::empty_state(
                palette,
                "Unable to load your top artists and tracks",
            ))
        } else if !loaded {
            div().child(components::list_skeleton_rows(palette, 4))
        } else if artists.is_empty() && tracks.is_empty() {
            div().child(components::empty_state(
                palette,
                "Listen to more music and your top artists and tracks will show up here",
            ))
        } else {
            let mut cards = div().flex().flex_wrap().gap_4();
            for (index, artist) in artists.iter().enumerate() {
                cards = cards.child(self.artist_card(artist, index, cx));
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
            let mut upcoming_cards = div().flex().flex_wrap().gap_3();
            for (index, release) in upcoming.iter().enumerate() {
                upcoming_cards = upcoming_cards.child(upcoming_card(palette, release, index));
            }
            let mut release_cards = div().flex().flex_wrap().gap_4();
            for (index, album) in releases.iter().enumerate() {
                release_cards = release_cards.child(self.release_card(album, index, cx));
            }
            div()
                .flex()
                .flex_col()
                .gap_6()
                .when(!artists.is_empty(), |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(heading("home-artists-heading", "Your top artists"))
                            .child(cards),
                    )
                })
                .when(releases_pending, |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(heading(
                                "home-releases-heading",
                                "New from artists you listen to",
                            ))
                            .child(release_skeleton(palette)),
                    )
                })
                .when(!releases.is_empty(), |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(heading(
                                "home-releases-heading",
                                "New from artists you listen to",
                            ))
                            .child(release_cards),
                    )
                })
                .when(upcoming_shown, |content| {
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
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
                                upcoming_cards.into_any_element()
                            }),
                    )
                })
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
                            .when(!self.permission_missing, |page| {
                                page.child(self.range_chips(palette, cx))
                            })
                            .child(div().mt_5().child(content)),
                    ),
            )
    }
}

/// `2026-11-07` as `Nov 7`; anything not an exact date gives nothing.
/// Placeholder cards the size of a release card, so the section does not
/// change height when the data arrives.
fn release_skeleton(palette: CadencePalette) -> Div {
    div().flex().flex_wrap().gap_4().children((0..5).map(|_| {
        div()
            .w(RELEASE_CARD_WIDTH + Rems(0.75))
            .p_1p5()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .size(RELEASE_CARD_WIDTH)
                    .rounded(Rems(0.625))
                    .bg(rgb(palette.control)),
            )
            .child(
                div()
                    .h_3()
                    .w(relative(0.7))
                    .rounded_md()
                    .bg(rgb(palette.control)),
            )
            .child(
                div()
                    .h_2()
                    .w(relative(0.45))
                    .rounded_md()
                    .bg(rgb(palette.control)),
            )
    }))
}

/// A release that is announced but not out: no artwork or link yet, just when.
fn upcoming_card(
    palette: CadencePalette,
    release: &model::UpcomingRelease,
    index: usize,
) -> impl IntoElement {
    let when = release_label(&release.date).unwrap_or_else(|| release.date.clone());
    div()
        .id(("home-upcoming", index))
        .test_support()
        .w(RELEASE_CARD_WIDTH + Rems(0.75))
        .min_w_0()
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .rounded_xl()
        .bg(rgb(palette.surface_raised))
        .child(
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.link))
                .child(when.to_uppercase()),
        )
        .child(
            div()
                .w_full()
                .truncate()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(release.title.clone()),
        )
        .child(
            components::page_detail(palette, format!("{} · {}", release.artist, release.kind))
                .w_full()
                .truncate(),
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
