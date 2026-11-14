use super::*;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, HeadlessAppContext, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle,
};

#[allow(dead_code)]
pub(crate) mod rendering;

mod interactions;

pub(super) struct BackendProbe {
    pub commands: tokio::sync::mpsc::Receiver<BackendCommand>,
    pub volume: tokio::sync::watch::Receiver<f32>,
}

pub(super) fn initialize(cx: &mut App, theme: ThemePreference) -> BackendProbe {
    gpui_kit::init(cx);
    let (backend, commands, volume) = Backend::isolated();
    services::AppServices::init_isolated(
        cx,
        backend,
        AppPreferences {
            theme,
            ..AppPreferences::default()
        },
    );
    bootstrap::bind_keys(cx);
    BackendProbe { commands, volume }
}

pub(super) fn track(index: usize) -> model::Track {
    model::Track {
        provider: model::Provider::Spotify,
        source_id: format!("track-{index}"),
        spotify_uri: Some(format!("spotify:track:track-{index}")),
        isrc: None,
        title: format!("Song {}", index + 1).into(),
        artist: "Sonder Ensemble".into(),
        artists: Vec::new(),
        album: "An afternoon in September".into(),
        album_ref: None,
        duration_ms: 240_000,
        artwork_url: None,
    }
}

/// The folders the Local Files scenes read.
pub(super) fn local_folders() -> Vec<std::path::PathBuf> {
    vec![
        "/Users/listener/Music".into(),
        "/Volumes/Archive/Lossless".into(),
    ]
}

/// A small invented library as the scanner would list it: tagged albums,
/// one with embedded artwork, and a file named only "Artist - Title".
pub(super) fn local_library() -> Vec<model::Track> {
    let album = |artist: &str, album: &str, art: bool, tracks: &[(&str, u32)]| {
        tracks
            .iter()
            .enumerate()
            .map(|(index, &(title, seconds))| {
                let path = format!(
                    "/Users/listener/Music/{artist}/{album}/{:02} {title}.flac",
                    index + 1
                );
                local_track(artist, album, title, seconds, path, art)
            })
            .collect::<Vec<_>>()
    };
    let mut tracks = album(
        "Ana Lind",
        "Tidewater",
        true,
        &[
            ("Low Light", 214),
            ("Harbour Lines", 187),
            ("Paper Boats", 242),
            ("Salt and Glass", 199),
            ("Slow Return", 305),
        ],
    );
    // The album shares its first song's cover, as the scanner lists it.
    let cover = tracks[0].artwork_url.clone();
    for track in &mut tracks {
        track.artwork_url.clone_from(&cover);
    }
    tracks.extend(album(
        "Ostrava Tapes",
        "Night Bus Demos",
        false,
        &[
            ("Night Bus", 176),
            ("Ninth Floor", 231),
            ("Kitchen Demo", 128),
        ],
    ));
    tracks.push(local_track(
        "Teo Marsh",
        "",
        "Untitled Sketch",
        94,
        "/Volumes/Archive/Lossless/Teo Marsh - Untitled Sketch.wav".into(),
        false,
    ));
    tracks
}

fn local_track(
    artist: &str,
    album: &str,
    title: &str,
    seconds: u32,
    path: String,
    art: bool,
) -> model::Track {
    model::Track {
        provider: model::Provider::Local,
        spotify_uri: Some(crate::local_files::local_uri(
            artist,
            album,
            title,
            seconds * 1000,
        )),
        isrc: None,
        title: title.to_owned().into(),
        artist: artist.to_owned().into(),
        artists: vec![model::ArtistRef {
            name: artist.to_owned().into(),
            source_id: None,
            spotify_uri: None,
        }],
        album: album.to_owned().into(),
        album_ref: None,
        duration_ms: seconds * 1000,
        artwork_url: art.then(|| format!("{}{path}", crate::local_files::ARTWORK_SCHEME)),
        source_id: path,
    }
}

