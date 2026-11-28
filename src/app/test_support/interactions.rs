use super::{BackendProbe, initialize, press_key, settle, tab_to, track, workspace};
use crate::app::mini_player::{MiniPlayer, Shape};
use crate::app::{
    COLLAPSED_SIDEBAR_WIDTH, EXPANDED_SIDEBAR_WIDTH, Route, Workspace, appearance, assets,
    components, onboarding, services, windows,
};
use crate::backend::{BackendCommand, BackendEvent};
use crate::model;
use crate::storage::{LyricsView, ThemePreference};
use gpui_kit::component::Root;
use gpui_kit::test::{ElementSnapshot, TestWindowExt};
use gpui_kit::{
    AnyView, App, AppContext, HeadlessAppContext, InputEvent as _, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NoopTextSystem, Pixels, Point, Size, WeakEntity, Window,
    WindowHandle, point, px, size,
};
use std::sync::Arc;
use std::time::Duration;

struct Fixture {
    cx: HeadlessAppContext,
    window: WindowHandle<Root>,
    workspace: Option<WeakEntity<Workspace>>,
    backend: BackendProbe,
}

impl Fixture {
    fn at_width(width: f32) -> Self {
        // Set initial bounds before child views measure responsive columns.
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem),
            Arc::new(assets::CadenceAssets),
        );
        let backend = cx.update(|cx| initialize(cx, ThemePreference::Dark));
        let (window, workspace) = workspace(&mut cx, width, 820.);
        settle(&mut cx, window.into());
        Self {
            cx,
            window,
            workspace: Some(workspace.downgrade()),
            backend,
        }
    }

    fn new(settings: bool) -> Self {
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem),
            Arc::new(assets::CadenceAssets),
        );
        let backend = cx.update(|cx| initialize(cx, ThemePreference::Dark));
        let (window, workspace) = workspace(&mut cx, 1280., 820.);
        if settings {
            cx.update(|cx| workspace.update(cx, |workspace, cx| workspace.open_settings(cx)));
        }
        settle(&mut cx, window.into());
        Self {
            cx,
            window,
            workspace: Some(workspace.downgrade()),
            backend,
        }
    }

    fn setup() -> Self {
        Self::setup_required(
            ThemePreference::Light,
            size(px(1280.), px(820.)),
            |window, cx| {
                appearance::Appearance::attach(window, cx);
                cx.new(|cx| onboarding::Onboarding::new(window, cx)).into()
            },
        )
    }

    /// The sign-in window as the app opens it, over a setup that still needs
    /// a Client ID.
    fn sign_in_window() -> Self {
        Self::setup_required(
            ThemePreference::Light,
            size(px(1140.), px(720.)),
            |window, cx| {
                cx.new(|cx| windows::OnboardingWindow::new(window, cx))
                    .into()
            },
        )
    }

    /// A window whose root is `root_view`, over a setup that still needs a
    /// Client ID.
    fn setup_required(
        theme: ThemePreference,
        window_size: Size<Pixels>,
        root_view: impl FnOnce(&mut Window, &mut App) -> AnyView,
    ) -> Self {
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem),
            Arc::new(assets::CadenceAssets),
        );
        let backend = cx.update(|cx| {
            let backend = initialize(cx, theme);
            services::AppServices::session(cx).update(cx, |session, cx| {
                session.handle_backend_event(BackendEvent::SetupRequired, cx);
            });
            backend
        });
        let window = cx
            .open_window(window_size, |window, cx| {
                let view = root_view(window, cx);
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("setup window");
        settle(&mut cx, window.into());
        Self {
            cx,
            window,
            workspace: None,
            backend,
        }
    }

    fn app_change_confirmation_open(&mut self) -> bool {
        self.update(|_, cx| {
            services::AppServices::session(cx)
                .read(cx)
                .app_change_confirmation_open()
        })
    }

    fn open_soloist_setup(&mut self) {
        let workspace = self.workspace.clone().expect("workspace fixture");
        self.update(|_, cx| {
            workspace
                .update(cx, |workspace, cx| workspace.open_soloist_setup(cx))
                .expect("live workspace");
        });
    }

    fn route(&mut self) -> Route {
        let workspace = self.workspace.clone().expect("workspace fixture");
        self.update(|_, cx| {
            workspace
                .upgrade()
                .expect("live workspace")
                .read(cx)
                .router
                .route()
        })
    }

    fn play(&mut self, current: model::Track) {
        self.player_event(BackendEvent::PlaybackSnapshotLoaded {
            current,
            next: vec![track(1)],
            position_ms: 0,
        });
    }

    fn player_event(&mut self, event: BackendEvent) {
        self.update(|_, cx| {
            services::AppServices::player(cx).update(cx, |player, cx| {
                player.handle_backend_event(event, cx);
            });
        });
    }

    fn queued_source_ids(&mut self) -> Vec<String> {
        self.update(|_, cx| {
            services::AppServices::player(cx)
                .read(cx)
                .queue()
                .iter()
                .map(|track| track.source_id.clone())
                .collect()
        })
    }

    fn marked_played(&mut self) -> String {
        match self.backend.commands.try_recv().expect("history request") {
            BackendCommand::MarkPlayed(track) => track.source_id,
            other => panic!("expected a history request, got {other:?}"),
        }
    }

    fn requested_artist(&mut self) -> String {
        match self.backend.commands.try_recv().expect("artist request") {
            BackendCommand::LoadArtist { source_id, .. } => source_id,
            other => panic!("expected an artist request, got {other:?}"),
        }
    }

    fn requested_album(&mut self) -> String {
        match self.backend.commands.try_recv().expect("album request") {
            BackendCommand::LoadAlbum { source_id, .. } => source_id,
            other => panic!("expected an album request, got {other:?}"),
        }
    }

    fn update<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        let result = self
            .cx
            .update_window(self.window.into(), |_, window, cx| f(window, cx))
            .expect("fixture window");
        settle(&mut self.cx, self.window.into());
        result
    }

    fn wait_for(&mut self, predicate: impl Fn(&mut Window, &mut App) -> bool) {
        for _ in 0..20 {
            if self.update(&predicate) {
                return;
            }
            self.cx.advance_clock(Duration::from_millis(10));
        }
        panic!("UI did not settle within 200 ms");
    }

    /// Scrolls Settings to its last section, where the lyrics switch is.
    fn scroll_settings_to_the_end(&mut self) {
        let workspace = self.workspace.clone().expect("workspace fixture");
        self.update(|_, cx| {
            let workspace = workspace.upgrade().expect("live workspace");
            let settings = workspace.read(cx).settings.clone();
            settings.update(cx, |settings, cx| settings.show_local_files(cx));
        });
        self.cx.run_until_parked();
    }

    fn no_commands(&mut self) {
        assert!(matches!(
            self.backend.commands.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    /// The pointer half of a scrub, split so a test can act between the press
    /// and the release. `TestWindowExt::drag` only does the whole gesture.
    fn mouse_down(&mut self, position: Point<gpui_kit::Pixels>) {
        self.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
        });
    }

    fn right_click_track(&mut self, index: usize) {
        self.right_click_track_at(index, 150.);
    }

    fn right_click_track_at(&mut self, index: usize, offset: f32) {
        let position = self.update(|window, _| {
            let bounds = window.find(("spotify-track", index)).bounds();
            point(bounds.origin.x + px(offset), bounds.center().y)
        });
        self.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
        });
        self.update(|window, cx| {
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
        });
    }

    fn mouse_move_to(&mut self, position: Point<gpui_kit::Pixels>) {
        self.update(|window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
        });
    }

    fn mouse_up(&mut self, position: Point<gpui_kit::Pixels>) {
        self.update(|window, cx| {
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
        });
    }

    fn press(&mut self, key: &str) {
        press_key(&mut self.cx, self.window.into(), key);
    }

    fn focused(&mut self, id: &'static str) -> bool {
        self.update(|window, _| window.find(id).focused() == Some(true))
    }

    fn tab_to(&mut self, id: impl Into<gpui_kit::ElementId>) {
        tab_to(&mut self.cx, self.window.into(), id);
    }
}

fn cached_search_fixture() -> Fixture {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: (0..30).map(track).collect(),
                    playlists: vec![],
                },
                0,
                cx,
            );
        });
        window.click("search-input", cx);
    });
    fixture
}

#[test]
fn cached_search_debounces_without_network_and_supports_keyboard_playback() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("Song", cx));
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_none()));
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.no_commands();
    fixture.update(|window, _| {
        for i in 0usize..6 {
            assert!(window.try_find(("cached-search-track", i)).is_some());
        }
        assert!(window.try_find(("cached-search-track", 6usize)).is_none());
    });
    fixture.update(|window, cx| {
        window.press("down", cx);
        window.press("enter", cx);
    });
    let BackendCommand::PlayContext { tracks, index } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected cached track playback")
    };
    assert_eq!(tracks.len(), 1);
    assert_eq!(index, 0);
    assert_eq!(tracks[0].source_id, track(0).source_id);
    fixture.update(|window, cx| {
        assert!(window.try_find("cached-search-suggestions").is_none());
        assert!(services::AppServices::player(cx).read(cx).loading());
    });
    fixture.no_commands();
}

#[test]
fn the_dropdown_asks_spotify_after_a_pause_and_opens_a_result() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("Song", cx));
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.no_commands();
    fixture.cx.advance_clock(Duration::from_millis(500));
    fixture.cx.run_until_parked();
    let BackendCommand::SearchCatalog { query, respond } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected one live search")
    };
    assert_eq!(query, "Song");
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "song-exploder".into(),
                spotify_uri: None,
                name: "Song Exploder".into(),
                artwork_url: None,
            }],
            tracks: vec![],
            albums: vec![],
        }))
        .unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, cx| {
        assert!(window.try_find(("live-search", 0usize)).is_some());
        window.click(("live-search", 0usize), cx);
    });
    assert_eq!(fixture.requested_artist(), "song-exploder");
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_none()));
    fixture.no_commands();
}

#[test]
fn only_spotify_songs_in_the_dropdown_get_a_heart_that_likes_without_playing() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("Song", cx));
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.no_commands();
    fixture.cx.advance_clock(Duration::from_millis(500));
    fixture.cx.run_until_parked();
    let BackendCommand::SearchCatalog { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected one live search")
    };
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![],
            albums: vec![],
            tracks: vec![track(100)],
        }))
        .unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, _| {
        // Library songs keep their "In library" label; the Spotify song gets a heart.
        assert!(window.try_find(("cached-search-track", 0usize)).is_some());
        assert!(window.try_find(("live-search", 0usize)).is_some());
        assert!(window.try_find(("live-like", 0usize)).is_some());
        assert!(window.try_find(("live-like", 1usize)).is_none());
    });
    // The heart likes the song. The row behind it must not start playback.
    fixture.update(|window, cx| window.click(("live-like", 0usize), cx));
    let BackendCommand::SetSpotifyLiked {
        track: sent, liked, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a Spotify like")
    };
    assert_eq!(sent, track(100));
    assert!(liked);
    fixture.no_commands();
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_some()));
}

#[test]
fn the_dropdown_does_not_ask_spotify_for_a_short_query() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("So", cx));
    fixture.cx.advance_clock(Duration::from_millis(1500));
    fixture.cx.run_until_parked();
    fixture.no_commands();
}

#[test]
fn cached_suggestions_click_and_escape_dismissal_work() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("Song", cx));
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.update(|window, cx| window.click(("cached-search-track", 1usize), cx));
    let BackendCommand::PlayContext { tracks, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected cached playback")
    };
    assert_eq!(tracks[0].source_id, track(1).source_id);
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input(" 2", cx);
    });
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.update(|window, cx| window.press("escape", cx));
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_none()));
    fixture.no_commands();
}

#[test]
fn cached_suggestions_reject_replaced_library_rows_and_close_outside() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| window.input("Song", cx));
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.update(|window, cx| {
        let position = window
            .find(("cached-search-track", 0usize))
            .bounds()
            .center();
        window.dispatch_event(
            MouseDownEvent {
                position,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.clear(cx);
            library.handle_backend_event(
                BackendEvent::LocalStateLoaded {
                    favorites: vec![],
                    pinned_playlists: vec![],
                    recently_played: vec![],
                },
                0,
                cx,
            );
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![track(99)],
                    playlists: vec![],
                },
                0,
                cx,
            );
        });
        window.dispatch_event(
            MouseUpEvent {
                position,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    if let Ok(command) = fixture.backend.commands.try_recv() {
        panic!("unexpected stale click command: {command:?}");
    }
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input(" ", cx);
    });
    fixture.cx.advance_clock(Duration::from_millis(130));
    fixture.wait_for(|window, _| window.try_find("cached-search-suggestions").is_some());
    fixture.update(|window, cx| window.click("nav-recent", cx));
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_none()));
}

#[test]
fn cached_suggestions_cancel_pending_work_when_query_is_cleared() {
    let mut fixture = cached_search_fixture();
    fixture.update(|window, cx| {
        window.input("Song", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.press("backspace", cx);
    });
    fixture.cx.advance_clock(Duration::from_millis(300));
    fixture.update(|window, _| assert!(window.try_find("cached-search-suggestions").is_none()));
    fixture.no_commands();
}

#[test]
fn search_shortcut_focuses_input_and_enter_submits_text_without_toggling_playback() {
    let mut fixture = Fixture::new(false);
    fixture.no_commands();
    fixture.update(|window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-k"
            } else {
                "ctrl-k"
            },
            cx,
        )
    });
    fixture.update(|window, cx| {
        assert_eq!(window.find("search-input").focused(), Some(true));
        window.input("Blue", cx);
        window.press("space", cx);
        window.input("Train", cx);
        assert_eq!(window.find("search-input").value(), Some("Blue Train"));
    });
    fixture.no_commands();
    fixture.update(|window, cx| window.press("enter", cx));
    match fixture.backend.commands.try_recv().expect("search command") {
        BackendCommand::SearchCatalog { query, .. } => assert_eq!(query, "Blue Train"),
        command => panic!("unexpected command: {command:?}"),
    }
    fixture.no_commands();
    fixture.update(|_, cx| assert!(!services::AppServices::player(cx).read(cx).playing()));
}

#[test]
fn client_id_validation_keeps_focus_and_only_submits_valid_input() {
    let mut fixture = Fixture::setup();
    fixture.update(|window, cx| {
        window.click("client-id-input", cx);
        window.input("not-a-client-id", cx);
    });
    fixture.update(|window, cx| window.press("enter", cx));
    fixture.no_commands();
    fixture.update(|window, cx| window.click("save-spotify-client-id", cx));
    fixture.update(|window, cx| {
        assert_eq!(window.find("client-id-input").focused(), Some(true));
        assert_eq!(
            window.find("client-id-input").value(),
            Some("not-a-client-id")
        );
        assert!(
            services::AppServices::session(cx)
                .read(cx)
                .setup_error()
                .is_some()
        );
    });
    fixture.no_commands();
    fixture.update(|window, cx| {
        window.click("client-id-input", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.input("0123456789abcdef0123456789abcdef", cx);
    });
    fixture.update(|window, cx| {
        assert!(
            services::AppServices::session(cx)
                .read(cx)
                .setup_error()
                .is_none()
        );
        window.press("enter", cx);
    });
    match fixture
        .backend
        .commands
        .try_recv()
        .expect("configuration command")
    {
        BackendCommand::ConfigureSpotify { client_id, .. } => {
            assert_eq!(client_id, "0123456789abcdef0123456789abcdef");
        }
        command => panic!("unexpected command: {command:?}"),
    }
    fixture.no_commands();
}

#[test]
fn autoplay_switch_updates_preferences_and_rendered_state() {
    let mut fixture = Fixture::new(true);
    fixture.update(|window, cx| {
        assert_eq!(window.find("settings-autoplay").checked(), Some(true));
        window.click("settings-autoplay", cx);
    });
    fixture.update(|window, cx| {
        assert!(!services::AppServices::preferences(cx).autoplay);
        assert_eq!(window.find("settings-autoplay").checked(), Some(false));
        window.click("settings-autoplay", cx);
    });
    fixture.update(|window, cx| {
        assert!(services::AppServices::preferences(cx).autoplay);
        assert_eq!(window.find("settings-autoplay").checked(), Some(true));
    });
    fixture.no_commands();
}

#[test]
fn tab_rings_the_autoplay_switch_and_space_toggles_it() {
    let mut fixture = Fixture::new(true);
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    let autoplay_ring_visible = move |window: &Window, cx: &mut App| {
        let workspace = workspace.upgrade().expect("live workspace");
        let settings = workspace.read(cx).settings.read(cx);
        components::is_focus_visible_within(settings.autoplay_focus(), window, cx)
    };
    fixture.update(|window, cx| assert!(!autoplay_ring_visible(window, cx)));

    fixture.tab_to("settings-autoplay");
    fixture.update(|window, cx| {
        assert!(autoplay_ring_visible(window, cx));
        assert_eq!(window.find("settings-autoplay").checked(), Some(true));
    });

    fixture.press("space");
    fixture.update(|window, cx| {
        assert!(!services::AppServices::preferences(cx).autoplay);
        assert_eq!(window.find("settings-autoplay").checked(), Some(false));
        assert!(autoplay_ring_visible(window, cx));
    });
    fixture.no_commands();
}

#[test]
fn streaming_quality_starts_very_high_and_follows_the_choice() {
    let mut fixture = Fixture::new(true);
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).streaming_quality,
            crate::storage::StreamingQuality::VeryHigh
        );
    });
    fixture.update(|window, cx| window.click(("settings-streaming-quality", 160usize), cx));
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).streaming_quality,
            crate::storage::StreamingQuality::High
        );
    });
    fixture.no_commands();
}

#[test]
fn queue_toggle_and_close_button_control_the_panel_without_backend_commands() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        assert!(window.try_find("close-queue").is_none());
        window.click("queue-toggle", cx);
    });
    fixture.update(|window, cx| {
        assert!(window.find("close-queue").visible());
        window.click("queue-toggle", cx);
    });
    fixture.update(|window, cx| {
        assert!(window.try_find("close-queue").is_none());
        window.click("queue-toggle", cx);
    });
    fixture.update(|window, cx| window.click("close-queue", cx));
    fixture.update(|window, _| assert!(window.try_find("close-queue").is_none()));
    fixture.no_commands();
}

#[test]
fn queue_keyboard_activation_preserves_playback_and_reports_expansion() {
    let mut fixture = Fixture::new(false);
    fixture.press("space");
    assert!(matches!(
        fixture
            .backend
            .commands
            .try_recv()
            .expect("playback shortcut"),
        BackendCommand::Resume
    ));
    let bounds = fixture.update(|window, _| window.find("queue-toggle").bounds());
    fixture.tab_to("queue-toggle");
    fixture.update(|window, _| {
        let button = window.find("queue-toggle");
        assert_eq!(button.role(), Some(gpui_kit::Role::Button));
        assert_eq!(button.label(), Some("Queue"));
        assert_eq!(button.expanded(), Some(false));
        assert_eq!(button.bounds(), bounds);
    });

    for (key, expanded) in [("space", true), ("enter", false), ("space", true)] {
        fixture.press(key);
        fixture.update(|window, cx| {
            assert_eq!(window.find("queue-toggle").expanded(), Some(expanded));
            assert_eq!(window.find("queue-toggle").focused(), Some(true));
            assert_eq!(window.find("queue-toggle").bounds(), bounds);
            assert_eq!(window.try_find("close-queue").is_some(), expanded);
            assert!(services::AppServices::player(cx).read(cx).playing());
        });
        fixture.no_commands();
    }
}

#[test]
fn duplicate_track_actions_preserve_row_index() {
    let mut fixture = Fixture::new(false);
    fixture.right_click_track(2);
    fixture.no_commands();
    fixture.update(|window, cx| {
        assert!(window.try_find(("track-menu-play", 1usize)).is_none());
        window.click(("track-menu-play", 2usize), cx);
    });
    match fixture.backend.commands.try_recv().expect("play command") {
        BackendCommand::PlayContext { tracks, index } => {
            assert_eq!(index, 2);
            assert_eq!(tracks.len(), 4);
            assert_eq!(tracks[1].source_id, tracks[2].source_id);
            assert_eq!(tracks[index].source_id, track(1).source_id);
        }
        command => panic!("unexpected command: {command:?}"),
    }
    fixture.no_commands();
}

#[test]
fn liked_songs_use_right_click_without_a_trailing_button_or_starting_playback() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        window.hover(("spotify-track", 1usize), cx);
        assert!(window.try_find(("track-actions", 1usize)).is_none());
    });
    fixture.right_click_track(1);
    fixture.no_commands();
    fixture.update(|window, _| assert!(window.try_find(("track-menu-play", 1usize)).is_some()));
    fixture.right_click_track_at(2, 40.);
    fixture.no_commands();
    fixture.update(|window, _| {
        assert!(window.try_find(("track-menu-play", 1usize)).is_none());
        assert!(window.try_find(("track-menu-play", 2usize)).is_some());
    });
    fixture.update(|window, cx| window.click("queue-toggle", cx));
    fixture.update(|window, _| assert!(window.try_find(("track-menu-play", 2usize)).is_none()));
    fixture.no_commands();
}

/// A point on the menu surface just above `item`, in the menu's own padding,
/// so a press there lands on the menu rather than on any item in it.
fn menu_padding_above(item: &ElementSnapshot) -> Point<gpui_kit::Pixels> {
    let bounds = item.bounds();
    point(bounds.center().x, bounds.origin.y - px(2.))
}

#[test]
fn pressing_inside_the_track_menu_keeps_it_open_without_playing_the_row_beneath() {
    let mut fixture = Fixture::new(false);
    fixture.right_click_track(1);
    let menu_padding = fixture
        .update(|window, _| menu_padding_above(&window.find(("track-menu-heading", 1usize))));
    let row_beneath = fixture.update(|window, _| window.find(("spotify-track", 1usize)).bounds());
    assert!(row_beneath.contains(&menu_padding));

    fixture.mouse_down(menu_padding);
    fixture.mouse_up(menu_padding);

    fixture.update(|window, _| assert!(window.try_find(("track-menu-play", 1usize)).is_some()));
    fixture.no_commands();
}

