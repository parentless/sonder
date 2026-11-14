use super::*;
use gpui_kit::TestSupportExt as _;

use page::PageEvent;

/// One of the track collections the library keeps for the signed-in account.
///
/// The three read different slices of `Library` and word themselves
/// differently, but the page around them is the same, so they share one entity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LibrarySection {
    LikedSongs,
    Recent,
    LocalFiles,
}

impl LibrarySection {
    fn page_id(self) -> &'static str {
        match self {
            Self::LikedSongs => "liked-songs-page",
            Self::Recent => "recent-page",
            Self::LocalFiles => "local-files-page",
        }
    }

    fn list_id(self) -> &'static str {
        match self {
            Self::LikedSongs => "liked-tracks",
            Self::Recent => "recent-tracks",
            Self::LocalFiles => "local-tracks",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::LikedSongs => "Liked Songs",
            Self::Recent => "Recently played",
            Self::LocalFiles => "Local Files",
        }
    }

    /// Icon, title and help for a section with nothing in it.
    fn empty_message(self) -> (CadenceIcon, &'static str, &'static str) {
        match self {
            Self::LikedSongs => (
                CadenceIcon::Heart,
                "No liked songs",
                "Tap the heart on a song to add it here.",
            ),
            Self::Recent => (
                CadenceIcon::Clock,
                "No listening history yet",
                "Songs you play will show up here.",
            ),
            Self::LocalFiles => (
                CadenceIcon::Folder,
                "No music found",
                "Sonder plays MP3, FLAC, Ogg, M4A and WAV files from your folders.",
            ),
        }
    }

    fn loading_message(self) -> &'static str {
        match self {
            Self::LikedSongs => "Loading liked songs…",
            Self::Recent => "Loading listening history…",
            Self::LocalFiles => "Looking for music…",
        }
    }

    fn tracks(self, library: &library::Library) -> Arc<[model::Track]> {
        match self {
            Self::LikedSongs => library.liked_tracks().clone(),
            Self::Recent => library.recently_played().clone(),
            Self::LocalFiles => library.local_tracks().clone(),
        }
    }

    /// Whether an empty collection means "nothing here" rather than "not yet".
    /// Liked songs come from Spotify; the other two are local state.
    fn loaded(self, library: &library::Library) -> bool {
        match self {
            Self::LikedSongs => library.loaded(),
            Self::Recent => library.local_loaded(),
            Self::LocalFiles => library.local_files_loaded(),
        }
    }

    fn detail(self, library: &library::Library, track_count: usize) -> String {
        match self {
            Self::LikedSongs => components::revalidating_detail(
                if library.loaded() {
                    format!("{track_count} tracks loaded from Spotify")
                } else {
                    "Liked on Spotify".to_owned()
                },
                library.reloading(),
            ),
            Self::Recent => "Listening history".to_owned(),
            Self::LocalFiles => local_files_detail(library, track_count),
        }
    }
}

/// "1,204 tracks from 2 folders", plus what the listener should know about
/// the last scan.
fn local_files_detail(library: &library::Library, track_count: usize) -> String {
    let folders = library.local_folders().len();
    let mut detail = format!(
        "{} from {}",
        count(track_count, "track", "tracks"),
        count(folders, "folder", "folders")
    );
    if library.local_truncated() {
        detail.push_str(&format!(
            " · showing the first {}",
            group_digits(crate::local_files::MAX_TRACKS)
        ));
    }
    let unreadable = library.local_unreadable();
    if unreadable > 0 {
        detail.push_str(&format!(
            " · {} couldn’t be read",
            count(unreadable, "file", "files")
        ));
    }
    components::revalidating_detail(detail, library.local_files_scanning())
}

fn count(value: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        group_digits(value),
        if value == 1 { one } else { many }
    )
}

