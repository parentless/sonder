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

    fn empty_message(self) -> &'static str {
        match self {
            Self::LikedSongs => "No liked songs",
            Self::Recent => "No listening history yet",
            Self::LocalFiles => {
                "No music found in these folders. Sonder plays MP3, FLAC, Ogg, M4A and WAV files."
            }
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

/// A saved collection of tracks, listed straight from the library.
pub(super) struct LibraryTracksPage {
    section: LibrarySection,
    library: Entity<library::Library>,
    tracks: Entity<track_list::TrackList>,
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
        }
    }

    /// Takes down any open row menu, for a route change no click drove.
    pub(super) fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.tracks.update(cx, |list, cx| list.close_menu(cx));
    }
}

impl Render for LibraryTracksPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let content = if tracks.is_empty() {
            let message = if no_folders {
                "Add a folder to list the music in it here."
            } else if loaded {
                section.empty_message()
            } else {
                section.loading_message()
            };
            components::empty_state(palette, message).into_any_element()
        } else {
            self.tracks
                .update(cx, |list, cx| list.show(section.list_id(), tracks, cx));
            self.tracks.clone().into_any_element()
        };

        let heading = components::page_heading(palette, section.title(), detail).when(
            section == LibrarySection::LocalFiles,
            |heading| {
                heading.child(
                    components::settings_button(palette, "local-files-folders", "Folders")
                        .test_support()
                        .flex_none()
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PageEvent::OpenLocalFilesSettings)),
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
                components::empty_state(palette, "No Spotify playlists").into_any_element()
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