#[test]
fn pressing_inside_the_account_menu_keeps_it_open_and_its_items_still_act() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("account", cx));
    let menu_padding =
        fixture.update(|window, _| menu_padding_above(&window.find("account-profile")));

    fixture.mouse_down(menu_padding);
    fixture.mouse_up(menu_padding);
    fixture.update(|window, _| assert!(window.try_find("account-settings").is_some()));

    fixture.update(|window, cx| window.click("account-settings", cx));
    assert_eq!(fixture.route(), Route::Settings);
    fixture.no_commands();
}

#[test]
fn clicking_the_avatar_again_closes_the_account_menu() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("account", cx));
    fixture.update(|window, _| assert!(window.try_find("account-settings").is_some()));
    // The press outside the menu closes it; the avatar's own click that
    // follows must not open it again.
    fixture.update(|window, cx| window.click("account", cx));
    fixture.update(|window, _| assert!(window.try_find("account-settings").is_none()));
    // And once closed, the next click opens it.
    fixture.update(|window, cx| window.click("account", cx));
    fixture.update(|window, _| assert!(window.try_find("account-settings").is_some()));
}

#[test]
fn mute_sends_volume_and_restores_the_previous_level() {
    let mut fixture = Fixture::new(false);
    let initial = fixture.update(|_, cx| services::AppServices::player(cx).read(cx).volume());
    fixture.update(|window, cx| window.click("volume", cx));
    assert_eq!(*fixture.backend.volume.borrow_and_update(), 0.);
    fixture.update(|window, cx| window.click("volume", cx));
    assert_eq!(*fixture.backend.volume.borrow_and_update(), initial);
    fixture.no_commands();
}

fn artist_ref(name: &str, source_id: &str) -> model::ArtistRef {
    model::ArtistRef {
        name: name.into(),
        source_id: Some(source_id.into()),
        spotify_uri: Some(format!("spotify:artist:{source_id}")),
    }
}

/// A track Spotify fully described, with two credited artists and an album.
pub(super) fn credited_track() -> model::Track {
    let mut track = track(0);
    track.artist = "Frankie Valli, The Four Seasons".into();
    track.artists = vec![
        artist_ref("Frankie Valli", "artist-valli"),
        artist_ref("The Four Seasons", "artist-seasons"),
    ];
    track.album_ref = Some(model::AlbumRef {
        name: "Grease".into(),
        source_id: Some("album-grease".into()),
        spotify_uri: Some("spotify:album:album-grease".into()),
        artwork_url: None,
    });
    track
}

#[test]
fn shared_rows_open_each_artist_and_album_from_every_library_collection() {
    for nav in ["nav-library", "nav-recent"] {
        for artist in [Some(0usize), Some(1), None] {
            let mut fixture = Fixture::new(false);
            fixture.update(|_, cx| {
                services::AppServices::library(cx).update(cx, |library, cx| {
                    library.handle_backend_event(
                        BackendEvent::LibraryLoaded {
                            generation: 0,
                            liked_tracks: vec![credited_track()],
                            playlists: vec![],
                        },
                        0,
                        cx,
                    );
                    library.handle_backend_event(
                        BackendEvent::LocalStateLoaded {
                            favorites: vec![credited_track()],
                            pinned_playlists: vec![],
                            recently_played: vec![credited_track()],
                        },
                        0,
                        cx,
                    );
                });
            });
            fixture.update(|window, cx| window.click(nav, cx));
            fixture.update(|window, cx| {
                let mut row = window.within(("spotify-track", 0usize));
                if let Some(artist) = artist {
                    assert_eq!(
                        row.find(("track-artist", artist)).role(),
                        Some(gpui_kit::Role::Link)
                    );
                    row.click(("track-artist", artist), cx);
                } else {
                    row.click(("track-album", 0usize), cx);
                }
            });
            if let Some(artist) = artist {
                assert_eq!(
                    fixture.requested_artist(),
                    if artist == 0 {
                        "artist-valli"
                    } else {
                        "artist-seasons"
                    }
                );
            } else {
                assert_eq!(fixture.requested_album(), "album-grease");
            }
            fixture.no_commands();
        }
    }
}

#[test]
fn row_artwork_is_plain_and_legacy_credits_stay_plain() {
    let mut fixture = Fixture::at_width(900.);
    fixture.update(|window, _| {
        assert!(window.try_find(("track-artist", 0usize)).is_none());
        assert!(window.try_find(("track-artwork", 0usize)).is_none());
    });
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![credited_track()],
                    playlists: vec![],
                },
                0,
                cx,
            );
        })
    });
    fixture.update(|window, _| {
        assert!(window.try_find(("track-album", 0usize)).is_none());
        assert!(window.try_find(("track-artwork", 0usize)).is_none());
    });
    fixture.no_commands();
}

#[test]
fn track_and_header_artist_links_work_inside_an_album_without_playing() {
    for header in [false, true] {
        let mut fixture = Fixture::new(false);
        let workspace = fixture.workspace.clone().unwrap();
        fixture.update(|_, cx| {
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace.open_album(credited_track().album_ref.unwrap(), Route::LikedSongs, cx);
            })
        });
        let BackendCommand::LoadAlbum { respond, .. } =
            fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("album request")
        };
        respond
            .send(Ok((
                model::Album {
                    provider: model::Provider::Spotify,
                    source_id: "album-grease".into(),
                    spotify_uri: None,
                    name: "Grease".into(),
                    artists: credited_track().artists,
                    release_date: None,
                    artwork_url: None,
                    track_count: Some(1),
                },
                vec![credited_track()],
            )))
            .unwrap();
        fixture.update(|_, _| {});
        fixture.update(|window, cx| {
            assert!(
                window.try_find(("track-artwork", 0usize)).is_none(),
                "album rows must not construct redundant cover elements"
            );
            if header {
                window.click(("album-artist", 1usize), cx);
            } else {
                window
                    .within(("spotify-track", 0usize))
                    .click(("track-artist", 1usize), cx);
            }
        });
        assert_eq!(fixture.requested_artist(), "artist-seasons");
        fixture.no_commands();
    }
}

/// Counts the open review track as fully played.
fn hear_review_track(fixture: &mut Fixture) {
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace
                .album
                .update(cx, |album, cx| album.hear_review_track(cx));
        })
    });
}

/// Opens the credited album with `count` plain tracks on the album page.
fn open_review_album(count: usize) -> Fixture {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| cx.set_reduce_motion(true));
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_album(credited_track().album_ref.unwrap(), Route::LikedSongs, cx);
        })
    });
    let BackendCommand::LoadAlbum { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("album request")
    };
    respond
        .send(Ok((
            model::Album {
                provider: model::Provider::Spotify,
                source_id: "album-grease".into(),
                spotify_uri: None,
                name: "Grease".into(),
                artists: credited_track().artists,
                release_date: None,
                artwork_url: None,
                track_count: Some(count as u32),
            },
            (0..count).map(track).collect(),
        )))
        .unwrap();
    fixture.update(|_, _| {});
    fixture
}

#[test]
fn album_review_unlocks_one_track_at_a_time_and_copies_the_result() {
    let mut fixture = open_review_album(3);
    let settle = |fixture: &mut Fixture| {
        fixture.cx.advance_clock(Duration::from_millis(400));
        fixture.update(|_, _| {});
    };
    fixture.update(|window, cx| window.click("album-review", cx));
    settle(&mut fixture);
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::PlayContext { index: 0, .. }
    ));
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 0usize))
                .try_find("review-panel")
                .is_some()
        );
        assert!(
            window
                .within(("review-item", 1usize))
                .try_find("review-panel")
                .is_none()
        );
    });
    // Locking in with no rating does nothing.
    fixture.update(|window, cx| window.click("review-lock-in", cx));
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 0usize))
                .try_find("review-panel")
                .is_some()
        );
    });
    // Until the song has been heard, stars and Enter do nothing.
    fixture.update(|window, cx| {
        window.click(("review-star", 4usize), cx);
        window.press("5", cx);
        window.press("enter", cx);
    });
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 0usize))
                .try_find("review-panel")
                .is_some()
        );
    });
    hear_review_track(&mut fixture);
    // Mouse: rate, then lock in.
    fixture.update(|window, cx| {
        window.click(("review-star", 4usize), cx);
        window.click("review-lock-in", cx);
    });
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 0usize))
                .try_find("review-panel")
                .is_none()
        );
        assert!(
            window
                .within(("review-item", 1usize))
                .try_find("review-panel")
                .is_some()
        );
    });
    // The next song is locked until it has been heard as well.
    fixture.update(|window, cx| {
        window.press("5", cx);
        window.press("enter", cx);
    });
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 1usize))
                .try_find("review-panel")
                .is_some()
        );
    });
    hear_review_track(&mut fixture);
    // Keyboard: digits rate, Enter locks in.
    fixture.update(|window, cx| {
        window.press("5", cx);
        window.press("enter", cx);
    });
    fixture.update(|window, _| {
        assert!(
            window
                .within(("review-item", 2usize))
                .try_find("review-panel")
                .is_some()
        );
        assert!(window.try_find("review-finish").is_none());
    });
    hear_review_track(&mut fixture);
    fixture.update(|window, cx| {
        window.press("3", cx);
        window.press("enter", cx);
    });
    settle(&mut fixture);
    fixture.update(|window, cx| {
        assert!(window.try_find("review-finish").is_some());
        assert!(window.try_find("review-panel").is_none());
        window.click("review-copy-text", cx);
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap();
        assert!(copied.contains("8.0 / 10"), "{copied}");
        assert!(copied.contains("Reviewed in Sonder"));
    });
}

#[test]
fn a_finished_review_copies_a_png_to_the_clipboard() {
    let mut fixture = open_review_album(2);
    fixture.update(|window, cx| window.click("album-review", cx));
    for stars in ["4", "5"] {
        hear_review_track(&mut fixture);
        fixture.update(|window, cx| {
            window.press(stars, cx);
            window.press("enter", cx);
        });
    }
    fixture.update(|window, cx| window.click("review-copy-image", cx));
    fixture.cx.run_until_parked();
    fixture.update(|_, cx| {
        let item = cx.read_from_clipboard().expect("an image was copied");
        let png = item
            .entries()
            .iter()
            .find_map(|entry| match entry {
                gpui_kit::ClipboardEntry::Image(image) => Some(image.bytes().to_vec()),
                _ => None,
            })
            .expect("the clipboard holds an image");
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), 1600);
        assert!(decoded.height() > 700);
    });
}

#[test]
fn locked_review_rows_do_not_play_and_leaving_the_album_ends_the_review() {
    let mut fixture = open_review_album(3);
    fixture.update(|window, cx| window.click("album-review", cx));
    let _ = fixture.backend.commands.try_recv();
    fixture.update(|window, cx| window.double_click(("review-row", 2usize), cx));
    fixture.no_commands();
    fixture.update(|window, cx| window.double_click(("review-row", 0usize), cx));
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::PlayContext { index: 0, .. }
    ));
    fixture.update(|window, cx| window.click("album-review", cx));
    fixture.update(|window, _| assert!(window.try_find("review-list").is_none()));
}

#[test]
fn repeating_the_same_search_does_not_ask_spotify_again() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input("Grease", cx);
        window.press("enter", cx);
        // Held or repeated Return while the search is still running.
        window.press("enter", cx);
        window.press("enter", cx);
    });
    let BackendCommand::SearchCatalog { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("search")
    };
    fixture.no_commands();
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![],
            tracks: vec![credited_track()],
            albums: vec![],
        }))
        .unwrap();
    fixture.update(|_, _| {});
    // Again once answered, with different capitalisation: still no request.
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.press("enter", cx);
    });
    fixture.no_commands();
    // A different query does search.
    fixture.update(|window, cx| {
        window.input(" 2", cx);
        window.press("enter", cx);
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::SearchCatalog { .. }
    ));
}

#[test]
fn search_track_album_links_use_loaded_metadata_without_searching_again() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input("Grease", cx);
        window.press("enter", cx);
    });
    let BackendCommand::SearchCatalog { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("search")
    };
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![],
            tracks: vec![credited_track()],
            albums: vec![],
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, cx| window.click(("track-album", 0usize), cx));
    assert_eq!(fixture.requested_album(), "album-grease");
    fixture.no_commands();
}

#[test]
fn track_links_work_from_keyboard_and_queue_without_playback_commands() {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![credited_track()],
                    playlists: vec![],
                },
                0,
                cx,
            );
        })
    });
    fixture.tab_to(("track-artist", 1usize));
    fixture.press("enter");
    assert_eq!(fixture.requested_artist(), "artist-seasons");
    fixture.no_commands();

    let mut next = credited_track();
    next.source_id = "linked-next".into();
    fixture.player_event(BackendEvent::PlaybackSnapshotLoaded {
        current: track(0),
        next: vec![next],
        position_ms: 0,
    });
    fixture.update(|window, cx| window.click("queue-toggle", cx));
    fixture.update(|window, cx| {
        window
            .within("queue-drawer")
            .within(("spotify-track", 1usize))
            .click(("track-artist", 0usize), cx)
    });
    assert_eq!(fixture.requested_artist(), "artist-valli");
    fixture.no_commands();
}

#[test]
fn loaded_playlist_row_opens_album_without_fetching_more_tracks() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(paged_playlist(), cx);
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("playlist")
    };
    respond
        .send(Ok(model::PlaylistTrackPage {
            tracks: vec![credited_track()],
            source_offsets: vec![],
            next_offset: None,
            total: 1,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, cx| window.click(("track-album", 0usize), cx));
    assert_eq!(fixture.requested_album(), "album-grease");
    fixture.no_commands();
}

#[test]
fn pinned_playlists_stay_above_recent_items_and_keep_their_order() {
    let mut fixture = Fixture::new(false);
    let playlists: Vec<_> = (0..6)
        .map(|i| model::Playlist {
            source_id: format!("playlist-{i}"),
            ..paged_playlist()
        })
        .collect();
    fixture.update(|_, cx| {
        cx.set_reduce_motion(true);
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![track(0)],
                    playlists: playlists.clone(),
                },
                0,
                cx,
            );
            library.handle_backend_event(
                BackendEvent::LocalStateLoaded {
                    favorites: vec![],
                    pinned_playlists: vec![playlists[1].clone(), playlists[3].clone()],
                    recently_played: vec![],
                },
                0,
                cx,
            );
        });
    });
    for clicked in [4usize, 2, 1, 3, 5] {
        fixture.update(|window, cx| {
            window.click(
                (
                    if clicked == 1 || clicked == 3 {
                        "pinned-playlist"
                    } else {
                        "sidebar-playlist"
                    },
                    clicked,
                ),
                cx,
            )
        });
        let BackendCommand::LoadPlaylist { .. } = fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("playlist navigation")
        };
        fixture.update(|window, _| {
            let first = window.find(("pinned-playlist", 1usize)).bounds();
            let second = window.find(("pinned-playlist", 3usize)).bounds();
            assert!(first.top() < second.top());
            for index in [0usize, 2, 4, 5] {
                assert!(second.top() < window.find(("sidebar-playlist", index)).bounds().top());
            }
        });
    }
    fixture.no_commands();
}

#[test]
fn discrete_wheel_preserves_distance_and_precise_scrolling_stays_direct() {
    fn run(smooth: bool, precise: bool) -> f32 {
        let mut fixture = Fixture::new(false);
        fixture.update(|_, cx| {
            cx.set_reduce_motion(!smooth);
            services::AppServices::library(cx).update(cx, |library, cx| {
                library.handle_backend_event(
                    BackendEvent::LibraryLoaded {
                        generation: 0,
                        liked_tracks: (0..200).map(track).collect(),
                        playlists: vec![],
                    },
                    0,
                    cx,
                );
            });
        });
        let before =
            fixture.update(|window, _| window.find(("spotify-track", 2usize)).bounds().top());
        fixture.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: window.find("track-scroll-viewport").bounds().center(),
                    delta: if precise {
                        gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-60.)))
                    } else {
                        gpui_kit::ScrollDelta::Lines(point(0., -3.))
                    },
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        if smooth && !precise {
            std::thread::sleep(Duration::from_millis(16));
            fixture.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            std::thread::sleep(Duration::from_millis(85));
            fixture.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
        }
        let after =
            fixture.update(|window, _| window.find(("spotify-track", 2usize)).bounds().top());
        (before - after).into()
    }
    let native = run(false, false);
    let smooth = run(true, false);
    assert!(native > 0.);
    assert!(
        (native - smooth).abs() < 1.,
        "smoothing must preserve wheel travel: {native} vs {smooth}"
    );
    assert!(
        (run(true, true) - 60.).abs() < 1.,
        "pixel input must scroll immediately"
    );
}

#[test]
fn scrollbar_gutter_keeps_duration_columns_stable_on_hover_and_drag() {
    for (width, zoom) in [(1280., 100), (900., 130)] {
        let mut fixture = Fixture::at_width(width);
        fixture.update(|_, cx| {
            appearance::Appearance::set_ui_zoom(
                crate::storage::UiZoom::from_percent(zoom).unwrap(),
                cx,
            )
            .unwrap();
            services::AppServices::library(cx).update(cx, |library, cx| {
                library.handle_backend_event(
                    BackendEvent::LibraryLoaded {
                        generation: 0,
                        liked_tracks: (0..200).map(track).collect(),
                        playlists: vec![],
                    },
                    0,
                    cx,
                );
            });
        });
        let (duration, viewport) = fixture.update(|window, _| {
            (
                window.find(("track-duration", 0usize)).bounds(),
                window.find("track-scroll-viewport").bounds(),
            )
        });
        assert!(duration.right() <= viewport.right() - px(16.));
        fixture.update(|window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: point(viewport.right() - px(8.), duration.center().y),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            )
        });
        fixture.update(|window, _| {
            assert_eq!(window.find(("track-duration", 0usize)).bounds(), duration)
        });
        fixture.update(|window, cx| {
            window.drag(
                point(viewport.right() - px(8.), viewport.top() + px(8.)),
                point(viewport.right() - px(8.), viewport.center().y),
                cx,
            )
        });
        fixture.update(|window, _| {
            let visible: Vec<_> = (0..200usize)
                .filter_map(|i| window.try_find(("track-duration", i)))
                .collect();
            assert!(!visible.is_empty());
            for item in visible {
                assert_eq!(item.bounds().right(), duration.right());
            }
        });
        fixture.no_commands();
    }
}

#[test]
fn player_bar_opens_each_credited_artist_and_the_album_separately() {
    let mut fixture = Fixture::new(false);
    fixture.play(credited_track());
    fixture.no_commands();

    fixture.update(|window, cx| window.click(("player-artist", 1usize), cx));
    assert_eq!(fixture.requested_artist(), "artist-seasons");
    assert_eq!(fixture.route(), Route::Artist);

    fixture.update(|window, cx| window.click(("player-artist", 0usize), cx));
    assert_eq!(fixture.requested_artist(), "artist-valli");
    assert_eq!(fixture.route(), Route::Artist);

    fixture.update(|window, cx| window.click("player-title", cx));
    assert_eq!(fixture.requested_album(), "album-grease");
    assert_eq!(fixture.route(), Route::Album);

    // Dropping the album reply above failed that load, so the title retries it.
    fixture.update(|window, cx| window.click("player-title", cx));
    assert_eq!(fixture.requested_album(), "album-grease");
    assert_eq!(fixture.route(), Route::Album);
    fixture.no_commands();
}

#[test]
fn player_bar_title_opens_the_album_from_the_keyboard() {
    let mut fixture = Fixture::new(false);
    fixture.play(credited_track());
    fixture.press(if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    });
    assert_eq!(fixture.route(), Route::Search);

    fixture.tab_to("player-title");
    fixture.update(|window, _| {
        assert_eq!(
            window.find("player-title").role(),
            Some(gpui_kit::Role::Link)
        );
    });
    fixture.press("enter");
    assert_eq!(fixture.requested_album(), "album-grease");
    assert_eq!(fixture.route(), Route::Album);
    fixture.no_commands();
}

#[test]
fn player_bar_credits_without_references_stay_plain() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, _| {
        assert!(window.try_find("player-title").is_none());
        assert!(window.try_find(("player-artist", 0usize)).is_none());
    });
    fixture.no_commands();
}

#[test]
fn unavailable_live_track_keeps_the_queue_and_skips_ahead() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-0".into(),
    });
    assert_eq!(fixture.marked_played(), "track-0");
    fixture.player_event(BackendEvent::TrackFailed {
        spotify_uri: "spotify:track:track-0".into(),
        error: "Spotify cannot play this track".into(),
    });
    fixture.update(|_, cx| {
        let player = services::AppServices::player(cx).read(cx);
        assert_eq!(
            player.now_playing().map(|track| track.source_id.as_str()),
            Some("track-0")
        );
        assert!(!player.playing());
    });
    assert_eq!(fixture.queued_source_ids(), ["track-1", "track-2"]);
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Next)
    ));
    fixture.no_commands();
}

#[test]
fn stale_track_failure_does_not_skip_ahead() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::TrackFailed {
        spotify_uri: "spotify:track:track-9".into(),
        error: "Spotify cannot play this track".into(),
    });
    assert_eq!(fixture.queued_source_ids(), ["track-1", "track-2"]);
    fixture.no_commands();
}

#[test]
fn track_failure_during_reconnect_does_not_skip_ahead() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackReconnecting);
    fixture.player_event(BackendEvent::TrackFailed {
        spotify_uri: "spotify:track:track-0".into(),
        error: "Spotify cannot play this track".into(),
    });
    assert_eq!(fixture.queued_source_ids(), ["track-1", "track-2"]);
    fixture.no_commands();
}

#[test]
fn history_is_recorded_once_the_track_plays_and_only_once() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: vec![track(4)],
    });
    fixture.no_commands();
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    assert_eq!(fixture.marked_played(), "track-3");
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    fixture.no_commands();
}

#[test]
fn a_track_that_fails_to_play_is_not_recorded() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: vec![track(4)],
    });
    fixture.player_event(BackendEvent::TrackFailed {
        spotify_uri: "spotify:track:track-3".into(),
        error: "Spotify cannot play this track".into(),
    });
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Next)
    ));
    fixture.no_commands();
}

#[test]
fn a_track_resumed_after_restart_is_recorded_when_it_plays() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-0".into(),
    });
    assert_eq!(fixture.marked_played(), "track-0");
    fixture.no_commands();
}