/// 20000 as "20,000".
fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The tracks whose title, artist or album contain every word of `query`,
/// ignoring case. An empty query keeps the same list.
pub(super) fn search_tracks(tracks: &Arc<[model::Track]>, query: &str) -> Arc<[model::Track]> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return tracks.clone();
    }
    tracks
        .iter()
        .filter(|track| {
            let text = format!("{}\n{}\n{}", track.title, track.artist, track.album).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .cloned()
        .collect()
}

/// A page's search field, and the list it last produced.
struct TrackSearch {
    input: Entity<InputState>,
    query: String,
    results: Option<SearchResults>,
    _subscription: Subscription,
}

/// What a search found, with the list and query it came from, so a repaint
/// or a rescan that changed nothing does not search again.
struct SearchResults {
    source: Arc<[model::Track]>,
    query: String,
    tracks: Arc<[model::Track]>,
}

/// A saved collection of tracks, listed straight from the library.
pub(super) struct LibraryTracksPage {
    section: LibrarySection,
    library: Entity<library::Library>,
    tracks: Entity<track_list::TrackList>,
    search: Option<TrackSearch>,
    _tracks_subscription: Subscription,
}

impl EventEmitter<PageEvent> for LibraryTracksPage {}

impl LibraryTracksPage {
    pub(super) fn new(section: LibrarySection, cx: &mut Context<Self>) -> Self {
        let tracks = cx.new(|cx| {
            let list = track_list::TrackList::new(cx);
            if matches!(section, LibrarySection::LikedSongs) {
                list.without_header()
            } else {
                list
            }
        });
        Self {
            section,
            library: services::AppServices::library(cx),
            _tracks_subscription: page::forward(&tracks, cx),
            tracks,
            search: None,
        }
    }

    /// Gives the page a field that narrows its list as the listener types.
    pub(super) fn with_search(
        mut self,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe(&input, |page, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(search) = page.search.as_mut()
            {
                search.query = input.read(cx).value().trim().to_owned();
                cx.notify();
            }
        });
        self.search = Some(TrackSearch {
            input,
            query: String::new(),
            results: None,
            _subscription: subscription,
        });
        self
    }

    /// Empties the search field, returning whether it held anything.
    pub(super) fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(search) = self
            .search
            .as_ref()
            .filter(|search| !search.query.is_empty())
        else {
            return false;
        };
        let input = search.input.clone();
        input.update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(search) = self.search.as_mut() {
            search.query.clear();
            search.results = None;
        }
        cx.notify();
        true
    }

    /// The part of `tracks` the search field lets through.
    fn searched(&mut self, tracks: Arc<[model::Track]>) -> Arc<[model::Track]> {
        let Some(search) = self.search.as_mut() else {
            return tracks;
        };
        if search.query.is_empty() {
            search.results = None;
            return tracks;
        }
        if let Some(results) = &search.results
            && Arc::ptr_eq(&results.source, &tracks)
            && results.query == search.query
        {
            return results.tracks.clone();
        }
        let found = search_tracks(&tracks, &search.query);
        search.results = Some(SearchResults {
            source: tracks,
            query: search.query.clone(),
            tracks: found.clone(),
        });
        found
    }

    fn search_field(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let search = self.search.as_ref()?;
        Some(
            components::glass_search_frame(palette, &search.input, window, cx)
                .id("page-search-field")
                .test_support()
                .flex_none()
                .w(tokens::PAGE_SEARCH_FIELD_WIDTH)
                .h_10()
                .gap_2p5()
                .text_sm()
                .text_color(rgb(palette.text))
                .child(components::icon(
                    CadenceIcon::Search,
                    tokens::FIELD_ICON,
                    palette.text_primary,
                ))
                .child(
                    Input::new(&search.input)
                        .id("page-search-input")
                        .appearance(false)
                        .text_color(rgb(palette.text_primary))
                        .bordered(false)
                        .focus_bordered(false)
                        .flex_1()
                        .min_w_0()
                        .h_full(),
                ),
        )
    }

    /// Takes down any open row menu, for a route change no click drove.
    pub(super) fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.tracks.update(cx, |list, cx| list.close_menu(cx));
    }
}