pub(super) fn gallery_results() -> model::CatalogSearchResults {
    model::CatalogSearchResults {
        artists: ["glaive", "Glass Animals", "Glasser"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| model::Artist {
                provider: model::Provider::Spotify,
                source_id: format!("artist-{index}"),
                spotify_uri: None,
                name: name.into(),
                artwork_url: None,
            })
            .collect(),
        albums: [
            "God Save The Three",
            "Y’all",
            "May It Never Falter",
            "i care so much that i dont care at all",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| model::Album {
            provider: model::Provider::Spotify,
            source_id: format!("album-{index}"),
            spotify_uri: None,
            name: name.into(),
            artists: vec![model::ArtistRef {
                name: "glaive".into(),
                source_id: Some("artist-0".into()),
                spotify_uri: None,
            }],
            release_date: Some((2026 - index).to_string()),
            artwork_url: None,
            track_count: None,
        })
        .collect(),
        tracks: (0..10).map(track).collect(),
    }
}

pub(super) fn ready(cx: &mut App) {
    services::AppServices::session(cx).update(cx, |session, cx| {
        session.handle_backend_event(
            BackendEvent::SpotifyConfigured {
                generation: 0,
                client_id: "0123456789abcdef0123456789abcdef".into(),
                source: ClientIdSource::Saved,
            },
            cx,
        );
        session.handle_backend_event(BackendEvent::CatalogReady { generation: 0 }, cx);
    });
    services::AppServices::library(cx).update(cx, |library, cx| {
        library.handle_backend_event(
            BackendEvent::LibraryLoaded {
                generation: 0,
                liked_tracks: vec![track(0), track(1), track(1), track(2)],
                playlists: Vec::new(),
            },
            0,
            cx,
        );
        library.handle_backend_event(
            BackendEvent::LocalStateLoaded {
                favorites: vec![track(0)],
                pinned_playlists: Vec::new(),
                recently_played: Vec::new(),
            },
            0,
            cx,
        );
    });
    services::AppServices::player(cx).update(cx, |player, cx| {
        player.handle_backend_event(
            BackendEvent::PlaybackSnapshotLoaded {
                current: track(0),
                next: vec![track(1), track(2)],
                position_ms: 60_000,
            },
            cx,
        );
    });
}

pub(super) fn workspace(
    cx: &mut HeadlessAppContext,
    width: f32,
    height: f32,
) -> (WindowHandle<Root>, Entity<Workspace>) {
    cx.update(ready);
    let mut workspace = None;
    let window = cx
        .open_window(size(px(width), px(height)), |window, cx| {
            let view = cx.new(|cx| Workspace::new(window, cx));
            workspace = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("workspace window");
    (window, workspace.expect("workspace entity"))
}

pub(super) fn settle(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("draw fixture");
    }
}

/// More Tab stops than any Sonder window has, so a Tab walk that runs past it
/// has missed its target.
const MAX_TAB_STOPS: usize = 64;

/// Presses and releases `key`, then lets the window settle. A literal space is
/// sent as the space key, so text can be typed a character at a time.
pub(super) fn press_key(cx: &mut HeadlessAppContext, window: AnyWindowHandle, key: &str) {
    let key = if key == " " { "space" } else { key };
    cx.update_window(window, |_, window, cx| {
        let keystroke = Keystroke::parse(key).expect("fixture key");
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
    })
    .expect("native keyboard input");
    settle(cx, window);
}

/// Presses Tab until `id` has keyboard focus, as a keyboard user would.
pub(super) fn tab_to(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
) {
    let id = id.into();
    for _ in 0..MAX_TAB_STOPS {
        let focused = cx
            .update_window(window, |_, window, _| {
                window.find(id.clone()).focused() == Some(true)
            })
            .expect("fixture focus");
        if focused {
            return;
        }
        press_key(cx, window, "tab");
    }
    panic!("Tab never reached {id:?}");
}