#[test]
fn a_reconnect_mid_track_records_history_once() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: vec![track(4)],
    });
    fixture.player_event(BackendEvent::PlaybackReconnecting);
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    fixture.no_commands();
    fixture.player_event(BackendEvent::PlaybackReconnected);
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::RestorePlayback { .. })
    ));
    fixture.player_event(BackendEvent::PlaybackRestored {
        position_ms: 0,
        playing: true,
    });
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    assert_eq!(fixture.marked_played(), "track-3");
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    fixture.no_commands();
}

/// The whole point of the scrub gesture: the position follows the pointer, and
/// the backend hears about it once, when the gesture ends.
#[test]
fn scrubbing_seeks_once_on_release_at_the_position_under_the_pointer() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    let bounds = fixture.update(|window, _| window.find("progress-slider").bounds());
    let at = |fraction: f32| {
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.center().y,
        )
    };
    fixture.update(|window, cx| window.drag(at(0.25), at(0.75), cx));

    // track(3) runs 240s, so releasing three quarters along asks for 3:00.
    match fixture.backend.commands.try_recv().expect("a seek") {
        BackendCommand::Seek(position_ms) => assert!(
            position_ms.abs_diff(180_000) < 2_000,
            "expected roughly 180000ms, got {position_ms}"
        ),
        other => panic!("expected a seek, got {other:?}"),
    }
    fixture.no_commands();
}

/// The seek maps through the track's painted bounds, so it stays correct when
/// the window is a size the layout constants were never written for.
#[test]
fn scrubbing_maps_through_the_painted_track_at_any_window_width() {
    for width in [1280., 1000., 760.] {
        let mut fixture = Fixture::at_width(width);
        fixture.player_event(BackendEvent::PlaybackContext {
            current: track(3),
            next: Vec::new(),
        });
        fixture.no_commands();

        let bounds = fixture.update(|window, _| window.find("progress-slider").bounds());
        let middle = point(bounds.origin.x + bounds.size.width * 0.5, bounds.center().y);
        fixture.update(|window, cx| window.drag(middle, middle, cx));

        match fixture.backend.commands.try_recv().expect("a seek") {
            BackendCommand::Seek(position_ms) => assert!(
                position_ms.abs_diff(120_000) < 2_000,
                "at width {width}: expected roughly 120000ms, got {position_ms}"
            ),
            other => panic!("expected a seek, got {other:?}"),
        }
    }
}

#[test]
fn arrow_keys_seek_once_the_scrubber_has_focus() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    fixture.update(|window, cx| window.click("progress-slider", cx));
    fixture.backend.commands.try_recv().expect("the click seek");
    fixture.update(|window, cx| window.press("right", cx));

    match fixture.backend.commands.try_recv().expect("a seek") {
        BackendCommand::Seek(position_ms) => assert!(
            position_ms.abs_diff(125_000) < 2_000,
            "expected roughly 125000ms, got {position_ms}"
        ),
        other => panic!("expected a seek, got {other:?}"),
    }
}

/// Escape during a scrub abandons it. No seek was ever sent, so there is
/// nothing to undo: commanding one would rewind playback by the drag's length.
#[test]
fn escape_during_a_scrub_abandons_it_without_seeking() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    let bounds = fixture.update(|window, _| window.find("progress-slider").bounds());
    let at = |fraction: f32| {
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.center().y,
        )
    };
    fixture.mouse_down(at(0.25));
    fixture.mouse_move_to(at(0.75));
    fixture.press("escape");
    fixture.no_commands();

    fixture.mouse_up(at(0.75));
    fixture.no_commands();
}

/// A gesture belongs to the track it began on. If that track goes away the
/// press must not commit against whatever is playing by the time it is released.
#[test]
fn a_scrub_whose_track_ends_mid_gesture_does_not_seek_the_next_one() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    let bounds = fixture.update(|window, _| window.find("progress-slider").bounds());
    let at = |fraction: f32| {
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.center().y,
        )
    };
    fixture.mouse_down(at(0.25));
    fixture.mouse_move_to(at(0.75));

    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(7),
        next: Vec::new(),
    });
    fixture.mouse_up(at(0.75));
    fixture.no_commands();
}

/// Guards the split press/move/release helpers the cancel tests rely on: the
/// preview has to actually follow the pointer, or those tests would pass for
/// the wrong reason. gpui's first qualifying move only opens the drag, so the
/// preview tracks from the second move onwards.
#[test]
fn a_scrub_preview_follows_the_pointer_before_it_commits() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    let bounds = fixture.update(|window, _| window.find("progress-slider").bounds());
    let at = |fraction: f32| {
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.center().y,
        )
    };
    fixture.mouse_down(at(0.25));
    fixture.mouse_move_to(at(0.5));
    fixture.mouse_move_to(at(0.75));
    fixture.no_commands();
    fixture.mouse_up(at(0.75));

    match fixture.backend.commands.try_recv().expect("a seek") {
        BackendCommand::Seek(position_ms) => assert!(
            position_ms.abs_diff(180_000) < 2_000,
            "expected roughly 180000ms, got {position_ms}"
        ),
        other => panic!("expected a seek, got {other:?}"),
    }
}

/// End means the end. Landing there finishes the track and moves on, which is
/// what every other player does.
#[test]
fn seeking_to_the_end_lands_on_the_end() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.no_commands();

    fixture.update(|window, cx| window.click("progress-slider", cx));
    fixture.backend.commands.try_recv().expect("the click seek");
    fixture.press("end");

    match fixture.backend.commands.try_recv().expect("a seek") {
        BackendCommand::Seek(position_ms) => assert_eq!(position_ms, 240_000),
        other => panic!("expected a seek, got {other:?}"),
    }
}

#[test]
fn closing_the_main_window_releases_the_workspace_while_services_keep_emitting() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    fixture
        .cx
        .update_window(fixture.window.into(), |_, window, _| window.remove_window())
        .expect("fixture window");
    fixture.cx.run_until_parked();
    fixture.cx.update(|cx| {
        assert!(workspace.upgrade().is_none());
        services::AppServices::session(cx).update(cx, |session, cx| {
            session.handle_backend_event(BackendEvent::SetupRequired, cx);
        });
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![track(0)],
                    playlists: Vec::new(),
                },
                0,
                cx,
            );
        });
    });
    fixture.cx.run_until_parked();
}

#[test]
fn reduced_motion_toggles_the_sidebar_straight_to_its_final_width() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    let sidebar_width = move |cx: &mut App| {
        let workspace = workspace.upgrade().expect("live workspace");
        workspace.read(cx).sidebar.read(cx).rendered_width()
    };
    // The collapsed rail clips the toggle, so press where its logo stays visible.
    let logo = point(px(20.), px(20.));
    fixture.update(|window, cx| {
        cx.set_reduce_motion(true);
        assert_eq!(sidebar_width(cx), EXPANDED_SIDEBAR_WIDTH);
        window.click_at("sidebar-toggle", logo, cx);
    });
    let collapsed_width = fixture.update(|_, cx| sidebar_width(cx));
    assert_eq!(collapsed_width, COLLAPSED_SIDEBAR_WIDTH);

    fixture.update(|window, cx| window.click_at("sidebar-toggle", logo, cx));
    let expanded_width = fixture.update(|_, cx| sidebar_width(cx));
    assert_eq!(expanded_width, EXPANDED_SIDEBAR_WIDTH);
}

#[test]
fn tab_reaches_the_sidebar_rows_and_enter_opens_the_focused_row() {
    let mut fixture = Fixture::new(false);
    fixture.tab_to("nav-recent");

    fixture.press("enter");
    assert_eq!(fixture.route(), Route::Recent);
    assert!(fixture.focused("nav-recent"));
    fixture.no_commands();
}

#[test]
fn space_activates_a_focused_transport_control_instead_of_toggling_playback() {
    let mut fixture = Fixture::new(false);
    fixture.tab_to("progress-slider");
    // Back past the lyrics and repeat buttons to Next.
    fixture.press("shift-tab");
    fixture.press("shift-tab");
    fixture.press("shift-tab");

    fixture.press("space");
    assert!(matches!(
        fixture.backend.commands.try_recv().expect("next track"),
        BackendCommand::Next
    ));
    fixture.no_commands();
}

#[test]
fn the_repeat_button_cycles_all_then_one_then_off_and_tells_the_backend() {
    let mut fixture = Fixture::new(false);
    for expected in [model::Repeat::All, model::Repeat::One, model::Repeat::Off] {
        fixture.update(|window, cx| window.click("repeat", cx));
        match fixture.backend.commands.try_recv().expect("repeat mode") {
            BackendCommand::SetRepeat(mode) => assert_eq!(mode, expected),
            other => panic!("unexpected command {other:?}"),
        }
    }
    fixture.no_commands();
}

/// Opens the small player the way the app does, in a window beside the main one.
fn open_mini(fixture: &mut Fixture, shape: Shape) -> WindowHandle<MiniPlayer> {
    let (width, height) = shape.size();
    let handle = fixture
        .cx
        .open_window(size(px(width), px(height)), |_, cx| {
            cx.new(|cx| MiniPlayer::new(shape, cx))
        })
        .expect("mini player window");
    settle(&mut fixture.cx, handle.into());
    handle
}

#[test]
fn the_mini_player_controls_drive_the_shared_player() {
    let mut fixture = Fixture::new(false);
    fixture.play(track(0));
    let mini = open_mini(&mut fixture, Shape::Mini);
    for (button, check) in [
        ("mini-play", "resume"),
        ("mini-next", "next"),
        ("mini-repeat", "repeat"),
    ] {
        fixture
            .cx
            .update_window(mini.into(), |_, window, cx| window.click(button, cx))
            .expect("click mini button");
        settle(&mut fixture.cx, mini.into());
        let command = fixture.backend.commands.try_recv().expect(check);
        match (check, command) {
            ("resume", BackendCommand::Resume) => {}
            ("next", BackendCommand::Next) => {}
            ("repeat", BackendCommand::SetRepeat(model::Repeat::All)) => {}
            (_, other) => panic!("{button} sent {other:?}"),
        }
    }
    fixture.no_commands();
}

#[test]
fn the_mini_player_switches_between_its_two_shapes() {
    let mut fixture = Fixture::new(false);
    fixture.play(track(0));
    let mini = open_mini(&mut fixture, Shape::Mini);
    for expected in [Shape::Strip, Shape::Mini] {
        fixture
            .cx
            .update_window(mini.into(), |_, window, cx| window.click("mini-shape", cx))
            .expect("click shape");
        settle(&mut fixture.cx, mini.into());
        let shape = mini
            .read_with(&fixture.cx, |view, _| view.shape())
            .expect("read shape");
        assert_eq!(shape, expected);
    }
}

#[test]
fn a_pointer_press_leaves_keyboard_focus_with_the_workspace() {
    let mut fixture = Fixture::new(false);
    fixture.press(if cfg!(target_os = "macos") {
        "cmd-k"
    } else {
        "ctrl-k"
    });
    assert!(fixture.focused("search-input"));

    fixture.update(|window, cx| window.click("nav-recent", cx));
    assert_eq!(fixture.route(), Route::Recent);
    assert!(!fixture.focused("nav-recent"));
    assert!(!fixture.focused("search-input"));

    fixture.press("space");
    assert!(matches!(
        fixture
            .backend
            .commands
            .try_recv()
            .expect("playback shortcut"),
        BackendCommand::Resume
    ));
    fixture.no_commands();
}

#[test]
fn space_after_clicking_the_lyrics_or_queue_button_still_plays_and_pauses() {
    for button in ["lyrics-toggle", "queue-toggle"] {
        let mut fixture = Fixture::new(false);
        fixture.play(track(0));
        fixture.update(|window, cx| window.click(button, cx));
        assert!(
            !fixture.focused(button),
            "{button} kept focus after a click"
        );
        while fixture.backend.commands.try_recv().is_ok() {}
        let was_playing =
            fixture.update(|_, cx| services::AppServices::player(cx).read(cx).playing());
        fixture.press("space");
        fixture.update(|window, cx| {
            let playing = services::AppServices::player(cx).read(cx).playing();
            assert_ne!(playing, was_playing, "space did not toggle playback");
            // The panel stays as the click left it.
            let open = window.try_find("lyrics-panel").is_some()
                || window.try_find("close-queue").is_some();
            assert!(open, "space closed the panel opened by {button}");
        });
    }
}

#[test]
fn the_play_button_and_space_drive_the_same_playback_toggle() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("play-toggle", cx));
    assert!(matches!(
        fixture.backend.commands.try_recv().expect("play button"),
        BackendCommand::Resume
    ));
    fixture.update(|_, cx| assert!(services::AppServices::player(cx).read(cx).playing()));

    fixture.press("space");
    assert!(matches!(
        fixture
            .backend
            .commands
            .try_recv()
            .expect("playback shortcut"),
        BackendCommand::Pause
    ));
    fixture.update(|_, cx| assert!(!services::AppServices::player(cx).read(cx).playing()));
    fixture.no_commands();
}

#[test]
fn escape_cancels_the_sign_in_app_change_wherever_keyboard_focus_is() {
    for tab_presses in 0..10 {
        let mut fixture = Fixture::sign_in_window();
        fixture.update(|_, cx| {
            services::AppServices::session(cx)
                .update(cx, |session, cx| session.request_app_change(cx));
        });
        for _ in 0..tab_presses {
            fixture.press("tab");
        }
        fixture.press("escape");
        let open = fixture.app_change_confirmation_open();
        assert!(
            !open,
            "Escape left the confirmation open after {tab_presses} Tab presses"
        );
        fixture.no_commands();
    }
}

#[test]
fn empty_artist_still_has_both_sections_after_skeleton_loading() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_artist(
                artist_ref("Empty artist", "empty-artist"),
                Route::LikedSongs,
                cx,
            );
        })
    });
    let BackendCommand::LoadArtist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("artist request")
    };
    respond
        .send(Ok((
            model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "empty-artist".into(),
                spotify_uri: None,
                name: "Empty artist".into(),
                artwork_url: None,
            },
            vec![],
            vec![].into(),
        )))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find("artist-popular-heading").is_some());
        assert!(window.try_find("artist-discography-heading").is_some());
    });
}

#[test]
fn artist_tracks_scroll_into_virtualized_discography_and_albums_still_open() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        cx.set_reduce_motion(true);
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_artist(artist_ref("Artist", "artist-flow"), Route::LikedSongs, cx);
        });
    });
    let BackendCommand::LoadArtist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected artist request");
    };
    respond
        .send(Ok((
            model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "artist-flow".into(),
                spotify_uri: None,
                name: "Artist".into(),
                artwork_url: None,
            },
            vec![track(0), track(1), track(2)],
            (0..400)
                .map(|index| model::Album {
                    provider: model::Provider::Spotify,
                    source_id: format!("album-{index}"),
                    spotify_uri: None,
                    name: format!("Album {index}").into(),
                    artists: Vec::new(),
                    release_date: None,
                    artwork_url: None,
                    track_count: Some(10),
                })
                .collect::<Vec<_>>()
                .into(),
        )))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, cx| {
        let last_track = window.find(("spotify-track", 2usize)).bounds();
        assert!(window.find("artist-discography-heading").bounds().origin.y >= last_track.bottom());
        assert!(
            window.try_find(("artist-album", 399usize)).is_none(),
            "offscreen albums stay virtualized"
        );
        let position = window.find("artist-portrait").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-450.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|window, cx| {
        let position = point(px(700.), px(400.));
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(450.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|window, cx| {
        let portrait = window.find("artist-portrait").bounds();
        assert_eq!(
            portrait.size.width,
            gpui_kit::Rems(15.).to_pixels(window.rem_size())
        );
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position: portrait.center(),
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-450.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|_, cx| {
        appearance::Appearance::set_ui_zoom(crate::storage::UiZoom::from_percent(110).unwrap(), cx)
            .unwrap();
    });
    fixture.update(|window, _| {
        assert!(
            window.find("artist-dock-portrait").bounds().size.width
                < gpui_kit::Rems(3.).to_pixels(window.rem_size()),
            "zoom must preserve the scrolled page instead of resetting to its header"
        );
    });
    let visible_album = fixture.update(|window, cx| {
        let top = crate::app::tokens::TOOLBAR_HEIGHT.to_pixels(window.rem_size());
        let portrait = window.find("artist-dock-portrait").bounds();
        let search = window.find("search-field").bounds();
        let account = window.find("account").bounds();
        assert!(
            (portrait.size.width - gpui_kit::Rems(2.).to_pixels(window.rem_size())).abs() < px(1.)
        );
        assert!(portrait.left() >= search.right());
        assert!(window.find("artist-dock-name").bounds().right() <= account.left());
        let index = (0usize..40)
            .find(|index| {
                window
                    .try_find(("artist-album", *index))
                    .is_some_and(|card| {
                        card.bounds().top() >= top
                            && card.bounds().bottom() < window.viewport_size().height - px(100.)
                    })
            })
            .expect("one complete discography card is visible after scrolling");
        window.click(("artist-album", index), cx);
        index
    });
    assert_eq!(fixture.requested_album(), format!("album-{visible_album}"));
    assert_eq!(fixture.route(), Route::Album);
}

#[test]
fn short_artist_pages_fully_dock_and_back_works_after_switching_artists() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    for (id, origin) in [("glaive", Route::LikedSongs), ("kurtains", Route::Artist)] {
        fixture.update(|_, cx| {
            cx.set_reduce_motion(true);
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace.open_artist(artist_ref(id, id), origin, cx);
            });
        });
        let BackendCommand::LoadArtist { respond, .. } =
            fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("artist request")
        };
        respond
            .send(Ok((
                model::Artist {
                    genres: Vec::new(),
                    provider: model::Provider::Spotify,
                    source_id: id.into(),
                    spotify_uri: None,
                    name: id.into(),
                    artwork_url: None,
                },
                vec![track(0), track(1), track(2)],
                vec![model::Album {
                    provider: model::Provider::Spotify,
                    source_id: format!("{id}-album"),
                    spotify_uri: None,
                    name: "Album".into(),
                    artists: Vec::new(),
                    release_date: None,
                    artwork_url: None,
                    track_count: Some(3),
                }]
                .into(),
            )))
            .unwrap();
        fixture.update(|_, _| {});
        fixture.update(|window, _| {
            assert_eq!(
                window.find("artist-portrait").bounds().size.width,
                gpui_kit::Rems(15.).to_pixels(window.rem_size()),
                "new artist resets the header"
            );
        });
        fixture.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: window.find("artist-portrait").bounds().center(),
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-2000.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        fixture.update(|window, _| {
            let portrait = window.find("artist-dock-portrait").bounds();
            assert_eq!(
                portrait.size.width,
                gpui_kit::Rems(2.).to_pixels(window.rem_size()),
                "short page must finish docking"
            );
            assert!(portrait.left() >= window.find("search-field").bounds().right());
        });
    }
    fixture.update(|window, cx| window.click("detail-back", cx));
    assert_eq!(
        fixture.route(),
        Route::LikedSongs,
        "Back must be clickable and cannot point to the current artist route"
    );
}

#[test]
fn tracks_use_double_click_and_liked_songs_have_no_column_titles() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        assert!(window.try_find("track-list-header").is_none());
        window.click(("spotify-track", 1usize), cx);
    });
    fixture.no_commands();
    fixture.update(|window, cx| window.double_click(("spotify-track", 1usize), cx));
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::PlayContext { index: 1, .. }
    ));
    fixture.no_commands();
}

fn paged_playlist() -> model::Playlist {
    model::Playlist { provider: model::Provider::Spotify, source_id: "large-playlist".into(), name: "A very long playlist name that should stay inside the sidebar without widening the window".into(), owner: "Owner".into(), track_count: 1000, artwork_url: None }
}

#[test]
fn playlist_fetches_small_pages_only_near_the_scroll_end_and_preserves_playback_cursor() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(paged_playlist(), cx);
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected first page");
    };
    assert_eq!(offset, 0);
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![],
            tracks: (0..15).map(track).collect(),
            next_offset: Some(15),
            total: 1000,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.no_commands();
    fixture.update(|window, cx| window.click(("spotify-track", 1usize), cx));
    fixture.no_commands();
    fixture.update(|window, cx| window.double_click(("spotify-track", 1usize), cx));
    match fixture.backend.commands.try_recv().unwrap() {
        BackendCommand::PlayPlaylistContext {
            tracks,
            index,
            next_offset,
            ..
        } => {
            assert_eq!(tracks.len(), 15);
            assert_eq!(index, 1);
            assert_eq!(next_offset, Some(15));
        }
        _ => panic!("expected paged playback context"),
    }
    fixture.update(|window, cx| {
        let position = window.find(("spotify-track", 1usize)).bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-850.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.no_commands();
    fixture.cx.advance_clock(Duration::from_millis(300));
    fixture.update(|_, _| {});
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected next page");
    };
    assert_eq!(offset, 15);
    fixture.no_commands(); // further frames/scrolls cannot duplicate an in-flight request
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![],
            tracks: (15..30).map(track).collect(),
            next_offset: None,
            total: 30,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.no_commands();
}