impl Render for LibraryTracksPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let section = self.section;
        let (tracks, loaded, detail, no_folders) = {
            let library = self.library.read(cx);
            let tracks = section.tracks(library);
            (
                tracks.clone(),
                section.loaded(library),
                section.detail(library, tracks.len()),
                section == LibrarySection::LocalFiles && !library.has_local_files(),
            )
        };
        let listed = self.searched(tracks.clone());
        let content = if !tracks.is_empty() && listed.is_empty() {
            let query = self
                .search
                .as_ref()
                .map_or("", |search| search.query.as_str());
            components::empty_state(
                palette,
                CadenceIcon::Search,
                format!("No songs match “{query}”"),
                Some("Try a different word.".into()),
            )
            .into_any_element()
        } else if tracks.is_empty() {
            if no_folders {
                components::empty_state(
                    palette,
                    CadenceIcon::Folder,
                    "No folders yet",
                    Some("Add a folder to list the music in it here.".into()),
                )
                .into_any_element()
            } else if loaded {
                let (glyph, title, detail) = section.empty_message();
                components::empty_state(palette, glyph, title, Some(detail.into()))
                    .into_any_element()
            } else {
                components::page_detail(palette, section.loading_message()).into_any_element()
            }
        } else {
            self.tracks
                .update(cx, |list, cx| list.show(section.list_id(), listed, cx));
            self.tracks.clone().into_any_element()
        };

        // The field stays while it holds a query, so a search with no matches
        // can still be cleared; it is hidden only when there is nothing to search.
        let searching = self
            .search
            .as_ref()
            .is_some_and(|search| !search.query.is_empty());
        let search = (searching || !tracks.is_empty())
            .then(|| self.search_field(palette, window, cx))
            .flatten();
        let heading = components::page_heading(palette, section.title(), detail).when(
            section == LibrarySection::LocalFiles,
            |heading| {
                heading.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .children(search)
                        .child(
                            components::settings_button(palette, "local-files-folders", "Folders")
                                .test_support()
                                .flex_none()
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(PageEvent::OpenLocalFilesSettings)
                                })),
                        ),
                )
            },
        );
        components::page(section.page_id())
            .pt_3()
            .child(heading)
            .child(content)
    }
}

/// Every playlist the account follows on Spotify.
pub(super) struct PlaylistsPage {
    library: Entity<library::Library>,
    playlists: Entity<track_list::PlaylistList>,
    _playlists_subscription: Subscription,
}

impl EventEmitter<PageEvent> for PlaylistsPage {}

impl PlaylistsPage {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let playlists = cx.new(|cx| track_list::PlaylistList::new(cx));
        Self {
            library: services::AppServices::library(cx),
            _playlists_subscription: page::forward(&playlists, cx),
            playlists,
        }
    }
}

impl Render for PlaylistsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let (playlists, loaded, detail) = {
            let library = self.library.read(cx);
            (
                library.playlists().clone(),
                library.loaded(),
                components::revalidating_detail("Your Spotify playlists", library.reloading()),
            )
        };
        let content = if playlists.is_empty() {
            if loaded {
                components::empty_state(
                    palette,
                    CadenceIcon::Playlist,
                    "No Spotify playlists",
                    Some("Playlists you create or follow in Spotify show up here.".into()),
                )
                .into_any_element()
            } else {
                components::list_skeleton(palette).into_any_element()
            }
        } else {
            self.playlists
                .update(cx, |list, cx| list.show("spotify-playlists", playlists, cx));
            self.playlists.clone().into_any_element()
        };

        components::page("playlists-page")
            .pt_3()
            .child(components::page_heading(palette, "Playlists", detail))
            .child(content)
    }
}