#[test]
fn playlist_keeps_raw_offsets_for_unavailable_rows_and_retries_visible_pages() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(paged_playlist(), cx);
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![0, 2, 5],
            tracks: (0..3).map(track).collect(),
            next_offset: Some(15),
            total: 40,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.no_commands();
    // The taller detail header leaves fewer rows initially visible.
    fixture.update(|window, cx| {
        let position = window.find(("spotify-track", 0usize)).bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-200.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|window, cx| window.double_click(("spotify-track", 5usize), cx));
    let BackendCommand::PlayPlaylistContext {
        index,
        next_offset,
        tracks,
        ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!((index, next_offset, tracks.len()), (2, Some(15), 3));
    fixture.update(|window, cx| {
        let position = window.find(("spotify-track", 5usize)).bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-650.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.cx.advance_clock(Duration::from_millis(300));
    fixture.update(|_, _| {});
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!(offset, 15);
    respond.send(Err(anyhow::anyhow!("offline"))).unwrap();
    fixture.update(|_, _| {});
    fixture.cx.advance_clock(Duration::from_secs(1));
    fixture.update(|_, _| {});
    fixture.no_commands(); // an error banner resizing the viewport must not trigger retries
    fixture.update(|window, cx| window.click("playlist-retry-page", cx));
    fixture.cx.advance_clock(Duration::from_millis(300));
    fixture.update(|_, _| {});
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::LoadPlaylist { offset: 15, .. }
    ));
}

#[test]
fn playlist_scroll_jumps_directly_to_the_final_viewport_after_debounce() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(
                model::Playlist {
                    track_count: 100_000,
                    ..paged_playlist()
                },
                cx,
            );
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![],
            tracks: (0..15).map(track).collect(),
            next_offset: Some(15),
            total: 100_000,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    let position =
        fixture.update(|window, _| window.find(("spotify-track", 1usize)).bounds().center());
    for delta in [-100_000_000., 100_000_000., -100_000_000.] {
        fixture.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Default::default(),
                    touch_phase: gpui_kit::TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            )
        });
    }
    fixture.update(|_, _| {});
    fixture.cx.advance_clock(Duration::from_millis(299));
    fixture.update(|_, _| {});
    fixture.no_commands();
    fixture.cx.advance_clock(Duration::from_millis(1));
    fixture.update(|_, _| {});
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("visible page request")
    };
    assert_eq!(offset, 99_990);
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![],
            tracks: (99_990..100_000).map(track).collect(),
            next_offset: None,
            total: 100_000,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, cx| window.double_click(("spotify-track", 99_999usize), cx));
    let BackendCommand::PlayPlaylistContext {
        tracks,
        index,
        next_offset,
        ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!((tracks.len(), index, next_offset), (10, 9, None));
    fixture.no_commands();
}

#[test]
fn selection_color_setting_applies_immediately_and_persists() {
    let mut fixture = Fixture::new(true);
    fixture.update(|window, cx| {
        window.click(("settings-selection-color", 3usize), cx);
    });
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).selection_color,
            crate::storage::SelectionColor::Purple
        );
        assert_eq!(
            appearance::Appearance::palette(cx).selection,
            crate::storage::SelectionColor::Purple.fill(true)
        );
    });
    fixture.no_commands();
}

#[test]
fn queue_uses_shared_rows_and_double_click_keeps_the_existing_playback_context() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("queue-toggle", cx));
    fixture.update(|window, cx| {
        assert!(
            window
                .within("queue-drawer")
                .try_find(("track-actions", 1usize))
                .is_none()
        );
        window
            .within("queue-drawer")
            .click(("spotify-track", 1usize), cx);
    });
    fixture.no_commands();
    fixture.update(|window, cx| {
        window
            .within("queue-drawer")
            .double_click(("spotify-track", 1usize), cx)
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::PlayQueueItem { offset: 1 }
    ));
    fixture.no_commands();
}

#[test]
fn sidebar_shows_unpinned_playlists_truncates_names_and_virtualizes_long_libraries() {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![track(0)],
                    playlists: (0..200)
                        .map(|index| model::Playlist {
                            source_id: format!("playlist-{index}"),
                            ..paged_playlist()
                        })
                        .collect(),
                },
                0,
                cx,
            );
        });
    });
    fixture.update(|window, cx| {
        let row = window.find(("sidebar-playlist", 0usize)).bounds();
        assert!(row.size.width < px(240.));
        assert!(row.right() < px(240.));
        assert!(window.try_find(("sidebar-playlist", 199usize)).is_none());
        window.click(("sidebar-playlist", 0usize), cx);
    });
    let BackendCommand::LoadPlaylist {
        playlist, offset, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected playlist navigation");
    };
    assert_eq!(playlist.source_id, "playlist-0");
    assert_eq!(offset, 0);
}

#[test]
fn current_track_index_toggles_playback_without_replacing_the_queue() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        assert_eq!(
            window.find(("track-playback-toggle", 0usize)).label(),
            Some("Play")
        );
        assert!(window.try_find(("track-playback-toggle", 1usize)).is_none());
        window.click(("track-playback-toggle", 0usize), cx);
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::Resume
    ));
    fixture.update(|window, cx| {
        assert_eq!(
            window.find(("track-playback-toggle", 0usize)).label(),
            Some("Pause")
        );
        window.click(("track-playback-toggle", 0usize), cx);
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::Pause
    ));
    fixture.update(|window, cx| window.double_click(("track-playback-toggle", 0usize), cx));
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::Resume
    ));
    fixture.no_commands();
    fixture.update(|window, cx| window.click("queue-toggle", cx));
    fixture.update(|window, cx| {
        window
            .within("queue-drawer")
            .click(("track-playback-toggle", 0usize), cx)
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::Pause
    ));
    fixture.no_commands();
}

#[test]
fn zoom_scales_layout_and_transport_and_survives_theme_changes() {
    let mut fixture = Fixture::new(true);
    let original = fixture.update(|window, _| window.find("play-toggle").bounds().size);
    fixture.update(|window, cx| window.click(("settings-ui-zoom", 130usize), cx));
    fixture.update(|window, cx| {
        assert_eq!(window.rem_size(), px(20.8));
        assert_eq!(
            services::AppServices::preferences(cx).ui_zoom.percent(),
            130
        );
        let width = f32::from(window.find("play-toggle").bounds().size.width);
        assert!((width - f32::from(original.width) * 1.3).abs() < 0.5);
        appearance::Appearance::set_preference(ThemePreference::Light, window, cx);
    });
    fixture.update(|window, cx| {
        assert_eq!(window.rem_size(), px(20.8));
        window.click(("settings-ui-zoom", 70usize), cx);
    });
    fixture.update(|window, cx| {
        assert_eq!(window.rem_size(), px(11.2));
        let width = f32::from(window.find("play-toggle").bounds().size.width);
        assert!((width - f32::from(original.width) * 0.7).abs() < 0.5);
        window.click(("settings-ui-zoom", 100usize), cx);
    });
    fixture.update(|window, _| assert_eq!(window.rem_size(), px(16.)));
    fixture.no_commands();
}

#[test]
fn collapsed_sidebar_keeps_playlist_buttons_inside_rail_and_promotes_opened_playlist() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.handle_backend_event(
                BackendEvent::LibraryLoaded {
                    generation: 0,
                    liked_tracks: vec![track(0)],
                    playlists: (0..200)
                        .map(|i| model::Playlist {
                            source_id: format!("playlist-{i}"),
                            ..paged_playlist()
                        })
                        .collect(),
                },
                0,
                cx,
            );
        });
    });
    fixture.update(|window, cx| window.click(("sidebar-playlist", 2usize), cx));
    let BackendCommand::LoadPlaylist { playlist, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected playlist request")
    };
    assert_eq!(playlist.source_id, "playlist-2");
    fixture.update(|window, cx| {
        assert!(
            window.find(("sidebar-playlist", 2usize)).bounds().top()
                < window.find(("sidebar-playlist", 0usize)).bounds().top()
        );
        // The rail springs to its width; this test checks where it ends up.
        cx.set_reduce_motion(true);
        workspace
            .upgrade()
            .unwrap()
            .read(cx)
            .sidebar
            .clone()
            .update(cx, |sidebar, cx| sidebar.set_collapsed(true, cx));
    });
    fixture.update(|window, cx| {
        assert_eq!(
            window.find(("sidebar-playlist", 2usize)).selected(),
            Some(true)
        );
        assert_eq!(window.find("nav-playlist").selected(), Some(false));
        assert!(
            window.find("nav-home").bounds().top()
                <= window.find("sidebar-toggle").bounds().bottom() + px(20.)
        );
        let rail_right = COLLAPSED_SIDEBAR_WIDTH.to_pixels(window.rem_size());
        for id in ["nav-home", "nav-library", "nav-playlist", "nav-recent"] {
            assert!(window.find(id).bounds().right() <= rail_right);
        }
        let cover = window.find(("sidebar-playlist", 2usize)).bounds();
        assert!(cover.right() <= rail_right);
        assert!(window.try_find(("sidebar-playlist", 199usize)).is_none());
        window.click(("sidebar-playlist", 1usize), cx);
    });
    let BackendCommand::LoadPlaylist { playlist, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected collapsed playlist navigation")
    };
    assert_eq!(playlist.source_id, "playlist-1");
}

#[test]
fn clearing_artwork_cache_releases_entries_without_changing_account_or_queue() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        let cache = services::AppServices::image_cache(cx);
        cache.update(cx, |cache, _| {
            let image = image::RgbaImage::new(32, 32);
            let image = Arc::new(gpui_kit::RenderImage::new([image::Frame::new(image)]));
            cache.seed_artist_fixture("https://fixtures.invalid/cache.png", image);
        });
        assert!(cache.read(cx).usage().0 > 0);
        let player = services::AppServices::player(cx);
        let before = player.read(cx).is_current_track(&track(0));
        cache.update(cx, |cache, cx| cache.clear(window, cx));
        assert_eq!(cache.read(cx).usage().0, 0);
        assert_eq!(player.read(cx).is_current_track(&track(0)), before);
    });
    fixture.no_commands();
}

#[test]
fn the_account_menu_holds_only_profile_settings_and_logout() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("account", cx));
    fixture.update(|window, _| {
        assert!(window.try_find("account-whats-new").is_none());
        assert!(window.try_find("account-profile").is_some());
        assert!(window.try_find("account-settings").is_some());
    });
}

#[cfg(target_os = "windows")]
#[test]
fn windows_brand_and_search_share_the_same_vertical_center() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, _| {
        assert_eq!(
            window.find("sidebar-toggle").bounds().center().y,
            window.find("search-field").bounds().center().y
        )
    });
}

#[test]
fn settings_cache_cleanup_button_is_reachable_and_does_not_send_playback_commands() {
    let mut fixture = Fixture::new(true);
    // Cache lives under the System tab (fifth).
    fixture.update(|window, cx| window.click(("settings-tab", 4usize), cx));
    fixture.update(|window, cx| {
        let position = window.find("settings-page").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-3000.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|window, cx| window.click("settings-clear-cache", cx));
    assert_eq!(fixture.route(), Route::Settings);
    fixture.no_commands();
}

fn open_voice_settings(fixture: &mut Fixture) {
    fixture.update(|window, cx| window.click(("settings-tab", 3usize), cx));
}

/// The page is taller than the window now that it lists three models.
fn scroll_voice_settings_down(fixture: &mut Fixture) {
    fixture.update(|window, cx| {
        let position = window.find("settings-page").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-3000.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
}

fn voice_config(fixture: &mut Fixture) -> crate::voice::Config {
    fixture.update(|_, cx| {
        services::AppServices::voice_play(cx)
            .read(cx)
            .config()
            .clone()
    })
}

#[test]
fn voice_play_cannot_be_turned_on_without_its_model_and_says_why() {
    let mut fixture = Fixture::new(true);
    open_voice_settings(&mut fixture);
    assert!(!voice_config(&mut fixture).enabled);
    fixture.update(|window, cx| window.click("settings-voice-enabled", cx));
    assert!(!voice_config(&mut fixture).enabled);
    // The reason is on the page, so the switch never fails silently.
    assert!(fixture.update(|window, _| window.try_find("settings-voice-error").is_some()));
    fixture.no_commands();
}

#[test]
fn the_voice_shortcut_is_changed_by_pressing_the_new_one() {
    let mut fixture = Fixture::new(true);
    open_voice_settings(&mut fixture);
    assert_eq!(
        voice_config(&mut fixture).hotkey,
        crate::voice::Hotkey::default()
    );
    fixture.update(|window, cx| window.click("settings-voice-shortcut", cx));
    // A bare letter would take over typing, so it is refused and listening
    // continues.
    fixture.press("v");
    assert_eq!(
        voice_config(&mut fixture).hotkey,
        crate::voice::Hotkey::default()
    );
    assert!(fixture.update(|window, _| window.try_find("settings-voice-error").is_some()));
    fixture.press("ctrl-shift-v");
    let hotkey = voice_config(&mut fixture).hotkey;
    assert_eq!(hotkey.encode(), "ctrl+shift+v");
    // Listening ended, and Reset brings the default back.
    fixture.press("x");
    assert_eq!(voice_config(&mut fixture).hotkey, hotkey);
    fixture.update(|window, cx| window.click("settings-voice-shortcut-reset", cx));
    assert_eq!(
        voice_config(&mut fixture).hotkey,
        crate::voice::Hotkey::default()
    );
}

#[test]
fn escape_stops_waiting_for_a_voice_shortcut() {
    let mut fixture = Fixture::new(true);
    open_voice_settings(&mut fixture);
    fixture.update(|window, cx| window.click("settings-voice-shortcut", cx));
    fixture.press("escape");
    fixture.press("ctrl-alt-q");
    assert_eq!(
        voice_config(&mut fixture).hotkey,
        crate::voice::Hotkey::default()
    );
}

#[test]
fn the_voice_model_can_be_chosen_before_it_is_downloaded() {
    use crate::voice::Model;
    let mut fixture = Fixture::new(true);
    open_voice_settings(&mut fixture);
    scroll_voice_settings_down(&mut fixture);
    assert_eq!(voice_config(&mut fixture).model, Model::Standard);
    for (index, expected) in [
        (2usize, Model::Accurate),
        (0, Model::Fast),
        (1, Model::Standard),
    ] {
        fixture.update(|window, cx| window.click(("settings-voice-model", index), cx));
        assert_eq!(voice_config(&mut fixture).model, expected);
    }
}

#[test]
fn volume_slider_uses_live_viewport_after_fullscreen_and_resize() {
    let mut fixture = Fixture::new(false);
    for (width, fraction) in [(1920., 0.25f32), (2560., 0.75), (1280., 0.5)] {
        fixture.update(|window, _| {
            window.toggle_fullscreen();
            window.resize(size(px(width), px(1080.)));
        });
        fixture.update(|window, cx| {
            let slider = window.find("volume-slider").bounds();
            let position = point(
                slider.left() + slider.size.width * fraction,
                slider.center().y,
            );
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
        });
        assert!(
            (*fixture.backend.volume.borrow_and_update() - fraction).abs() < 0.02,
            "width {width} got {} want {fraction}",
            *fixture.backend.volume.borrow()
        );
        fixture.update(|window, cx| {
            let position = window.find("volume-slider").bounds().center();
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
        });
        fixture.no_commands();
    }
}

#[test]
fn shuffle_toggle_is_lightweight_and_preserves_transport_bounds_and_playback() {
    let mut fixture = Fixture::new(false);
    let before = fixture.update(|window, cx| {
        let player = services::AppServices::player(cx);
        assert!(!player.read(cx).shuffle());
        (
            window.find("play-toggle").bounds(),
            player.read(cx).now_playing().cloned(),
        )
    });
    fixture.update(|window, cx| window.click("shuffle", cx));
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::SetShuffle(true)
    ));
    fixture.update(|window, cx| {
        let player = services::AppServices::player(cx);
        assert!(player.read(cx).shuffle());
        assert_eq!(player.read(cx).now_playing(), before.1.as_ref());
        assert_eq!(window.find("play-toggle").bounds(), before.0);
        window.click("shuffle", cx);
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::SetShuffle(false)
    ));
    fixture.no_commands();
}

#[test]
fn search_places_artists_above_tracks_and_opens_the_selected_artist() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input("glaive", cx);
        window.press("enter", cx);
    });
    let BackendCommand::SearchCatalog { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("search request")
    };
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "glaive-id".into(),
                spotify_uri: None,
                name: "glaive".into(),
                artwork_url: None,
            }],
            tracks: vec![track(0)],
            albums: vec![],
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, cx| {
        assert!(
            window.find("search-top-result").bounds().top()
                <= window.find(("spotify-track", 0usize)).bounds().top()
        );
        assert!(
            window.find(("spotify-track", 0usize)).bounds().bottom()
                <= window.find(("search-artist", 0usize)).bounds().top()
        );
        window.click(("search-artist", 0usize), cx);
    });
    assert_eq!(fixture.requested_artist(), "glaive-id");
    fixture.no_commands();
}

#[test]
fn search_filters_narrow_results_and_the_top_result_opens_its_artist() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| {
        window.click("search-input", cx);
        window.input("glaive", cx);
        window.press("enter", cx);
    });
    let BackendCommand::SearchCatalog { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("search request")
    };
    respond
        .send(Ok(model::CatalogSearchResults {
            artists: vec![model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "glaive-id".into(),
                spotify_uri: None,
                name: "glaive".into(),
                artwork_url: None,
            }],
            tracks: vec![track(0)],
            albums: vec![],
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find("search-top-result").is_some());
        assert!(window.try_find(("spotify-track", 0usize)).is_some());
        assert!(window.try_find(("search-artist", 0usize)).is_some());
    });
    fixture.update(|window, cx| window.click(("search-filter", 2usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("search-top-result").is_none());
        assert!(window.try_find(("spotify-track", 0usize)).is_none());
        assert!(window.try_find(("search-artist", 0usize)).is_some());
    });
    fixture.update(|window, cx| window.click(("search-filter", 0usize), cx));
    fixture.update(|window, cx| window.click("search-top-result", cx));
    assert_eq!(fixture.requested_artist(), "glaive-id");
    fixture.no_commands();
}

#[test]
fn returning_to_a_recent_playlist_restores_loaded_tracks_and_scroll_without_fetching() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    let open = |fixture: &mut Fixture, id: &str| {
        fixture.update(|_, cx| {
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace.load_playlist(
                    model::Playlist {
                        source_id: id.into(),
                        ..paged_playlist()
                    },
                    cx,
                );
                workspace.navigate(Route::Playlist, cx);
            })
        });
    };
    let answer = |fixture: &mut Fixture| {
        let BackendCommand::LoadPlaylist {
            offset, respond, ..
        } = fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("playlist request")
        };
        assert_eq!(offset, 0);
        respond
            .send(Ok(model::PlaylistTrackPage {
                source_offsets: vec![],
                tracks: (0..40).map(track).collect(),
                next_offset: None,
                total: 40,
            }))
            .unwrap();
        fixture.update(|_, _| {});
    };
    open(&mut fixture, "a");
    answer(&mut fixture);
    fixture.update(|window, cx| {
        let position = window.find(("spotify-track", 1usize)).bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-850.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
    let before = fixture.update(|window, _| window.find(("spotify-track", 15usize)).bounds());
    open(&mut fixture, "b");
    answer(&mut fixture);
    open(&mut fixture, "a");
    fixture.no_commands();
    fixture
        .update(|window, _| assert_eq!(window.find(("spotify-track", 15usize)).bounds(), before));
    open(&mut fixture, "c");
    answer(&mut fixture);
    open(&mut fixture, "b");
    answer(&mut fixture); // oldest inactive page was evicted
    fixture.no_commands();
}

#[test]
fn recent_album_pages_restore_scroll_without_refetching() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    let open = |fixture: &mut Fixture, id: &str| {
        fixture.update(|_, cx| {
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace.open_album(
                    model::AlbumRef {
                        name: id.to_owned().into(),
                        source_id: Some(id.into()),
                        ..Default::default()
                    },
                    Route::LikedSongs,
                    cx,
                );
            })
        });
    };
    let answer = |fixture: &mut Fixture, id: &str| {
        let BackendCommand::LoadAlbum { respond, .. } =
            fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("album request")
        };
        respond
            .send(Ok((
                model::Album {
                    provider: model::Provider::Spotify,
                    source_id: id.into(),
                    spotify_uri: None,
                    name: id.to_owned().into(),
                    artists: vec![],
                    release_date: None,
                    artwork_url: None,
                    track_count: Some(40),
                },
                (0..40).map(track).collect(),
            )))
            .unwrap();
        fixture.update(|_, _| {});
    };
    open(&mut fixture, "a");
    answer(&mut fixture, "a");
    fixture.update(|window, cx| {
        let position = window.find(("spotify-track", 1usize)).bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-850.))),
                modifiers: Default::default(),
                touch_phase: gpui_kit::TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    });
    let before = fixture.update(|window, _| window.find(("spotify-track", 15usize)).bounds());
    open(&mut fixture, "b");
    answer(&mut fixture, "b");
    open(&mut fixture, "a");
    fixture.no_commands();
    fixture
        .update(|window, _| assert_eq!(window.find(("spotify-track", 15usize)).bounds(), before));
}

#[test]
fn playlist_play_skips_unavailable_start_pages_without_loading_the_whole_list() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(paged_playlist(), cx);
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![],
            tracks: vec![],
            next_offset: Some(15),
            total: 1000,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.no_commands();
    fixture.update(|window, cx| window.click("playlist-play", cx));
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!(offset, 15);
    respond
        .send(Ok(model::PlaylistTrackPage {
            source_offsets: vec![17],
            tracks: vec![track(2)],
            next_offset: Some(30),
            total: 1000,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    let BackendCommand::PlayPlaylistContext {
        tracks,
        index,
        next_offset,
        ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!((tracks, index, next_offset), (vec![track(2)], 0, Some(30)));
    fixture.no_commands();
}

#[test]
fn discography_cards_fit_the_available_width_at_narrow_and_wide_sizes() {
    for (width, zoom, collapsed) in [(900., 130, false), (1920., 100, false), (1280., 100, true)] {
        let mut fixture = Fixture::at_width(width);
        let workspace = fixture.workspace.clone().unwrap();
        fixture.update(|_, cx| {
            cx.set_reduce_motion(true);
            appearance::Appearance::set_ui_zoom(
                crate::storage::UiZoom::from_percent(zoom).unwrap(),
                cx,
            )
            .unwrap();
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.set_collapsed(collapsed, cx));
                workspace.open_artist(
                    model::ArtistRef {
                        name: "Artist".into(),
                        source_id: Some("artist".into()),
                        spotify_uri: None,
                    },
                    Route::LikedSongs,
                    cx,
                )
            });
        });
        let BackendCommand::LoadArtist { respond, .. } =
            fixture.backend.commands.try_recv().unwrap()
        else {
            panic!()
        };
        respond
            .send(Ok((
                model::Artist {
                    genres: Vec::new(),
                    provider: model::Provider::Spotify,
                    name: "Artist".into(),
                    source_id: "artist".into(),
                    spotify_uri: None,
                    artwork_url: None,
                },
                (0..3).map(track).collect(),
                (0..20)
                    .map(|i| model::Album {
                        provider: model::Provider::Spotify,
                        source_id: format!("album-{i}"),
                        name: format!("Album {i}").into(),
                        spotify_uri: None,
                        artwork_url: None,
                        artists: vec![],
                        release_date: None,
                        track_count: None,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            )))
            .unwrap();
        fixture.update(|_, _| {});
        fixture.cx.advance_clock(Duration::from_secs(1));
        fixture.update(|_, _| {});
        fixture.update(|window, cx| {
            let position = window.find("artist-portrait").bounds().center();
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-800.))),
                    modifiers: Default::default(),
                    touch_phase: gpui_kit::TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
        });
        fixture.update(|window, _| {
            let portrait = window.find("artist-dock-portrait").bounds();
            assert!(
                portrait.left() >= window.find("search-field").bounds().right(),
                "dock must not overlap search at any zoom or sidebar width"
            );
            assert!(
                window.find("artist-dock-name").bounds().right()
                    <= window.find("account").bounds().left()
            );
            let mut found = 0;
            for index in 0usize..20 {
                if let Some(card) = window.try_find(("artist-album", index)) {
                    assert!(
                        card.bounds().right()
                            <= window.viewport_size().width
                                - gpui_kit::Rems(2.).to_pixels(window.rem_size())
                    );
                    found += 1;
                }
            }
            assert!(found >= 2);
            if width > 1800. || collapsed {
                let first = window.find(("artist-album", 0usize)).bounds();
                let fifth = window
                    .find(("artist-album", if collapsed { 5usize } else { 4usize }))
                    .bounds();
                assert_eq!(
                    first.origin.y, fifth.origin.y,
                    "wide layouts must exceed four columns"
                );
            }
        });
        fixture.no_commands();
    }
}

#[test]
fn spotify_likes_confirm_success_and_bound_pending_work() {
    let mut fixture = Fixture::new(false);
    fixture.right_click_track(1);
    fixture.update(|window, cx| window.click(("track-menu-spotify-like", 1usize), cx));
    let BackendCommand::SetSpotifyLiked {
        track: sent,
        liked,
        respond,
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("Spotify write")
    };
    assert_eq!(sent, track(1));
    assert!(!liked);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            assert!(
                library.is_spotify_liked(&track(1)),
                "retain confirmed state while pending"
            );
            library.set_spotify_liked(track(2), false, cx);
            library.revalidate(cx);
        });
    });
    fixture.no_commands();
    respond
        .send(Err(anyhow::anyhow!(
            "Sign in again to grant library editing permission"
        )))
        .unwrap();
    fixture.update(|_, cx| {
        assert!(
            services::AppServices::library(cx)
                .read(cx)
                .is_spotify_liked(&track(1))
        )
    });
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        assert!(
            workspace
                .upgrade()
                .unwrap()
                .read(cx)
                .action_notice
                .is_some()
        )
    });
    fixture.right_click_track(1);
    fixture.update(|window, cx| window.click(("track-menu-spotify-like", 1usize), cx));
    let BackendCommand::SetSpotifyLiked { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    respond.send(Ok(())).unwrap();
    fixture.update(|_, _| {});
    fixture.update(|_, cx| {
        let library = services::AppServices::library(cx);
        let library = library.read(cx);
        assert!(!library.is_spotify_liked(&track(1)));
    });
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.set_spotify_liked(track(1), true, cx)
        })
    });
    let BackendCommand::SetSpotifyLiked { liked, respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert!(liked);
    respond.send(Ok(())).unwrap();
    fixture.update(|_, _| {});
    fixture.update(|_, cx| {
        let library = services::AppServices::library(cx);
        assert_eq!(library.read(cx).liked_tracks()[0], track(1));
        assert_eq!(
            library
                .read(cx)
                .liked_tracks()
                .iter()
                .filter(|t| **t == track(1))
                .count(),
            1
        );
    });
    fixture.no_commands();
}

#[test]
fn signing_out_discards_a_pending_spotify_like_reply() {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.set_spotify_liked(track(9), true, cx)
        })
    });
    let BackendCommand::SetSpotifyLiked { respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| library.clear(cx))
    });
    let _ = respond.send(Ok(()));
    fixture.update(|_, cx| {
        assert!(
            services::AppServices::library(cx)
                .read(cx)
                .liked_tracks()
                .is_empty()
        )
    });
    fixture.no_commands();
}

#[test]
fn collapsed_rail_tooltips_center_on_each_trigger_instead_of_pointer() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace
            .upgrade()
            .unwrap()
            .read(cx)
            .sidebar
            .clone()
            .update(cx, |sidebar, cx| sidebar.set_collapsed(true, cx));
    });
    fixture.cx.advance_clock(Duration::from_secs(1));
    fixture.update(|_, _| {});
    for id in [
        "nav-home",
        "nav-library",
        "nav-recent",
        "nav-playlist",
        "nav-library",
    ] {
        fixture.update(|window, cx| window.hover(id, cx));
        fixture.cx.advance_clock(Duration::from_millis(1));
        fixture.update(|_, _| {});
        fixture.update(|window, _| {
            let trigger = window.find(id).bounds();
            let tooltip = window.find("sidebar-tooltip").bounds();
            assert!(
                (trigger.center().y - tooltip.center().y).abs() <= px(1.),
                "{id}: {trigger:?} vs {tooltip:?}"
            );
            assert!(tooltip.left() > trigger.right());
            assert_eq!(tooltip.size.height, px(32.));
        });
    }
    fixture.no_commands();
}

#[test]
fn track_index_play_is_single_click_and_cover_is_passive() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click(("track-cover", 1usize), cx));
    fixture.no_commands();
    fixture.update(|window, cx| {
        let row = window.find(("spotify-track", 1usize)).bounds();
        let play = window.find(("track-index", 1usize)).bounds();
        assert!((play.center().y - row.center().y).abs() <= px(1.));
        window.hover(("track-index", 1usize), cx);
        window.click(("track-play", 1usize), cx);
    });
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::PlayContext { index: 1, .. }
    ));
    fixture.no_commands();
}

#[test]
fn initial_playlist_failure_replaces_skeletons_and_can_retry_without_scrolling() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.load_playlist(paged_playlist(), cx);
            workspace.navigate(Route::Playlist, cx);
        })
    });
    let BackendCommand::LoadPlaylist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    respond
        .send(Err(anyhow::anyhow!("Spotify denied playlist access")))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.cx.advance_clock(Duration::from_secs(2));
    fixture.update(|window, cx| {
        assert!(window.try_find(("spotify-track", 0usize)).is_none());
        window.click("playlist-retry-page", cx);
    });
    let BackendCommand::LoadPlaylist {
        offset, respond, ..
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!(offset, 0);
    fixture.no_commands();
    respond
        .send(Ok(model::PlaylistTrackPage {
            tracks: (0..15).map(track).collect(),
            source_offsets: vec![],
            total: 1000,
            next_offset: Some(15),
        }))
        .unwrap();
    fixture.update(|window, _| {
        // Response processing is settled by the fixture after this closure.
        let _ = window;
    });
    fixture.update(|window, _| {
        assert!(window.try_find("playlist-retry-page").is_none());
        assert!(window.try_find(("spotify-track", 0usize)).is_some());
    });
    fixture.no_commands();
}

#[test]
fn gallery_shares_one_scroll_area_and_opens_albums_without_prefetching() {
    for width in [900., 1280.] {
        let mut fixture = Fixture::at_width(width);
        fixture.update(|window, cx| {
            window.click("search-input", cx);
            window.input("gla", cx);
            window.press("enter", cx);
        });
        let BackendCommand::SearchCatalog { respond, .. } =
            fixture.backend.commands.try_recv().unwrap()
        else {
            panic!("search request")
        };
        respond.send(Ok(super::gallery_results())).unwrap();
        fixture.update(|_, _| {});
        fixture.no_commands();
        let old_heading = fixture.update(|window, cx| {
            assert!(window.try_find("tab-playlists").is_none());
            assert!(
                window
                    .try_find(("spotify-track", model::SEARCH_TRACK_LIMIT))
                    .is_none()
            );
            assert!(
                window.find("search-songs-heading").bounds().top()
                    < window.find("search-artists-heading").bounds().top()
            );
            assert!(
                window.find("search-artists-heading").bounds().top()
                    < window.find("search-albums-heading").bounds().top()
            );
            let viewport = window.find("search-scroll").bounds();
            for (id, count) in [("search-artist", 3usize), ("search-album", 4usize)] {
                for index in 0..count {
                    let bounds = window.find((id, index)).bounds();
                    assert!(
                        bounds.left() >= viewport.left() && bounds.right() < viewport.right(),
                        "card overflows at width {width}: {bounds:?}"
                    );
                }
            }
            // The artists now sit below the songs: bring them into view first.
            let gap = window.find("search-artists-heading").bounds().top() - viewport.top();
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: viewport.center(),
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), -(gap - px(40.)))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            let before = window.find("search-artists-heading").bounds().top();
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: window.find(("search-artist", 0usize)).bounds().center(),
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-160.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            before
        });
        fixture.update(|window, cx| {
            assert!(
                window.find("search-artists-heading").bounds().top() < old_heading,
                "wheel over artist did not scroll the page"
            );
            window.click(("search-album", 0usize), cx);
        });
        assert_eq!(fixture.requested_album(), "album-0");
        fixture.no_commands();
    }
}

fn discography_page(
    indices: impl IntoIterator<Item = usize>,
    next: Option<u32>,
    total: u32,
) -> model::ArtistAlbumPage {
    model::ArtistAlbumPage {
        albums: indices
            .into_iter()
            .map(|index| model::Album {
                provider: model::Provider::Spotify,
                source_id: format!("paged-album-{index}"),
                name: format!("Release {index}").into(),
                spotify_uri: None,
                artists: vec![],
                release_date: None,
                artwork_url: None,
                track_count: None,
            })
            .collect(),
        next_offset: next,
        total,
    }
}

fn open_discography(fixture: &mut Fixture, id: &str, page: model::ArtistAlbumPage) {
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        cx.set_reduce_motion(true);
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_artist(artist_ref(id, id), Route::LikedSongs, cx);
        });
    });
    let BackendCommand::LoadArtist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("artist request")
    };
    respond
        .send(Ok((
            model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: id.into(),
                name: id.into(),
                spotify_uri: None,
                artwork_url: None,
            },
            (0..5).map(track).collect(),
            page,
        )))
        .unwrap();
    fixture.update(|_, _| {});
}

fn scroll_discography(fixture: &mut Fixture) {
    for _ in 0..4 {
        fixture.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: point(px(700.), px(400.)),
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-3000.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
    }
}

fn requested_discography(
    fixture: &mut Fixture,
    artist: &str,
    expected: u32,
) -> crate::backend::Reply<model::ArtistAlbumPage> {
    let BackendCommand::LoadArtistAlbums {
        source_id,
        offset,
        respond,
    } = fixture
        .backend
        .commands
        .try_recv()
        .expect("next discography page")
    else {
        panic!("discography request")
    };
    assert_eq!(source_id, artist);
    assert_eq!(offset, expected);
    respond
}

#[test]
fn discography_loads_only_at_the_visible_footer_and_appends_in_order_without_duplicates_or_scroll_jumps()
 {
    let mut fixture = Fixture::new(false);
    open_discography(
        &mut fixture,
        "large-artist",
        discography_page(0..10, Some(10), 26),
    );
    fixture.no_commands();
    scroll_discography(&mut fixture);
    let reply = requested_discography(&mut fixture, "large-artist", 10);
    // Repaints and repeated wheel input while loading keep one request in flight.
    scroll_discography(&mut fixture);
    fixture.no_commands();
    reply
        .send(Ok(discography_page(
            std::iter::once(9).chain(10..19),
            Some(20),
            26,
        )))
        .unwrap();
    fixture.update(|_, _| {});
    // The appended rows extend below the viewport rather than snapping to their footer.
    fixture.no_commands();
    scroll_discography(&mut fixture);
    let reply = requested_discography(&mut fixture, "large-artist", 20);
    reply.send(Ok(discography_page(19..25, None, 26))).unwrap();
    fixture.update(|_, _| {});
    scroll_discography(&mut fixture);
    fixture.no_commands();
    fixture.update(|window, cx| {
        assert!(
            window.try_find("artist-discography-footer").is_none(),
            "completed catalog kept a load-more footer"
        );
        window.click(("artist-album", 24usize), cx);
    });
    // A repeated ID across the page boundary did not shift the final card's identity.
    assert_eq!(fixture.requested_album(), "paged-album-24");
    fixture.no_commands();
}

#[test]
fn discography_failure_keeps_loaded_albums_and_retries_only_on_request() {
    let mut fixture = Fixture::new(false);
    open_discography(
        &mut fixture,
        "retry-artist",
        discography_page(0..10, Some(10), 20),
    );
    scroll_discography(&mut fixture);
    requested_discography(&mut fixture, "retry-artist", 10)
        .send(Err(anyhow::anyhow!("temporary Spotify failure")))
        .unwrap();
    fixture.update(|_, _| {});
    scroll_discography(&mut fixture);
    fixture.no_commands();
    fixture.update(|window, cx| window.click("artist-load-more", cx));
    requested_discography(&mut fixture, "retry-artist", 10)
        .send(Ok(discography_page(10..20, None, 20)))
        .unwrap();
    fixture.update(|_, _| {});
    scroll_discography(&mut fixture);
    fixture.no_commands();
    fixture.update(|window, cx| window.click(("artist-album", 19usize), cx));
    assert_eq!(fixture.requested_album(), "paged-album-19");
}

#[test]
fn switching_artists_cancels_a_pending_discography_page() {
    let mut fixture = Fixture::new(false);
    open_discography(
        &mut fixture,
        "first-artist",
        discography_page(0..10, Some(10), 100),
    );
    scroll_discography(&mut fixture);
    let old = requested_discography(&mut fixture, "first-artist", 10);
    open_discography(
        &mut fixture,
        "second-artist",
        discography_page(100..103, None, 3),
    );
    assert!(
        old.is_closed(),
        "abandoned page receiver should cancel its backend fetch"
    );
    assert!(old.send(Ok(discography_page(10..20, None, 20))).is_err());
    fixture.update(|_, _| {});
    fixture.no_commands();
    scroll_discography(&mut fixture);
    fixture.update(|window, cx| window.click(("artist-album", 2usize), cx));
    assert_eq!(fixture.requested_album(), "paged-album-102");
}

#[test]
fn duplicate_only_discography_pages_require_manual_continuation_instead_of_auto_draining_the_catalog()
 {
    let mut fixture = Fixture::new(false);
    open_discography(
        &mut fixture,
        "duplicates",
        discography_page(0..10, Some(10), 30),
    );
    scroll_discography(&mut fixture);
    requested_discography(&mut fixture, "duplicates", 10)
        .send(Ok(discography_page(0..10, Some(20), 30)))
        .unwrap();
    fixture.update(|_, _| {});
    scroll_discography(&mut fixture);
    fixture.no_commands();
    fixture.update(|window, cx| window.click("artist-load-more", cx));
    requested_discography(&mut fixture, "duplicates", 20)
        .send(Ok(discography_page(10..20, None, 30)))
        .unwrap();
    fixture.update(|_, _| {});
    scroll_discography(&mut fixture);
    fixture.no_commands();
}

#[test]
fn playback_clock_updates_reuse_metadata_and_shared_track_allocation() {
    let mut fixture = Fixture::new(false);
    let current = track(0);
    let uri = current.spotify_uri.clone().unwrap();
    fixture.play(current);
    let workspace = fixture.workspace.clone().unwrap();
    let count = |fixture: &mut Fixture| {
        fixture.update(|_, cx| {
            workspace
                .upgrade()
                .unwrap()
                .read(cx)
                .player_bar
                .read(cx)
                .metadata_render_count(cx)
        })
    };
    let before = count(&mut fixture);
    let allocation = fixture.update(|_, cx| {
        services::AppServices::player(cx)
            .read(cx)
            .shared_now_playing()
            .unwrap()
    });
    for position in 1..=100 {
        fixture.player_event(BackendEvent::PositionChanged {
            spotify_uri: uri.clone(),
            position_ms: position * 10,
        });
    }
    let after = count(&mut fixture);
    assert_eq!(
        after, before,
        "progress updates rebuilt the track metadata subtree"
    );
    fixture.update(|_, cx| {
        assert!(Arc::ptr_eq(
            &allocation,
            &services::AppServices::player(cx)
                .read(cx)
                .shared_now_playing()
                .unwrap()
        ))
    });
    fixture.play(track(2));
    assert_eq!(
        count(&mut fixture),
        before + 1,
        "new tracks must refresh cached labels"
    );
}

#[test]
fn artwork_cleanup_invalidates_cached_player_metadata_once_per_burst() {
    let mut fixture = Fixture::new(false);
    fixture.play(track(0));
    let workspace = fixture.workspace.clone().unwrap();
    let count = |fixture: &mut Fixture| {
        fixture.update(|_, cx| {
            workspace
                .upgrade()
                .unwrap()
                .read(cx)
                .player_bar
                .read(cx)
                .metadata_render_count(cx)
        })
    };
    let before = count(&mut fixture);
    fixture.update(|window, cx| {
        services::AppServices::image_cache(cx).update(cx, |cache, cx| {
            cache.clear(window, cx);
            cache.clear(window, cx);
        });
    });
    assert_eq!(
        count(&mut fixture),
        before + 1,
        "cached scene was not refreshed once after artwork cleanup"
    );
    fixture.update(|_, _| {});
    assert_eq!(
        count(&mut fixture),
        before + 1,
        "cleanup kept invalidating while idle"
    );
}

#[test]
fn settings_tabs_show_one_group_at_a_time() {
    let mut fixture = Fixture::new(true);
    // General is the default and holds appearance and playback.
    fixture.update(|window, _| {
        assert!(window.try_find("settings-autoplay").is_some());
        assert!(window.try_find("settings-clear-cache").is_none());
        assert!(window.try_find("settings-lastfm-connect").is_none());
    });
    fixture.update(|window, cx| window.click(("settings-tab", 1usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("settings-lastfm-connect").is_some());
        assert!(window.try_find("settings-autoplay").is_none());
    });
    fixture.update(|window, cx| window.click(("settings-tab", 2usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("equalizer-graph").is_some());
        assert!(window.try_find("settings-clear-cache").is_none());
    });
    fixture.update(|window, cx| window.click(("settings-tab", 3usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("settings-voice-enabled").is_some());
        assert!(window.try_find("equalizer-graph").is_none());
    });
    fixture.update(|window, cx| window.click(("settings-tab", 4usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("settings-clear-cache").is_some());
        assert!(window.try_find("equalizer-graph").is_none());
        assert!(window.try_find("settings-lastfm-connect").is_none());
    });
    fixture.update(|window, cx| window.click(("settings-tab", 0usize), cx));
    fixture.update(|window, _| assert!(window.try_find("settings-autoplay").is_some()));
    fixture.no_commands();
}

fn release_album() -> model::Album {
    model::Album {
        provider: model::Provider::Spotify,
        source_id: "release-1".into(),
        spotify_uri: Some("spotify:album:release-1".into()),
        name: "Fresh Release".into(),
        artists: vec![model::ArtistRef {
            name: "Top Artist".into(),
            source_id: Some("artist-1".into()),
            spotify_uri: None,
        }],
        release_date: Some("2026-11-07".into()),
        artwork_url: None,
        track_count: None,
    }
}

/// Answers the new-releases request Home makes alongside its top items, and
/// whatever else it asked for along with it.
fn answer_new_releases(fixture: &mut Fixture, result: anyhow::Result<Vec<model::Album>>) {
    let BackendCommand::LoadNewReleases { respond } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a new-releases request")
    };
    respond.send(result).unwrap();
    fixture.update(|_, _| {});
    // Home also asks for Made for you; nothing here depends on it.
    while let Ok(command) = fixture.backend.commands.try_recv() {
        match command {
            BackendCommand::LoadMadeForYou { respond, .. } => {
                respond.send(Err(anyhow::anyhow!("not connected"))).unwrap();
            }
            BackendCommand::LoadHome { respond, .. } => {
                respond.send(Ok(model::HomeData::default())).unwrap();
            }
            _ => {}
        }
        fixture.update(|_, _| {});
    }
}

#[test]
fn home_loads_top_items_once_when_opened_and_lists_no_top_artists() {
    let mut fixture = Fixture::new(false);
    fixture.no_commands();
    fixture.update(|window, cx| window.click("nav-home", cx));
    let BackendCommand::LoadHome { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a Home request")
    };
    respond
        .send(Ok(model::HomeData {
            artists: vec![model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "artist-1".into(),
                spotify_uri: Some("spotify:artist:artist-1".into()),
                name: "Top Artist".into(),
                artwork_url: None,
            }],
            tracks: vec![credited_track()],
        }))
        .unwrap();
    answer_new_releases(&mut fixture, Ok(vec![release_album()]));
    fixture.update(|_, _| {});
    // Leaving and returning within the freshness window must not refetch.
    fixture.update(|window, cx| window.click("nav-recent", cx));
    fixture.update(|window, cx| window.click("nav-home", cx));
    fixture.no_commands();
    // Home lists top tracks only; the artists it knows feed the rows above.
    fixture.update(|window, _| {
        assert!(window.try_find("home-artists-heading").is_none());
        assert!(window.try_find(("home-artist", 0usize)).is_none());
        assert!(window.try_find("home-tracks-heading").is_none());
    });
}

#[test]
fn an_album_queues_whole_after_the_current_song() {
    let mut fixture = Fixture::new(false);
    fixture.play(credited_track());
    fixture.no_commands();
    fixture.update(|window, cx| window.click("player-title", cx));
    let BackendCommand::LoadAlbum { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected an album request")
    };
    let album = crate::app::test_support::gallery_results().albums.remove(0);
    respond
        .send(Ok((album, vec![track(0), track(1), track(2)])))
        .unwrap();
    fixture.update(|_, _| {});

    fixture.update(|window, cx| window.click("album-queue", cx));
    let BackendCommand::PlayNextMany(tracks) = fixture.backend.commands.try_recv().unwrap() else {
        panic!("expected the album to be queued")
    };
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].source_id, track(0).source_id);
    assert_eq!(tracks[2].source_id, track(2).source_id);
}

#[test]
fn home_lists_new_releases_from_top_artists_and_opens_one() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("nav-home", cx));
    let BackendCommand::LoadHome { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a Home request")
    };
    respond
        .send(Ok(model::HomeData {
            artists: vec![],
            tracks: vec![credited_track()],
        }))
        .unwrap();
    // While the releases and the artist to try load, the page holds its place.
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find("home-releases-loading").is_some());
        assert!(window.try_find(("home-release", 0usize)).is_none());
    });
    answer_new_releases(&mut fixture, Ok(vec![release_album()]));
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find("home-releases-loading").is_none());
        assert!(window.try_find("home-releases-heading").is_some());
        assert!(window.try_find(("home-release", 0usize)).is_some());
    });
    fixture.update(|window, cx| window.click(("home-release", 0usize), cx));
    assert_eq!(fixture.route(), Route::Album);
}

#[test]
fn home_opens_with_shortcuts_back_into_the_library() {
    let mut fixture = Fixture::new(false);
    fixture.play(track(0));
    fixture.update(|window, cx| window.click("nav-home", cx));
    let BackendCommand::LoadHome { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a Home request")
    };
    respond.send(Ok(model::HomeData::default())).unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find("home-shortcuts").is_some());
        // Liked Songs and Recently played always lead.
        assert!(window.try_find(("home-shortcut", 0usize)).is_some());
        assert!(window.try_find(("home-shortcut", 1usize)).is_some());
    });

    // The play button on a tile starts its songs without leaving Home.
    fixture.update(|window, cx| window.hover(("home-shortcut", 0usize), cx));
    fixture.update(|window, cx| window.click(("home-shortcut-play", 0usize), cx));
    let mut started = false;
    while let Ok(command) = fixture.backend.commands.try_recv() {
        started |= matches!(command, BackendCommand::PlayContext { index: 0, .. });
    }
    assert!(started, "the tile's songs start");
    assert_eq!(fixture.route(), Route::Home);

    fixture.update(|window, cx| window.click(("home-shortcut", 0usize), cx));
    assert_eq!(fixture.route(), Route::LikedSongs);
    fixture.update(|window, cx| window.click("nav-home", cx));
    fixture.update(|window, cx| window.click(("home-shortcut", 1usize), cx));
    assert_eq!(fixture.route(), Route::Recent);
}

#[test]
fn home_asks_for_a_new_sign_in_when_the_permission_is_missing() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("nav-home", cx));
    let BackendCommand::LoadHome { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a Home request")
    };
    respond
        .send(Err(model::HomePermissionMissing.into()))
        .unwrap();
    answer_new_releases(&mut fixture, Err(model::HomePermissionMissing.into()));
    fixture.update(|_, _| {});
    fixture.update(|window, _| assert!(window.try_find("home-sign-in-again").is_some()));
    // Revisiting does not hammer a request that cannot succeed.
    fixture.update(|window, cx| window.click("nav-recent", cx));
    fixture.update(|window, cx| window.click("nav-home", cx));
    fixture.no_commands();
}

#[test]
fn track_menu_copies_the_spotify_link() {
    let mut fixture = Fixture::new(false);
    fixture.right_click_track(0);
    fixture.update(|window, cx| window.click(("track-menu-copy-link", 0usize), cx));
    fixture.update(|_, cx| {
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        let expected = format!("https://open.spotify.com/track/{}", track(0).source_id);
        assert_eq!(copied.as_deref(), Some(expected.as_str()));
    });
    fixture.no_commands();
}

fn local_track(title: &str) -> model::Track {
    model::Track {
        provider: model::Provider::Local,
        source_id: format!("C:/Music/{title}.flac"),
        spotify_uri: Some(crate::local_files::local_uri(
            "Ana Lind", "Rooms", title, 200_000,
        )),
        isrc: None,
        title: title.to_owned().into(),
        artist: "Ana Lind".into(),
        artists: vec![model::ArtistRef {
            name: "Ana Lind".into(),
            source_id: None,
            spotify_uri: None,
        }],
        album: "Rooms".into(),
        album_ref: None,
        duration_ms: 200_000,
        artwork_url: None,
    }
}

/// Answers the next Local Files request, returning whether it was a rescan.
fn answer_local_files(fixture: &mut Fixture, tracks: Vec<model::Track>) -> bool {
    let BackendCommand::LoadLocalFiles {
        rescan, respond, ..
    } = fixture
        .backend
        .commands
        .try_recv()
        .expect("local files request")
    else {
        panic!("expected a Local Files request")
    };
    respond
        .send(Ok(crate::local_files::LocalLibrary {
            tracks,
            unreadable: 0,
            truncated: false,
        }))
        .unwrap();
    fixture.update(|_, _| {});
    rescan
}

#[test]
fn local_files_join_the_sidebar_once_a_folder_is_added_and_play_from_disk() {
    let mut fixture = Fixture::new(false);
    // Nothing changes, and nothing is scanned, for a listener without folders.
    fixture.update(|window, _| assert!(window.try_find("nav-local-files").is_none()));
    fixture.no_commands();

    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.open_local_files(vec!["C:/Music".into()], cx)
        });
    });
    // The saved index paints first, then the rescan replaces it.
    assert!(!answer_local_files(
        &mut fixture,
        vec![local_track("Harbour")]
    ));
    assert!(answer_local_files(
        &mut fixture,
        vec![local_track("Harbour"), local_track("Tides")]
    ));

    fixture.update(|window, cx| window.click("nav-local-files", cx));
    assert_eq!(fixture.route(), Route::LocalFiles);
    // The scan that just finished is fresh enough; opening asks for nothing.
    fixture.no_commands();

    fixture.right_click_track(1);
    fixture.update(|window, _| {
        assert!(window.try_find(("track-menu-reveal", 1usize)).is_some());
        for spotify_only in [
            "track-menu-radio",
            "track-menu-credits",
            "track-menu-copy-link",
            "track-menu-spotify",
            "track-menu-spotify-like",
        ] {
            assert!(
                window.try_find((spotify_only, 1usize)).is_none(),
                "{spotify_only} is shown for a local file"
            );
        }
    });
    fixture.update(|window, cx| window.click(("track-menu-play", 1usize), cx));
    let BackendCommand::PlayContext { tracks, index } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected local playback")
    };
    assert_eq!(index, 1);
    assert_eq!(tracks.len(), 2);
    assert!(tracks.iter().all(model::Track::is_local));
    assert_eq!(tracks[1].source_id, "C:/Music/Tides.flac");
}

#[test]
fn removing_the_last_local_folder_hides_local_files() {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.open_local_files(vec!["C:/Music".into()], cx)
        });
    });
    answer_local_files(&mut fixture, vec![local_track("Harbour")]);
    answer_local_files(&mut fixture, vec![local_track("Harbour")]);
    fixture.update(|window, _| assert!(window.try_find("nav-local-files").is_some()));

    let error = fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.remove_local_folder(std::path::Path::new("C:/Music"), cx)
        })
    });
    assert!(error.is_none());
    fixture.update(|window, cx| {
        assert!(window.try_find("nav-local-files").is_none());
        let library = services::AppServices::library(cx).read(cx);
        assert!(library.local_tracks().is_empty());
    });
    // The rescan with no folders empties the saved index too.
    assert!(answer_local_files(&mut fixture, Vec::new()));
}

#[test]
fn local_files_search_narrows_the_list_and_escape_clears_it() {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.open_local_files(vec!["C:/Music".into()], cx)
        });
    });
    let mut night_bus = local_track("Night Bus");
    night_bus.artist = "Ostrava Tapes".into();
    let tracks = vec![local_track("Harbour"), local_track("Tides"), night_bus];
    answer_local_files(&mut fixture, tracks.clone());
    answer_local_files(&mut fixture, tracks);
    fixture.update(|window, cx| window.click("nav-local-files", cx));
    fixture.no_commands();

    // Every word must match, in any of title, artist or album, in any case.
    fixture.update(|window, cx| {
        window.click("page-search-input", cx);
        window.input("ostrava BUS", cx);
    });
    fixture.update(|window, _| {
        assert!(window.try_find(("spotify-track", 0usize)).is_some());
        assert!(window.try_find(("spotify-track", 1usize)).is_none());
    });
    // Playing from the results plays the results.
    fixture.right_click_track(0);
    fixture.update(|window, cx| window.click(("track-menu-play", 0usize), cx));
    let BackendCommand::PlayContext { tracks, index } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected local playback")
    };
    assert_eq!(index, 0);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title.as_ref(), "Night Bus");

    // A search that finds nothing keeps its field, so it can be cleared.
    fixture.update(|window, cx| {
        window.click("page-search-input", cx);
        window.input(" zz", cx);
    });
    fixture.update(|window, _| {
        assert!(window.try_find(("spotify-track", 0usize)).is_none());
        assert!(window.try_find("page-search-field").is_some());
    });
    fixture.update(|window, cx| window.press("escape", cx));
    fixture.update(|window, _| {
        assert_eq!(window.find("page-search-input").value(), Some(""));
        assert!(window.try_find(("spotify-track", 2usize)).is_some());
    });
    // Search never asks the backend for anything.
    fixture.no_commands();
}

/// A fixture sitting on the Local Files page, with one song and a folder.
fn on_local_files() -> Fixture {
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        services::AppServices::library(cx).update(cx, |library, cx| {
            library.open_local_files(vec!["C:/Music".into()], cx)
        });
    });
    answer_local_files(&mut fixture, vec![local_track("Harbour")]);
    answer_local_files(&mut fixture, vec![local_track("Harbour")]);
    fixture.update(|window, cx| window.click("nav-local-files", cx));
    fixture.no_commands();
    fixture
}

#[test]
fn the_link_dialog_opens_from_its_button_and_closes_on_escape_its_close_button_or_the_dimmed_area()
{
    let mut fixture = on_local_files();
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_none()));

    fixture.update(|window, cx| window.click("local-files-link", cx));
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_some()));
    // Escape closes it and nothing else: the page stays.
    fixture.update(|window, cx| window.press("escape", cx));
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_none()));
    assert_eq!(fixture.route(), Route::LocalFiles);

    // Its close button closes it.
    fixture.update(|window, cx| window.click("local-files-link", cx));
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_some()));
    fixture.update(|window, cx| window.click("link-close", cx));
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_none()));

    // So does a press on the dimmed area around it.
    fixture.update(|window, cx| window.click("local-files-link", cx));
    fixture.update(|window, cx| window.click_at("link-scrim", point(px(12.), px(12.)), cx));
    fixture.update(|window, _| assert!(window.try_find("link-popover").is_none()));
    // Opening and closing never asked the backend for anything.
    fixture.no_commands();
}

#[test]
fn a_looked_up_link_offers_formats_and_a_download_and_text_without_a_link_does_not() {
    let mut fixture = on_local_files();
    fixture.update(|window, cx| window.click("local-files-link", cx));
    let url = "https://example.com/ana-lind/harbour-lights-live";
    fixture.update(|_, cx| {
        services::AppServices::link_downloads(cx).update(cx, |downloads, _| {
            downloads.fixture_probe(
                url,
                crate::app::link_downloads::ProbeState::Found(crate::link_download::Probe {
                    title: "Harbour Lights (Live)".into(),
                    artist: Some("Ana Lind".into()),
                    duration_secs: Some(221.),
                    site: "example.com".into(),
                    ..Default::default()
                }),
            )
        });
    });
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    let popover = fixture.update(|_, cx| {
        workspace
            .upgrade()
            .expect("workspace")
            .read(cx)
            .local_files
            .read(cx)
            .link_popover()
            .expect("open")
    });

    // Words that are not a link find nothing to save.
    fixture.update(|window, cx| {
        popover.update(cx, |popover, cx| {
            popover.fixture_type("harbour lights", window, cx)
        })
    });
    fixture.update(|window, _| {
        assert!(window.try_find("link-save").is_none());
        assert!(window.try_find("link-save-disabled").is_some());
        assert!(window.try_find(("link-format", 0usize)).is_none());
    });

    fixture.update(|window, cx| {
        popover.update(cx, |popover, cx| popover.fixture_type(url, window, cx))
    });
    fixture.update(|window, _| {
        assert!(window.try_find("link-save").is_some());
        assert!(window.try_find("link-save-disabled").is_none());
    });
    // A format chip chooses that format, and the choice stays.
    fixture.update(|window, cx| {
        window.click(
            ("link-format", crate::link_download::Format::Flac as usize),
            cx,
        )
    });
    fixture.update(|_, cx| {
        let downloads = services::AppServices::link_downloads(cx);
        assert_eq!(
            downloads.read(cx).format(),
            crate::link_download::Format::Flac
        );
    });
    fixture.no_commands();
}

#[test]
fn a_folder_change_during_a_scan_rescans_once_it_finishes() {
    let mut fixture = Fixture::new(false);
    let library = fixture.update(|_, cx| services::AppServices::library(cx));
    fixture.update(|_, cx| {
        library.update(cx, |library, cx| {
            library.open_local_files(vec!["C:/Music".into()], cx)
        })
    });
    // The saved index arrives; the rescan it starts is still running when
    // the folder changes, so the change waits instead of starting another.
    assert!(!answer_local_files(
        &mut fixture,
        vec![local_track("Harbour")]
    ));
    fixture.update(|_, cx| library.update(cx, |library, cx| library.rescan_after_change(cx)));
    fixture.update(|_, cx| library.update(cx, |library, cx| library.rescan_after_change(cx)));
    assert!(answer_local_files(
        &mut fixture,
        vec![local_track("Harbour")]
    ));
    // Both changes are covered by one more rescan.
    assert!(answer_local_files(
        &mut fixture,
        vec![local_track("Harbour"), local_track("Tides")]
    ));
    fixture.no_commands();

    // With nothing running, a change rescans straight away.
    fixture.update(|_, cx| library.update(cx, |library, cx| library.rescan_after_change(cx)));
    assert!(answer_local_files(
        &mut fixture,
        vec![local_track("Harbour")]
    ));
    fixture.no_commands();
}

fn synced_fixture_lyrics() -> crate::lyrics::Lyrics {
    crate::lyrics::parse_synced("[00:01.00]First line\n[00:05.00]Second line\n[00:09.00]Third line")
}

#[test]
fn the_lyrics_button_is_always_there_and_asks_nothing_until_opened() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.update(|window, _| assert!(window.try_find("lyrics-toggle").is_some()));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, _| {
        assert!(window.try_find(("settings-lyrics-view", 0usize)).is_some());
        assert!(window.try_find("settings-lyrics").is_none());
    });
    fixture.no_commands();
}

#[test]
fn opening_lyrics_asks_once_shows_the_lines_and_a_click_seeks() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    let BackendCommand::LoadLyrics { query, respond } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a lyrics lookup")
    };
    assert_eq!(query.title, "Song 1");
    assert_eq!(query.artist, "Sonder Ensemble");
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-panel").is_some());
        assert!(window.try_find(("lyric-line", 0usize)).is_none());
    });
    respond.send(Ok(synced_fixture_lyrics())).unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, cx| {
        assert!(window.try_find(("lyric-line", 0usize)).is_some());
        assert!(window.try_find(("lyric-line", 2usize)).is_some());
        window.click(("lyric-line", 2usize), cx);
    });
    let BackendCommand::Seek(position) = fixture.backend.commands.try_recv().unwrap() else {
        panic!("expected a seek")
    };
    assert_eq!(position, 9_000);
    // Closing and reopening for the same song does not ask again.
    fixture.update(|window, cx| window.click("close-lyrics", cx));
    fixture.update(|window, _| assert!(window.try_find("lyrics-panel").is_none()));
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|window, _| assert!(window.try_find(("lyric-line", 0usize)).is_some()));
    fixture.no_commands();
}

#[test]
fn songs_without_lyrics_say_so_and_the_choice_of_view_changes_the_place() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click(("settings-lyrics-view", 2usize), cx));
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).lyrics_view,
            LyricsView::Strip
        );
    });
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    let BackendCommand::LoadLyrics { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a lyrics lookup")
    };
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-strip").is_some());
        assert!(window.try_find("lyrics-panel").is_none());
    });
    respond.send(Ok(crate::lyrics::Lyrics::default())).unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, _| assert!(window.try_find("lyrics-strip").is_some()));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click(("settings-lyrics-view", 1usize), cx));
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-page").is_some());
        assert!(window.try_find("lyrics-strip").is_none());
    });
}

#[test]
fn the_cover_opens_the_now_playing_page_and_closes_it_again() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.update(|window, _| assert!(window.try_find("lyrics-page").is_none()));
    fixture.update(|window, cx| window.click("player-artwork", cx));
    fixture.update(|window, _| {
        // Whatever place lyrics usually open in, the cover shows the page.
        assert!(window.try_find("lyrics-page").is_some());
        assert!(window.try_find("lyrics-panel").is_none());
        assert!(window.try_find("now-playing-cover").is_some());
    });
    fixture.update(|window, cx| window.click("player-artwork", cx));
    fixture.update(|window, _| assert!(window.try_find("lyrics-page").is_none()));
    // The lyrics button still opens them where the listener put them.
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-panel").is_some());
        assert!(window.try_find("lyrics-page").is_none());
    });
}

#[test]
fn the_lyrics_panel_and_the_queue_share_the_right_edge() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|window, _| assert!(window.try_find("lyrics-panel").is_some()));
    fixture.update(|window, cx| window.click("queue-toggle", cx));
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-panel").is_none());
        assert!(window.try_find("close-queue").is_some());
    });
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|window, _| {
        assert!(window.try_find("lyrics-panel").is_some());
        assert!(window.try_find("close-queue").is_none());
    });
}

impl Fixture {
    fn lyrics_scroll(&mut self) -> (f32, bool) {
        let workspace = self.workspace.clone().expect("workspace fixture");
        self.update(|_, cx| {
            let workspace = workspace.upgrade().expect("live workspace");
            let lyrics = workspace.read(cx).lyrics.clone();
            let lyrics = lyrics.read(cx);
            (lyrics.scroll_offset_y(), lyrics.gliding())
        })
    }
}

#[test]
fn moving_to_the_next_line_glides_and_then_goes_quiet() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    let BackendCommand::LoadLyrics { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a lyrics lookup")
    };
    respond.send(Ok(synced_fixture_lyrics())).unwrap();
    fixture.cx.run_until_parked();
    // Let the first placement settle.
    for _ in 0..6 {
        fixture.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        fixture.cx.advance_clock(Duration::from_millis(20));
    }
    // Playback reaches the first line, and the view settles on it.
    fixture.update(|window, cx| window.click(("lyric-line", 0usize), cx));
    let _ = fixture.backend.commands.try_recv();
    for _ in 0..90 {
        fixture.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        fixture.cx.advance_clock(Duration::from_millis(16));
    }
    let (resting, gliding) = fixture.lyrics_scroll();
    assert!(!gliding, "the first placement is done");
    // Playback reaches the second line.
    fixture.update(|window, cx| window.click(("lyric-line", 1usize), cx));
    let _ = fixture.backend.commands.try_recv();
    let mut offsets = vec![resting];
    // Where the new current line sits on screen: it should only rise to its place.
    let mut tops = Vec::new();
    let mut frames_with_work = 0;
    for _ in 0..90 {
        let work = fixture.update(|window, cx| window.simulate_next_frame(cx));
        frames_with_work += usize::from(work > 0);
        fixture.cx.advance_clock(Duration::from_millis(16));
        offsets.push(fixture.lyrics_scroll().0);
        tops.push(
            fixture
                .update(|window, _| f32::from(window.find(("lyric-line", 1usize)).bounds().top())),
        );
    }
    let (last, gliding) = fixture.lyrics_scroll();
    assert!(!gliding, "the glide finished");
    assert!(frames_with_work > 3, "the glide spans several frames");
    assert!(
        frames_with_work < 90,
        "and then stops asking for frames: {frames_with_work}"
    );
    // The new line only ever rises to its place (the column's own offset may
    // wobble a little as the lines above it shrink), and the column ends
    // somewhere new.
    assert!(
        tops.windows(2).all(|pair| pair[1] <= pair[0] + 0.5),
        "one direction: {tops:?}"
    );
    assert!((last - resting).abs() > 1., "it moved: {offsets:?}");
}

#[test]
fn a_line_arriving_mid_glide_carries_on_without_stepping_back() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    let BackendCommand::LoadLyrics { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a lyrics lookup")
    };
    respond.send(Ok(synced_fixture_lyrics())).unwrap();
    fixture.cx.run_until_parked();
    let frame = |fixture: &mut Fixture| {
        fixture.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        fixture.cx.advance_clock(Duration::from_millis(16));
    };
    for _ in 0..6 {
        frame(&mut fixture);
    }
    fixture.update(|window, cx| window.click(("lyric-line", 0usize), cx));
    let _ = fixture.backend.commands.try_recv();
    for _ in 0..90 {
        frame(&mut fixture);
    }
    // The next line arrives, and the one after it a few frames later, while the
    // column is still moving.
    fixture.update(|window, cx| window.click(("lyric-line", 1usize), cx));
    let _ = fixture.backend.commands.try_recv();
    for _ in 0..5 {
        frame(&mut fixture);
    }
    fixture.update(|window, cx| window.click(("lyric-line", 2usize), cx));
    let _ = fixture.backend.commands.try_recv();
    let mut tops = Vec::new();
    for _ in 0..90 {
        frame(&mut fixture);
        tops.push(
            fixture
                .update(|window, _| f32::from(window.find(("lyric-line", 2usize)).bounds().top())),
        );
    }
    assert!(
        tops.windows(2).all(|pair| pair[1] <= pair[0] + 0.5),
        "the third line only rises to its place: {tops:?}"
    );
    assert!(!fixture.lyrics_scroll().1, "and then it goes quiet");
}

#[test]
fn a_reopened_window_comes_back_with_lyrics_open() {
    let mut fixture = Fixture::new(true);
    fixture.play(track(0));
    fixture.scroll_settings_to_the_end();
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|_, cx| {
        assert!(services::AppServices::lyrics_open(cx));
        // A window made after the last one closed starts from that choice.
        let bar = cx.new(crate::app::player_bar::PlayerBar::new);
        assert!(bar.read(cx).lyrics_open());
    });
    fixture.update(|window, cx| window.click("lyrics-toggle", cx));
    fixture.update(|_, cx| {
        assert!(!services::AppServices::lyrics_open(cx));
        let bar = cx.new(crate::app::player_bar::PlayerBar::new);
        assert!(!bar.read(cx).lyrics_open());
    });
}

/// Delivers an event the way the backend does: the player first, then the
/// window for whatever it leaves.
fn deliver(fixture: &mut Fixture, event: BackendEvent) {
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        let left = services::AppServices::player(cx)
            .update(cx, |player, cx| player.handle_backend_event(event, cx));
        if let Some(event) = left {
            workspace.upgrade().unwrap().update(cx, |workspace, cx| {
                workspace.handle_backend_events(vec![event], cx)
            });
        }
    });
}

fn refused(index: usize) -> BackendEvent {
    BackendEvent::TrackFailed {
        spotify_uri: format!("spotify:track:track-{index}"),
        error: "Spotify wouldn’t unlock this song".into(),
    }
}

#[test]
fn an_account_spotify_wont_stream_to_stops_and_says_why() {
    let mut fixture = Fixture::new(false);
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: vec![track(4)],
    });
    // The first refused song only skips; one song proves nothing.
    deliver(&mut fixture, refused(3));
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Next)
    ));
    fixture.update(|window, _| assert!(window.try_find("streaming-refused").is_none()));

    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(4),
        next: Vec::new(),
    });
    deliver(&mut fixture, BackendEvent::StreamingRefused);
    // The song it was opening stops instead of decoding noise.
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Pause)
    ));
    fixture.update(|window, cx| {
        assert!(window.try_find("streaming-refused").is_some());
        let player = services::AppServices::player(cx).read(cx);
        assert!(!player.playing());
        assert_eq!(
            player.error().map(|error| error.as_ref()),
            Some("Spotify won’t stream to this account")
        );
    });
    // A late failure for that song neither skips on nor adds a second error.
    deliver(&mut fixture, refused(4));
    fixture.no_commands();

    fixture.update(|window, cx| window.click("streaming-refused-later", cx));
    fixture.update(|window, _| assert!(window.try_find("streaming-refused").is_none()));
    fixture.no_commands();

    // The next try shows it again at once, and Escape closes it.
    deliver(&mut fixture, BackendEvent::StreamingRefused);
    fixture.update(|window, _| assert!(window.try_find("streaming-refused").is_some()));
    fixture.press("escape");
    fixture.update(|window, _| assert!(window.try_find("streaming-refused").is_none()));
}

/// Puts the process-wide choice of player back, so one test's setup never
/// leaks into the next.
struct RestoreEngine;

impl Drop for RestoreEngine {
    fn drop(&mut self) {
        crate::playback::set_engine(crate::playback::Engine::BuiltIn);
    }
}

fn setup_command(fixture: &mut Fixture) -> Option<String> {
    match fixture.backend.commands.try_recv() {
        Ok(BackendCommand::SetUpSoloist { api_key }) => api_key.or(Some(String::new())),
        other => panic!(
            "expected a Soloist setup, got {:?}",
            other.map(|_| "a command")
        ),
    }
}

#[test]
fn a_refused_account_can_set_up_spotifys_own_player() {
    if !crate::soloist::supported() {
        return;
    }
    let _restore = RestoreEngine;
    let mut fixture = Fixture::new(false);
    deliver(&mut fixture, BackendEvent::StreamingRefused);
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Pause)
    ));
    fixture.update(|window, cx| window.click("streaming-refused-soloist", cx));
    // The dialog hands over to the setup, which first asks the backend whether
    // a key is already saved.
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    fixture.update(|window, _| {
        assert!(window.try_find("streaming-refused").is_none());
        assert!(window.try_find("soloist-setup-checking").is_some());
    });

    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::NeedKey),
    );
    fixture.update(|window, _| assert!(window.try_find("soloist-setup-key").is_some()));
    // Nothing happens until there is a key to go on with.
    fixture.update(|window, cx| window.click("soloist-setup-continue", cx));
    fixture.no_commands();
    fixture.update(|window, cx| {
        window.click("soloist-setup-key-input", cx);
        window.input("  my-key-123 ", cx);
    });
    fixture.update(|window, cx| window.click("soloist-setup-continue", cx));
    assert_eq!(setup_command(&mut fixture), Some("my-key-123".into()));

    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Downloading),
    );
    fixture.update(|window, _| assert!(window.try_find("soloist-setup-downloading").is_some()));
    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Pairing),
    );
    fixture.update(|window, _| assert!(window.try_find("soloist-setup-pairing").is_some()));

    // Once Spotify's player is up, the dialog closes and the choice is kept.
    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Done),
    );
    fixture.update(|window, cx| {
        assert!(window.try_find("soloist-setup-pairing").is_none());
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::Soloist
        );
    });
    fixture.no_commands();
}

#[test]
fn a_failed_setup_says_why_and_can_try_the_saved_key_again() {
    if !crate::soloist::supported() {
        return;
    }
    let _restore = RestoreEngine;
    let mut fixture = Fixture::new(false);
    fixture.open_soloist_setup();
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Failed(
            "Spotify’s player stopped as it started".into(),
        )),
    );
    fixture.update(|window, _| {
        assert!(window.try_find("soloist-setup-key").is_some());
        assert!(window.try_find("soloist-setup-error").is_some());
    });
    // The key was saved before, so trying again needs nothing pasted.
    fixture.update(|window, cx| window.click("soloist-setup-continue", cx));
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    fixture.update(|window, _| assert!(window.try_find("soloist-setup-checking").is_some()));
}

#[test]
fn leaving_the_setup_stops_it_and_keeps_the_player_as_it_was() {
    if !crate::soloist::supported() {
        return;
    }
    let _restore = RestoreEngine;
    let mut fixture = Fixture::new(false);
    fixture.open_soloist_setup();
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Pairing),
    );
    fixture.press("escape");
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::CancelSoloistSetup)
    ));
    fixture.update(|window, cx| {
        assert!(window.try_find("soloist-setup-pairing").is_none());
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::BuiltIn
        );
    });
    fixture.no_commands();
}

#[test]
fn settings_switches_between_the_built_in_player_and_spotifys() {
    if !crate::soloist::supported() {
        return;
    }
    let _restore = RestoreEngine;
    let mut fixture = Fixture::new(true);
    fixture.update(|window, cx| {
        window.scroll(
            "settings-page",
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
    });
    // Choosing Spotify's player starts its setup rather than switching blind.
    fixture.update(|window, cx| window.click(("settings-player", 1usize), cx));
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::BuiltIn
        );
    });
    deliver(
        &mut fixture,
        BackendEvent::SoloistSetup(crate::soloist::Step::Done),
    );
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::Soloist
        );
    });
    // And back again: the backend is told, and the choice is kept.
    fixture.update(|window, cx| window.click(("settings-player", 0usize), cx));
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::UseBuiltInPlayer)
    ));
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::BuiltIn
        );
    });
    fixture.no_commands();
}

#[test]
fn spotifys_player_that_wont_connect_can_be_set_up_again_or_swapped_out() {
    if !crate::soloist::supported() {
        return;
    }
    let _restore = RestoreEngine;
    let mut fixture = Fixture::new(false);
    fixture.update(|_, cx| {
        let _ = services::AppServices::set_player(crate::playback::Engine::Soloist, cx);
    });
    deliver(
        &mut fixture,
        BackendEvent::PlaybackConnectFailed("Spotify’s player stopped as it started".into()),
    );
    fixture.update(|window, _| {
        assert!(window.try_find("playback-not-connected").is_some());
        assert!(window.try_find("playback-not-connected-built-in").is_some());
    });
    // Setting up again means redoing Spotify's player, not the browser sign-in.
    fixture.update(|window, cx| window.click("playback-not-connected-reset", cx));
    assert_eq!(setup_command(&mut fixture), Some(String::new()));
    fixture.update(|window, _| assert!(window.try_find("soloist-setup-checking").is_some()));
    fixture.press("escape");
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::CancelSoloistSetup)
    ));

    deliver(
        &mut fixture,
        BackendEvent::PlaybackConnectFailed("Spotify’s player stopped as it started".into()),
    );
    fixture.update(|window, cx| window.click("playback-not-connected-built-in", cx));
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::UseBuiltInPlayer)
    ));
    fixture.update(|_, cx| {
        assert_eq!(
            services::AppServices::preferences(cx).player,
            crate::playback::Engine::BuiltIn
        );
    });
}

#[test]
fn a_song_that_plays_clears_the_account_message() {
    let mut fixture = Fixture::new(false);
    deliver(&mut fixture, BackendEvent::StreamingRefused);
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Pause)
    ));
    fixture.update(|window, cx| window.click("streaming-refused-later", cx));
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: Vec::new(),
    });
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-3".into(),
    });
    assert_eq!(fixture.marked_played(), "track-3");
    fixture.update(|_, cx| assert!(services::AppServices::player(cx).read(cx).error().is_none()));
}

#[test]
fn playback_that_wont_connect_offers_to_try_again_or_set_up_again() {
    let mut fixture = Fixture::new(false);
    deliver(
        &mut fixture,
        BackendEvent::PlaybackConnectFailed(
            "could not refresh librespot playback credentials".into(),
        ),
    );
    fixture.update(|window, _| {
        assert!(window.try_find("playback-not-connected").is_some());
        assert!(window.try_find("playback-not-connected-detail").is_some());
    });
    fixture.update(|window, cx| window.click("playback-not-connected-retry", cx));
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::RetryPlayback)
    ));
    fixture.update(|window, _| assert!(window.try_find("playback-not-connected").is_none()));

    deliver(
        &mut fixture,
        BackendEvent::PlaybackConnectFailed("librespot could not connect to Spotify".into()),
    );
    fixture.update(|window, cx| window.click("playback-not-connected-reset", cx));
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::ResetPlayback)
    ));
    deliver(
        &mut fixture,
        BackendEvent::PlaybackApprovalRequested {
            url: "https://accounts.spotify.com/authorize".into(),
        },
    );
    fixture.update(|window, _| {
        assert!(window.try_find("playback-approval").is_some());
        assert!(window.try_find("playback-approval-reopen").is_some());
    });
    // Approving in the browser connects playback, which closes the dialog.
    deliver(&mut fixture, BackendEvent::PlaybackReady);
    fixture.update(|window, cx| {
        assert!(window.try_find("playback-approval").is_none());
        assert!(services::AppServices::player(cx).read(cx).error().is_none());
    });
    fixture.no_commands();
}

#[test]
fn a_play_while_playback_is_down_brings_the_dialog_back_instead_of_skipping() {
    let mut fixture = Fixture::new(false);
    deliver(
        &mut fixture,
        BackendEvent::PlaybackConnectFailed("network unreachable".into()),
    );
    fixture.update(|window, cx| window.click("playback-not-connected-later", cx));
    fixture.update(|window, _| assert!(window.try_find("playback-not-connected").is_none()));

    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(3),
        next: vec![track(4)],
    });
    deliver(
        &mut fixture,
        BackendEvent::TrackFailed {
            spotify_uri: "spotify:track:track-3".into(),
            error: "Spotify playback is not connected".into(),
        },
    );
    fixture.update(|window, _| assert!(window.try_find("playback-not-connected").is_some()));
    fixture.no_commands();
}

#[test]
fn playback_stops_after_three_songs_in_a_row_fail() {
    let mut fixture = Fixture::new(false);
    for index in 3..5 {
        fixture.player_event(BackendEvent::PlaybackContext {
            current: track(index),
            next: vec![track(index + 1)],
        });
        deliver(
            &mut fixture,
            BackendEvent::TrackFailed {
                spotify_uri: format!("spotify:track:track-{index}"),
                error: "Spotify cannot play this track".into(),
            },
        );
        assert!(matches!(
            fixture.backend.commands.try_recv(),
            Ok(BackendCommand::Next)
        ));
    }
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(5),
        next: vec![track(6)],
    });
    deliver(
        &mut fixture,
        BackendEvent::TrackFailed {
            spotify_uri: "spotify:track:track-5".into(),
            error: "Spotify cannot play this track".into(),
        },
    );
    fixture.no_commands();
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        let notice = workspace.upgrade().unwrap().read(cx).action_notice.clone();
        assert!(notice.is_some_and(|notice| notice.contains("3 songs in a row")));
    });

    // A song that plays starts the count over.
    fixture.player_event(BackendEvent::PlaybackContext {
        current: track(6),
        next: vec![track(7)],
    });
    fixture.player_event(BackendEvent::Playing {
        spotify_uri: "spotify:track:track-6".into(),
    });
    assert_eq!(fixture.marked_played(), "track-6");
    deliver(
        &mut fixture,
        BackendEvent::TrackFailed {
            spotify_uri: "spotify:track:track-6".into(),
            error: "Spotify cannot play this track".into(),
        },
    );
    assert!(matches!(
        fixture.backend.commands.try_recv(),
        Ok(BackendCommand::Next)
    ));
}

#[test]
fn whats_new_opens_from_the_sidebar_and_back_returns() {
    let mut fixture = Fixture::new(false);
    let before = fixture.route();
    fixture.update(|window, cx| window.click("sidebar-whats-new", cx));
    assert_eq!(fixture.route(), Route::WhatsNew);
    // Older months are folded until opened.
    fixture.update(|window, _| assert!(window.try_find(("whats-new-month", 1usize)).is_some()));
    fixture.update(|window, cx| window.click("detail-back", cx));
    assert_eq!(fixture.route(), before);
    fixture.no_commands();
}

#[test]
fn the_sidebar_offers_an_available_update_with_one_button() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|window, _| assert!(window.try_find("sidebar-update").is_none()));
    fixture.update(|_, cx| {
        workspace
            .upgrade()
            .unwrap()
            .read(cx)
            .sidebar
            .clone()
            .update(cx, |sidebar, cx| {
                sidebar.set_update(
                    Some(crate::app::sidebar::UpdateOffer::Available("9.9.9".into())),
                    cx,
                )
            })
    });
    fixture.update(|window, _| assert!(window.try_find("sidebar-update").is_some()));
    fixture.no_commands();
}

#[test]
fn popular_songs_stop_at_five_until_show_more_is_pressed() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        cx.set_reduce_motion(true);
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_artist(
                artist_ref("Artist", "artist-popular"),
                Route::LikedSongs,
                cx,
            );
        });
    });
    let BackendCommand::LoadArtist { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected artist request");
    };
    respond
        .send(Ok((
            model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "artist-popular".into(),
                spotify_uri: None,
                name: "Artist".into(),
                artwork_url: None,
            },
            (0..10).map(track).collect(),
            Vec::new().into(),
        )))
        .unwrap();
    fixture.update(|_, _| {});
    // Bring the lower rows and the toggle on screen.
    fixture.update(|window, cx| {
        let position = window.find("artist-portrait").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-360.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.try_find(("spotify-track", 4usize)).is_some());
        assert!(window.try_find(("spotify-track", 5usize)).is_none());
        assert!(window.try_find("artist-popular-toggle").is_some());
    });
    fixture.update(|window, cx| window.click("artist-popular-toggle", cx));
    fixture.update(|window, _| {
        assert!(window.try_find(("spotify-track", 6usize)).is_some());
    });
    // "Show fewer" now sits below the fold.
    fixture.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position: point(px(700.), px(400.)),
                delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-500.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.update(|_, _| {});
    fixture.update(|window, cx| window.click("artist-popular-toggle", cx));
    fixture.update(|window, _| {
        assert!(window.try_find(("spotify-track", 5usize)).is_none());
    });
    fixture.no_commands();
}

#[test]
fn an_artist_shows_its_monthly_listeners_under_the_name_once_spotify_says() {
    crate::backend::KEEP_LISTENER_REQUESTS.with(|keep| keep.set(true));
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        cx.set_reduce_motion(true);
        workspace.upgrade().unwrap().update(cx, |workspace, cx| {
            workspace.open_artist(
                artist_ref("Artist", "artist-listeners"),
                Route::LikedSongs,
                cx,
            );
        });
    });
    let mut listeners = None;
    let mut artist = None;
    while let Ok(command) = fixture.backend.commands.try_recv() {
        match command {
            BackendCommand::LoadArtistListeners { source_id, respond } => {
                assert_eq!(source_id, "artist-listeners");
                listeners = Some(respond);
            }
            BackendCommand::LoadArtist { respond, .. } => artist = Some(respond),
            _ => {}
        }
    }
    artist
        .expect("the artist is asked for")
        .send(Ok((
            model::Artist {
                genres: Vec::new(),
                provider: model::Provider::Spotify,
                source_id: "artist-listeners".into(),
                spotify_uri: None,
                name: "Artist".into(),
                artwork_url: None,
            },
            vec![track(0)],
            Vec::new().into(),
        )))
        .unwrap();
    fixture.update(|_, _| {});
    // Until Spotify answers, a placeholder holds the line and the name is
    // where it will stay.
    let before = fixture.update(|window, _| window.find("artist-moving-name").bounds());
    let placeholder = fixture.update(|window, _| window.find("artist-listeners").bounds().size);
    assert!(placeholder.width > px(0.), "a placeholder holds the line");
    listeners
        .expect("the listener count is asked for")
        .send(Ok(Some(8_123_456)))
        .unwrap();
    fixture.update(|_, _| {});
    fixture.update(|window, _| {
        assert!(window.find("artist-listeners").bounds().size.width > px(0.));
        assert_eq!(window.find("artist-moving-name").bounds(), before);
    });
    crate::backend::KEEP_LISTENER_REQUESTS.with(|keep| keep.set(false));
}

/// A window opened for an install that has never run Sonder.
fn first_launch_fixture() -> Fixture {
    let mut cx = HeadlessAppContext::with_asset_source(
        Arc::new(NoopTextSystem),
        Arc::new(assets::CadenceAssets),
    );
    let backend = cx.update(|cx| initialize(cx, ThemePreference::Dark));
    cx.update(|cx| services::AppServices::set_launch_intro_pending(true, cx));
    let (window, workspace) = workspace(&mut cx, 1280., 820.);
    settle(&mut cx, window.into());
    Fixture {
        cx,
        window,
        workspace: Some(workspace.downgrade()),
        backend,
    }
}

#[test]
fn the_launch_intro_covers_a_first_launch_once_and_cannot_be_skipped() {
    let mut fixture = first_launch_fixture();
    fixture.update(|window, _| assert!(window.try_find("launch-intro").is_some()));
    // It is claimed by this window: no other window (or relaunch) gets it.
    fixture.update(|_, cx| assert!(!services::AppServices::take_launch_intro(cx)));
    fixture.press("escape");
    fixture.update(|_, _| {});
    fixture.update(|window, _| assert!(window.try_find("launch-intro").is_some()));
}

#[test]
fn a_launch_that_is_not_the_first_never_shows_the_intro() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, _| assert!(window.try_find("launch-intro").is_none()));
}

#[test]
fn a_click_neither_skips_the_launch_intro_nor_reaches_the_page_beneath() {
    let mut fixture = first_launch_fixture();
    let route = fixture.route();
    fixture.update(|window, cx| window.click("launch-intro", cx));
    fixture.update(|_, _| {});
    fixture.update(|window, _| assert!(window.try_find("launch-intro").is_some()));
    assert_eq!(fixture.route(), route);
}

#[test]
fn the_link_dialog_always_opens_with_an_empty_field() {
    let mut fixture = on_local_files();
    fixture.update(|window, cx| window.click("local-files-link", cx));
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    let popover = fixture.update(|_, cx| {
        workspace
            .upgrade()
            .expect("workspace")
            .read(cx)
            .local_files
            .read(cx)
            .link_popover()
            .expect("open")
    });
    fixture.update(|window, cx| {
        popover.update(cx, |popover, cx| {
            popover.fixture_type(
                "https://example.com/ana-lind/harbour-lights-live",
                window,
                cx,
            )
        })
    });
    fixture.update(|window, _| assert!(window.try_find("link-paste").is_none()));

    // Closing and opening again starts over: no link, nothing found.
    fixture.update(|window, cx| window.press("escape", cx));
    fixture.update(|window, cx| window.click("local-files-link", cx));
    fixture.update(|window, _| {
        assert!(window.try_find("link-paste").is_some());
        assert!(window.try_find("link-save").is_none());
    });
}

#[test]
fn deleting_a_local_song_removes_its_row_at_once_and_a_failure_brings_it_back() {
    let mut fixture = on_local_files();
    let library = |fixture: &mut Fixture| {
        fixture.update(|_, cx| {
            services::AppServices::library(cx)
                .read(cx)
                .local_tracks()
                .len()
        })
    };
    assert_eq!(library(&mut fixture), 1);

    fixture.right_click_track(0);
    fixture.update(|window, _| assert!(window.try_find(("track-menu-delete", 0usize)).is_some()));
    fixture.update(|window, cx| window.click(("track-menu-delete", 0usize), cx));
    // The row is gone before the file system has answered.
    assert_eq!(library(&mut fixture), 0);

    // The file is not really there, so moving it fails; the next scan, which
    // is always asked for, shows the truth and the row returns.
    for _ in 0..4 {
        fixture.update(|_, _| {});
    }
    assert!(answer_local_files(
        &mut fixture,
        vec![local_track("Harbour")]
    ));
    assert_eq!(library(&mut fixture), 1);
}

type Players<'a> = &'a [(&'a str, Option<&'a str>)];

fn credits(roles: &[(&str, Players)]) -> crate::credits::Credits {
    crate::credits::Credits {
        roles: roles
            .iter()
            .map(|(label, people)| crate::credits::Role {
                label: (*label).to_owned(),
                people: people
                    .iter()
                    .map(|(name, detail)| crate::credits::Person {
                        name: (*name).to_owned(),
                        detail: detail.map(str::to_owned),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn open_credits(
    fixture: &mut Fixture,
    index: usize,
) -> (
    String,
    tokio::sync::oneshot::Sender<anyhow::Result<crate::credits::Credits>>,
) {
    fixture.right_click_track(index);
    fixture.update(|window, cx| window.click(("track-menu-credits", index), cx));
    let BackendCommand::LoadCredits { isrc, respond } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("expected a credits lookup")
    };
    (isrc, respond)
}

#[test]
fn credits_open_from_a_songs_menu_show_who_made_it_and_close_with_escape() {
    let mut fixture = Fixture::new(false);
    let (isrc, respond) = open_credits(&mut fixture, 1);
    assert_eq!(isrc, track(1).isrc.unwrap(), "asked by the song's ISRC");
    fixture.update(|window, _| {
        assert!(window.try_find("credits-dialog").is_some());
        assert!(window.try_find("credits-loading").is_some());
    });
    respond
        .send(Ok(credits(&[
            ("Written by", &[("Thom Yorke", None)]),
            (
                "Performed by",
                &[("Jonny Greenwood", Some("guitar, piano"))],
            ),
        ])))
        .unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, _| {
        assert!(window.try_find("credits-loading").is_none());
        assert!(window.try_find(("credits-role", 0usize)).is_some());
        assert!(window.try_find(("credits-role", 1usize)).is_some());
        assert!(window.try_find(("credits-role", 2usize)).is_none());
    });
    fixture.update(|window, cx| window.press("escape", cx));
    fixture.update(|window, _| assert!(window.try_find("credits-dialog").is_none()));
    fixture.no_commands();
}

#[test]
fn credits_say_so_when_nothing_is_listed_and_retry_after_a_failure() {
    let mut fixture = Fixture::new(false);
    let (_, respond) = open_credits(&mut fixture, 0);
    respond.send(Err(anyhow::anyhow!("offline"))).unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, cx| {
        assert!(window.try_find("credits-retry").is_some());
        window.click("credits-retry", cx);
    });
    let BackendCommand::LoadCredits { respond, .. } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("retry asks again")
    };
    respond
        .send(Ok(crate::credits::Credits::default()))
        .unwrap();
    fixture.cx.run_until_parked();
    fixture.update(|window, _| {
        assert!(window.try_find("credits-empty").is_some());
        assert!(window.try_find("credits-retry").is_none());
    });
    // Closing from the button leaves nothing asked.
    fixture.update(|window, cx| window.click("credits-close", cx));
    fixture.update(|window, _| assert!(window.try_find("credits-dialog").is_none()));
}

#[test]
fn pressing_the_dim_area_closes_the_credits() {
    let mut fixture = Fixture::new(false);
    let _pending = open_credits(&mut fixture, 0);
    fixture.update(|window, cx| {
        assert!(window.try_find("credits-dialog").is_some());
        window.click_at("credits-scrim", point(px(4.), px(4.)), cx);
    });
    fixture.update(|window, _| assert!(window.try_find("credits-dialog").is_none()));
}

fn home_mix(name: &str, id: &str) -> model::HomeMix {
    model::HomeMix {
        playlist: model::Playlist {
            provider: model::Provider::Spotify,
            source_id: id.to_owned(),
            name: name.to_owned().into(),
            owner: "Spotify".into(),
            track_count: 0,
            artwork_url: None,
        },
        description: Some("glaive, Ericdoa and more".to_owned()),
    }
}

/// Opens Home and answers what it asks with one release and `mixes`; `None`
/// answers Made for you with a failure.
fn home_with_mixes(mixes: Option<model::HomeMixes>) -> Fixture {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("nav-home", cx));
    let mut released = false;
    let mut mixes = mixes;
    fixture.update(|_, _| {});
    while let Ok(command) = fixture.backend.commands.try_recv() {
        match command {
            BackendCommand::LoadHome { respond, .. } => {
                respond.send(Ok(model::HomeData::default())).unwrap();
            }
            BackendCommand::LoadNewReleases { respond } => {
                respond.send(Ok(vec![release_album()])).unwrap();
                released = true;
            }
            BackendCommand::LoadMadeForYou { respond, .. } => respond
                .send(mixes.take().ok_or_else(|| anyhow::anyhow!("unavailable")))
                .unwrap(),
            _ => {}
        }
        fixture.update(|_, _| {});
    }
    assert!(released, "Home asked for new releases");
    fixture
}

#[test]
fn home_shows_made_for_you_and_your_top_mixes_and_opens_one() {
    let mut fixture = home_with_mixes(Some(model::HomeMixes {
        made_for_you: vec![home_mix("glaive Mix", "37i9dQZF1E39Q6ggt8iyvY")],
        top_mixes: vec![home_mix("Top mix 1", "37i9dQZF1E3aaaaaaaaaaa")],
    }));
    fixture.update(|window, _| {
        assert!(window.try_find("home-made-heading").is_some());
        assert!(window.try_find("home-top-heading").is_some());
        assert!(window.try_find(("home-made", 0usize)).is_some());
        // Fewer cards than a row holds leave no "Show all" to press.
        assert!(window.try_find("home-made-show-all").is_none());
    });
    fixture.update(|window, cx| window.click(("home-made", 0usize), cx));
    assert_eq!(fixture.route(), Route::Playlist);
}

#[test]
fn home_leaves_the_mix_shelves_out_when_spotify_will_not_say() {
    let mut fixture = home_with_mixes(None);
    fixture.update(|window, _| {
        assert!(window.try_find("home-made-heading").is_none());
        assert!(window.try_find("home-top-heading").is_none());
        // Everything else is still there.
        assert!(window.try_find("home-shortcuts").is_some());
        assert!(window.try_find("home-releases-heading").is_some());
    });
}

#[test]
fn show_all_opens_a_shelf_out_and_closes_it_again() {
    let mixes = (0..9)
        .map(|n| home_mix(&format!("Mix {n}"), &format!("37i9dQZF1E3{n:011}")))
        .collect::<Vec<_>>();
    let mut fixture = home_with_mixes(Some(model::HomeMixes {
        made_for_you: mixes,
        top_mixes: Vec::new(),
    }));
    fixture.update(|window, _| {
        assert!(window.try_find(("home-made", 3usize)).is_some());
        assert!(window.try_find(("home-made", 8usize)).is_none());
    });
    fixture.update(|window, cx| window.click("home-made-show-all", cx));
    fixture.update(|window, _| assert!(window.try_find(("home-made", 8usize)).is_some()));
    fixture.update(|window, cx| window.click("home-made-show-all", cx));
    fixture.update(|window, _| assert!(window.try_find(("home-made", 8usize)).is_none()));
}

#[test]
fn a_rail_release_plays_from_its_button_without_leaving_home() {
    let mut fixture = home_with_mixes(None);
    fixture.update(|window, cx| window.click(("home-release-play", 0usize), cx));
    let BackendCommand::LoadAlbum { source_id, respond } = fixture
        .backend
        .commands
        .try_recv()
        .expect("the album is fetched")
    else {
        panic!("expected an album request")
    };
    assert_eq!(source_id, "release-1");
    respond
        .send(Ok((
            model::Album {
                provider: model::Provider::Spotify,
                source_id: "release-1".into(),
                spotify_uri: None,
                name: "Fresh Release".into(),
                artists: Vec::new(),
                release_date: None,
                artwork_url: None,
                track_count: None,
            },
            vec![track(0), track(1)],
        )))
        .unwrap();
    fixture.update(|_, _| {});
    let BackendCommand::PlayContext { tracks, index } = fixture
        .backend
        .commands
        .try_recv()
        .expect("playback starts")
    else {
        panic!("expected playback")
    };
    assert_eq!((tracks.len(), index), (2, 0));
    assert_eq!(fixture.route(), Route::Home);
}

#[test]
fn profile_opens_from_the_account_menu_and_marks_the_avatar() {
    let mut fixture = Fixture::new(false);
    fixture.update(|window, cx| window.click("account", cx));
    fixture.update(|window, cx| window.click("account-profile", cx));
    assert_eq!(fixture.route(), Route::Profile);
    fixture.update(|window, _| {
        assert!(window.try_find("account-menu").is_none());
        assert!(window.try_find("profile-page").is_some());
    });
    // Opening the menu warmed the page: the saved answer to show at once, and
    // one request to check it. Choosing Profile then asks for nothing more.
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::LoadProfileSaved { .. }
    ));
    assert!(matches!(
        fixture.backend.commands.try_recv().unwrap(),
        BackendCommand::LoadProfile { .. }
    ));
    fixture.no_commands();
}

#[test]
fn a_top_artist_on_the_profile_opens_that_artist() {
    let mut fixture = Fixture::new(false);
    let workspace = fixture.workspace.clone().unwrap();
    fixture.update(|_, cx| {
        let _ = workspace.update(cx, |workspace, cx| {
            workspace.navigate(Route::Profile, cx);
            workspace.profile.update(cx, |page, cx| {
                let artist = model::Artist {
                    genres: Vec::new(),
                    provider: model::Provider::Spotify,
                    source_id: "artist-glaive".into(),
                    spotify_uri: None,
                    name: "glaive".into(),
                    artwork_url: None,
                };
                page.seed(
                    model::ProfileData {
                        top: model::ProfileTop {
                            ranges: [Vec::new(), vec![artist], Vec::new()],
                            followed: None,
                        },
                        recent_hours: Some(Vec::new()),
                    },
                    cx,
                );
            });
        });
    });
    // Opening the page asked for its data; that is not what this checks.
    while fixture.backend.commands.try_recv().is_ok() {}
    fixture.update(|window, cx| window.click(("profile-artist", 0usize), cx));
    assert_eq!(fixture.requested_artist(), "artist-glaive");
    assert_eq!(fixture.route(), Route::Artist);
}

// ---------------------------------------------------------------- equalizer

fn audio_fixture() -> Fixture {
    let mut fixture = Fixture::new(true);
    fixture.update(|window, cx| window.click(("settings-tab", 2usize), cx));
    fixture
}

impl Fixture {
    fn equalizer(&mut self) -> crate::equalizer::Settings {
        self.update(|_, cx| services::AppServices::preferences(cx).equalizer)
    }

    fn stored_equalizer(&mut self) -> crate::equalizer::Settings {
        self.update(|_, cx| services::AppServices::stored_equalizer(cx).expect("settings store"))
    }

    fn comparing(&mut self) -> bool {
        let workspace = self.workspace.clone().expect("workspace fixture");
        self.update(|_, cx| {
            workspace
                .upgrade()
                .expect("live workspace")
                .read(cx)
                .settings
                .read(cx)
                .comparing()
        })
    }

    /// The point a band's handle sits at, in window coordinates.
    fn band_point(&mut self, band: usize) -> Point<Pixels> {
        self.update(|window, _| window.find(("equalizer-band", band)).bounds().center())
    }

    fn graph(&mut self) -> gpui_kit::Bounds<Pixels> {
        self.update(|window, _| window.find("equalizer-graph").bounds())
    }
}

#[test]
fn the_equalizer_starts_off_and_flat_and_a_preset_turns_it_on_and_saves() {
    let mut fixture = audio_fixture();
    assert_eq!(fixture.equalizer(), crate::equalizer::Settings::default());

    // "Bass boost" is the second built-in chip.
    fixture.update(|window, cx| window.click(("equalizer-preset", 1usize), cx));
    let expected = crate::equalizer::Settings {
        enabled: true,
        steps: [12, 6, 0, 0, 0],
    };
    assert_eq!(fixture.equalizer(), expected);
    assert_eq!(fixture.stored_equalizer(), expected);

    fixture.update(|window, cx| window.click(("equalizer-preset", 0usize), cx));
    assert!(fixture.equalizer().is_flat());
    assert!(fixture.equalizer().enabled);
    fixture.no_commands();
}

#[test]
fn the_switch_turns_the_equalizer_off_without_losing_the_curve() {
    let mut fixture = audio_fixture();
    fixture.update(|window, cx| window.click(("equalizer-preset", 2usize), cx));
    let shaped = fixture.equalizer();
    assert!(shaped.enabled && !shaped.is_flat());

    fixture.update(|window, cx| window.click("settings-equalizer", cx));
    let off = fixture.equalizer();
    assert!(!off.enabled);
    assert_eq!(off.steps, shaped.steps);
    assert!(!fixture.stored_equalizer().enabled);

    fixture.update(|window, cx| window.click("settings-equalizer", cx));
    assert_eq!(fixture.equalizer(), shaped);
}

#[test]
fn compare_plays_the_original_and_ends_with_the_tab() {
    let mut fixture = audio_fixture();
    // With the equalizer off there is nothing to compare.
    fixture.update(|window, cx| window.click("settings-equalizer-compare", cx));
    assert!(!fixture.comparing());

    fixture.update(|window, cx| window.click(("equalizer-preset", 1usize), cx));
    fixture.update(|window, cx| window.click("settings-equalizer-compare", cx));
    assert!(fixture.comparing());
    // The settings stay as they were while comparing.
    assert_eq!(fixture.equalizer().steps, [12, 6, 0, 0, 0]);
    assert!(fixture.equalizer().enabled);

    // Shaping the sound is always heard, so it leaves Compare.
    fixture.update(|window, cx| window.click(("equalizer-preset", 2usize), cx));
    assert!(!fixture.comparing());

    fixture.update(|window, cx| window.click("settings-equalizer-compare", cx));
    assert!(fixture.comparing());
    fixture.update(|window, cx| window.click(("settings-tab", 0usize), cx));
    assert!(!fixture.comparing());

    // Leaving Settings ends it too.
    fixture.update(|window, cx| window.click(("settings-tab", 2usize), cx));
    fixture.update(|window, cx| window.click("settings-equalizer-compare", cx));
    assert!(fixture.comparing());
    fixture.update(|window, cx| window.click("nav-recent", cx));
    fixture.update(|_, _| ());
    assert!(!fixture.comparing());
}

#[test]
fn dragging_a_point_follows_the_pointer_and_saves_when_it_lets_go() {
    let mut fixture = audio_fixture();
    let graph = fixture.graph();
    let start = fixture.band_point(2);
    // 1 kHz: lift the middle point a third of the way up the graph.
    let target = point(start.x, graph.origin.y + graph.size.height * 0.25);

    fixture.mouse_down(start);
    // The first move starts the drag; the ones after it are delivered to it.
    fixture.mouse_move_to(point(start.x, graph.origin.y + graph.size.height * 0.45));
    fixture.mouse_move_to(point(start.x, graph.origin.y + graph.size.height * 0.4));
    let during = fixture.equalizer();
    assert!(during.enabled && during.steps[2] > 0, "{during:?}");
    // Nothing is written mid-drag; the sound follows, the file waits.
    assert_eq!(
        fixture.stored_equalizer(),
        crate::equalizer::Settings::default()
    );

    fixture.mouse_move_to(target);
    fixture.mouse_up(target);
    let after = fixture.equalizer();
    assert!(after.steps[2] > during.steps[2], "{after:?} vs {during:?}");
    assert_eq!(fixture.stored_equalizer(), after);
    // The other bands are untouched, and the point sits on the curve now.
    assert_eq!(after.steps[0], 0);
    assert_eq!(after.steps[4], 0);

    // Pulling it below the graph pins it at the bottom of the range.
    let below = point(start.x, graph.origin.y + graph.size.height + px(300.));
    let grab = fixture.band_point(2);
    fixture.mouse_down(grab);
    fixture.mouse_move_to(point(grab.x, grab.y + px(20.)));
    fixture.mouse_move_to(below);
    fixture.mouse_up(below);
    assert_eq!(fixture.equalizer().steps[2], -24);
    fixture.no_commands();
}

#[test]
fn pressing_between_points_moves_the_nearest_one() {
    let mut fixture = audio_fixture();
    let graph = fixture.graph();
    // Near the 4 kHz label, well above the curve.
    let position = point(
        graph.origin.x + graph.size.width * 0.72,
        graph.origin.y + graph.size.height * 0.2,
    );
    fixture.mouse_down(position);
    fixture.mouse_up(position);
    let steps = fixture.equalizer().steps;
    assert!(steps[3] > 0, "{steps:?}");
    assert_eq!([steps[0], steps[1], steps[2], steps[4]], [0, 0, 0, 0]);
}

#[test]
fn a_double_press_resets_the_point() {
    let mut fixture = audio_fixture();
    fixture.update(|window, cx| window.click(("equalizer-preset", 1usize), cx));
    assert!(fixture.equalizer().steps[0] > 0);
    let position = fixture.band_point(0);
    fixture.update(|window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 2,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    });
    fixture.mouse_up(position);
    assert_eq!(fixture.equalizer().steps[0], 0);
    assert_eq!(fixture.stored_equalizer().steps[0], 0);
    // The other bands kept their preset values.
    assert_eq!(fixture.equalizer().steps[1], 6);
}

#[test]
fn a_focused_point_moves_with_the_arrow_keys() {
    let mut fixture = audio_fixture();
    fixture.tab_to(("equalizer-band", 1usize));
    fixture.press("up");
    assert_eq!(fixture.equalizer().steps, [0, 2, 0, 0, 0]);
    assert!(fixture.equalizer().enabled);
    fixture.press("pageup");
    assert_eq!(fixture.equalizer().steps[1], 8);
    fixture.press("down");
    assert_eq!(fixture.equalizer().steps[1], 6);
    // Past the end stays at the end.
    for _ in 0..20 {
        fixture.press("pageup");
    }
    assert_eq!(fixture.equalizer().steps[1], 24);
    fixture.press("home");
    assert_eq!(fixture.equalizer().steps[1], 0);
    assert_eq!(fixture.stored_equalizer().steps[1], 0);
    // Other points did not move, and nothing reached playback.
    assert_eq!(fixture.equalizer().steps, [0; 5]);
    fixture.no_commands();
}

#[test]
fn saved_presets_are_named_kept_and_deleted() {
    let mut fixture = audio_fixture();
    fixture.update(|window, cx| window.click(("equalizer-preset", 3usize), cx));
    let curve = fixture.equalizer().steps;

    fixture.update(|window, cx| window.click("equalizer-save-preset", cx));
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    let name = |fixture: &mut Fixture, text: &'static str| {
        let workspace = workspace.clone();
        fixture.update(move |window, cx| {
            let settings = workspace
                .upgrade()
                .expect("live workspace")
                .read(cx)
                .settings
                .clone();
            settings.update(cx, |settings, cx| {
                settings.name_preset_for_test(text, window, cx)
            });
        });
    };

    // An empty name and a built-in name are refused, and nothing is saved.
    fixture.update(|window, cx| window.click("equalizer-save-confirm", cx));
    name(&mut fixture, "  vocal ");
    fixture.update(|window, cx| window.click("equalizer-save-confirm", cx));
    assert!(
        fixture
            .update(|_, cx| services::AppServices::equalizer_presets(cx))
            .is_empty()
    );

    name(&mut fixture, "  My night mix  ");
    fixture.update(|window, cx| window.click("equalizer-save-confirm", cx));
    let saved = fixture.update(|_, cx| services::AppServices::equalizer_presets(cx));
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "My night mix");
    assert_eq!(saved[0].steps, curve);
    fixture.update(|window, _| {
        assert!(
            window
                .try_find(("equalizer-custom-preset", 0usize))
                .is_some()
        )
    });

    // Choosing the saved chip restores the curve after another one.
    fixture.update(|window, cx| window.click(("equalizer-preset", 0usize), cx));
    assert!(fixture.equalizer().is_flat());
    fixture.update(|window, cx| window.click(("equalizer-custom-preset", 0usize), cx));
    assert_eq!(fixture.equalizer().steps, curve);

    // Saving under the same name replaces it instead of adding another.
    fixture.update(|window, cx| window.click(("equalizer-preset", 1usize), cx));
    fixture.update(|window, cx| window.click("equalizer-save-preset", cx));
    name(&mut fixture, "my NIGHT mix");
    fixture.update(|window, cx| window.click("equalizer-save-confirm", cx));
    let saved = fixture.update(|_, cx| services::AppServices::equalizer_presets(cx));
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].steps, [12, 6, 0, 0, 0]);

    fixture.update(|window, cx| window.click(("equalizer-custom-preset-delete", 0usize), cx));
    assert!(
        fixture
            .update(|_, cx| services::AppServices::equalizer_presets(cx))
            .is_empty()
    );
    fixture.update(|window, _| {
        assert!(
            window
                .try_find(("equalizer-custom-preset", 0usize))
                .is_none()
        )
    });
}

#[test]
fn only_a_few_presets_can_be_kept() {
    let mut fixture = audio_fixture();
    let workspace = fixture.workspace.clone().expect("workspace fixture");
    for index in 0..crate::equalizer::MAX_CUSTOM_PRESETS + 1 {
        // The chips grow the card, so the button moves down the page.
        fixture.update(|window, cx| {
            let position = window.find("settings-page").bounds().center();
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-3000.))),
                    modifiers: Default::default(),
                    touch_phase: gpui_kit::TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
        });
        fixture.update(|window, cx| window.click("equalizer-save-preset", cx));
        let workspace = workspace.clone();
        let text = format!("Mine {index}");
        fixture.update(move |window, cx| {
            let settings = workspace
                .upgrade()
                .expect("live workspace")
                .read(cx)
                .settings
                .clone();
            settings.update(cx, |settings, cx| {
                settings.name_preset_for_test(&text, window, cx)
            });
        });
        fixture.update(|window, cx| window.click("equalizer-save-confirm", cx));
    }
    let saved = fixture.update(|_, cx| services::AppServices::equalizer_presets(cx));
    assert_eq!(saved.len(), crate::equalizer::MAX_CUSTOM_PRESETS);
}

#[test]
fn the_player_bar_heart_likes_and_unlikes_the_playing_spotify_song() {
    let mut fixture = Fixture::new(false);
    fixture.play(track(1));
    fixture.no_commands();
    fixture.update(|window, _| {
        assert!(window.find("player-like").bounds().size.width > px(0.));
    });
    // Song 1 is already liked, so the heart removes it.
    fixture.update(|window, cx| window.click("player-like", cx));
    let BackendCommand::SetSpotifyLiked {
        track: sent,
        liked,
        respond,
    } = fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("Spotify write")
    };
    assert_eq!(sent, track(1));
    assert!(!liked);
    // Pending: a second click is ignored until Spotify answers.
    fixture.update(|window, cx| window.click("player-like", cx));
    fixture.no_commands();
    respond.send(Ok(())).unwrap();
    fixture.update(|_, _| {});
    fixture.update(|_, cx| {
        assert!(
            !services::AppServices::library(cx)
                .read(cx)
                .is_spotify_liked(&track(1))
        );
    });
    // Now it likes the song again.
    fixture.update(|window, cx| window.click("player-like", cx));
    let BackendCommand::SetSpotifyLiked { liked, respond, .. } =
        fixture.backend.commands.try_recv().unwrap()
    else {
        panic!("Spotify write")
    };
    assert!(liked);
    respond.send(Ok(())).unwrap();
    fixture.update(|_, _| {});
    fixture.update(|_, cx| {
        assert!(
            services::AppServices::library(cx)
                .read(cx)
                .is_spotify_liked(&track(1))
        );
    });
}

#[test]
fn local_songs_have_no_player_bar_heart() {
    let mut fixture = Fixture::new(false);
    fixture.play(local_track("Local song"));
    fixture.update(|window, _| {
        assert!(window.try_find("player-like").is_none());
    });
}
