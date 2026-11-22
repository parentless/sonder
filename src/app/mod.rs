use std::{
    cell::Cell,
    collections::HashSet,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::{
    backend::{Backend, BackendCommand, BackendEvent, BackendHandle, LibraryReload, Reply},
    lifecycle::{Instance, InstanceLifecycle},
    model,
    spotify::{self, ClientIdSource, valid_client_id},
    storage::{
        AppPreferences, Design, LyricsView, SelectionColor, Store, StreamingQuality,
        ThemePreference, UiZoom,
    },
};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    Icon, Sizable, Theme, WindowExt,
    avatar::Avatar,
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
    theme::ThemeMode,
    v_flex,
};
use gpui_kit::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Bounds, ClipboardItem, Context, Div,
    ElementId, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels, Rems,
    RenderOnce, SharedString, Stateful, Subscription, Window, WindowAppearance, WindowBounds,
    WindowOptions, actions, anchored, deferred, div, ease_out_quint, img, point, prelude::*, px,
    relative, rgb, size, uniform_list,
};
use icons::CadenceIcon;

use library_pages::LibrarySection;
use workspace::Workspace;

mod ambient;
mod disk_cache;
mod home;
mod http;
mod image_cache;
mod playlist_buffer;
#[cfg(target_os = "windows")]
mod tray_host;

#[cfg(test)]
pub(crate) mod test_support;

actions!(
    cadence,
    [
        OpenSearch,
        TogglePlayback,
        Quit,
        CloseWindow,
        DismissOverlay,
        CheckForUpdates,
        SeekBackward,
        SeekForward,
        SeekBackwardLarge,
        SeekForwardLarge,
        SeekToStart,
        SeekToEnd
    ]
);

/// The key context of the main window.
const WORKSPACE_KEY_CONTEXT: &str = "Sonder";
/// The key context of the onboarding window.
const ONBOARDING_KEY_CONTEXT: &str = "Onboarding";
/// The key context of the focused progress slider.
const SCRUBBER_KEY_CONTEXT: &str = "Scrubber";
/// The key context of a focused control. Space activates the control instead
/// of toggling playback while it is set.
const CONTROL_KEY_CONTEXT: &str = "Control";
/// The key context gpui-kit gives a focused text input.
const INPUT_KEY_CONTEXT: &str = "Input";

/// What the listener sees when a command cannot reach the backend.
const BACKEND_UNAVAILABLE: &str = "Sonder backend is busy or not running";

/// Dims everything behind a dialog that must be answered, in either appearance.
const BLOCKING_SCRIM: gpui_kit::Hsla = gpui_kit::Hsla {
    h: 0.,
    s: 0.,
    l: 0.,
    a: 0.6,
};

#[derive(Clone, Copy)]
struct CadencePalette {
    canvas: u32,
    surface: u32,
    surface_raised: u32,
    surface_hover: u32,
    control: u32,
    control_hover: u32,
    /// What a card on the page turns on hover (the card fill itself is `tile`).
    card_hover: u32,
    selection: u32,
    text_primary: u32,
    text: u32,
    text_muted: u32,
    border: u32,
    focus_ring: u32,
    danger: u32,
    destructive: u32,
    on_destructive: u32,
    scrim: gpui_kit::Hsla,
    link: u32,
    accent_hover: u32,
    on_accent: u32,
    /// The big structural fills. Solid, they are the plain roles above; the
    /// glass window styles give them an alpha so the desktop shows through.
    window: Fill,
    sidebar: Fill,
    page: Fill,
    player: Fill,
    tile: Fill,
    tile_hover: Fill,
    /// What a sidebar row or a list row rests on. Transparent in glass, so rows
    /// do not paint dark strips over the see-through surface.
    sidebar_rest: Fill,
    page_rest: Fill,
    /// Which design this palette belongs to, so views can lay themselves out to
    /// match without a second lookup.
    design: Design,
    dark: bool,
}

/// A colour with an alpha, for the surfaces that can be see-through.
#[derive(Clone, Copy, PartialEq)]
struct Fill {
    rgb: u32,
    alpha: f32,
}

impl Fill {
    const fn solid(rgb: u32) -> Self {
        Self { rgb, alpha: 1. }
    }

    const fn glass(rgb: u32, alpha: f32) -> Self {
        Self { rgb, alpha }
    }

    fn color(self) -> gpui_kit::Hsla {
        gpui_kit::Hsla::from(rgb(self.rgb)).opacity(self.alpha)
    }
}

impl CadencePalette {
    const LIGHT: Self = Self {
        canvas: 0xFBFAF9,
        surface: 0xFFFFFF,
        surface_raised: 0xF2F0ED,
        surface_hover: 0xF8F7F4,
        control: 0xF6F4EF,
        control_hover: 0xEAE6DD,
        card_hover: 0xECECF0,
        selection: 0xD8ECFC,
        text_primary: 0x171717,
        text: 0x494440,
        text_muted: 0x757373,
        border: 0xE8E8E8,
        focus_ring: 0x848281,
        danger: 0xEF4444,
        destructive: 0xB42318,
        on_destructive: 0xFFFFFF,
        scrim: gpui_kit::Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 0.32,
        },
        link: 0x0066CC,
        accent_hover: 0x121212,
        on_accent: 0xFFFFFF,
        window: Fill::solid(0xFBFAF9),
        sidebar: Fill::solid(0xFBFAF9),
        page: Fill::solid(0xFFFFFF),
        player: Fill::solid(0xFFFFFF),
        tile: Fill::solid(0xF6F4EF),
        tile_hover: Fill::solid(0xECECF0),
        sidebar_rest: Fill::solid(0xFBFAF9),
        page_rest: Fill::solid(0xFFFFFF),
        design: Design::Classic,
        dark: false,
    };

    const DARK: Self = Self {
        canvas: 0x121212,
        surface: 0x1A1A1A,
        surface_raised: 0x292929,
        surface_hover: 0x242424,
        control: 0x303030,
        control_hover: 0x404040,
        card_hover: 0x39393D,
        selection: 0x183B56,
        text_primary: 0xF5F3EF,
        text: 0xD5D1CB,
        text_muted: 0xA09D99,
        border: 0x414141,
        focus_ring: 0xA8A5A1,
        danger: 0xF87171,
        destructive: 0xFF6961,
        on_destructive: 0x171717,
        scrim: gpui_kit::Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 0.56,
        },
        link: 0x2997FF,
        accent_hover: 0xFFFFFF,
        on_accent: 0x171717,
        window: Fill::solid(0x121212),
        sidebar: Fill::solid(0x121212),
        page: Fill::solid(0x1A1A1A),
        player: Fill::solid(0x1A1A1A),
        tile: Fill::solid(0x242426),
        tile_hover: Fill::solid(0x39393D),
        sidebar_rest: Fill::solid(0x121212),
        page_rest: Fill::solid(0x1A1A1A),
        design: Design::Classic,
        dark: true,
    };

    /// The palette for a design. Classic is the base unchanged. The others change
    /// the colours and the big structural fills; small controls, menus and
    /// dialogs keep opaque roles so text on them never depends on what is
    /// behind.
    fn with_design(mut self, design: Design, dark: bool) -> Self {
        self.design = design;
        match (design, dark) {
            (Design::Classic, _) => {}
            (Design::Refined, true) => {
                self.canvas = 0x101012;
                self.surface = 0x161618;
                self.surface_raised = 0x222226;
                self.surface_hover = 0x1F1F23;
                self.control = 0x26262A;
                self.control_hover = 0x303034;
                self.card_hover = 0x26262A;
                self.text_primary = 0xF1EFEB;
                self.text = 0xC9C7C3;
                self.text_muted = 0x8D8B88;
                self.border = 0x2A2A2E;
                self.window = Fill::solid(0x101012);
                self.sidebar = Fill::solid(0x101012);
                self.page = Fill::solid(0x161618);
                self.player = Fill::solid(0x101012);
                self.tile = Fill::solid(0x1E1E21);
                self.tile_hover = Fill::solid(0x26262A);
                self.sidebar_rest = Fill::solid(0x101012);
                self.page_rest = Fill::solid(0x161618);
            }
            (Design::Refined, false) => {
                self.canvas = 0xF7F6F4;
                self.window = Fill::solid(0xF7F6F4);
                self.sidebar = Fill::solid(0xF7F6F4);
                self.sidebar_rest = Fill::solid(0xF7F6F4);
            }
            (Design::Ambient | Design::Crate, true) => {
                let crate_ = design == Design::Crate;
                let base = if crate_ { 0x0F0E12 } else { 0x121114 };
                self.canvas = base;
                self.surface = base;
                self.surface_raised = 0x232027;
                self.surface_hover = 0x1D1A21;
                self.control = 0x2A2730;
                self.control_hover = 0x36323D;
                self.card_hover = 0x36323D;
                self.text_primary = 0xF3F1F4;
                self.text = 0xCDC9D2;
                self.text_muted = if crate_ { 0x9D98A6 } else { 0xA8A3AE };
                self.border = 0x2A2730;
                let none = Fill::glass(0, 0.);
                self.window = Fill::solid(base);
                self.sidebar = none;
                self.page = none;
                self.player = none;
                self.tile = Fill::glass(0xFFFFFF, if crate_ { 0.05 } else { 0.06 });
                self.tile_hover = Fill::glass(0xFFFFFF, 0.10);
                self.sidebar_rest = none;
                self.page_rest = none;
            }
            (Design::Ambient | Design::Crate, false) => {
                let none = Fill::glass(0, 0.);
                self.window = Fill::solid(self.surface);
                self.sidebar = none;
                self.page = none;
                self.player = none;
                self.tile = Fill::glass(0x000000, 0.04);
                self.tile_hover = Fill::glass(0x000000, 0.07);
                self.sidebar_rest = none;
                self.page_rest = none;
            }
        }
        self
    }
}

fn is_dark_appearance(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

fn resolve_dark_mode(preference: ThemePreference, appearance: WindowAppearance) -> bool {
    match preference {
        ThemePreference::System => is_dark_appearance(appearance),
        ThemePreference::Light => false,
        ThemePreference::Dark => true,
    }
}
const VOLUME_SLIDER_WIDTH: Rems = Rems(7.5);
const VOLUME_SLIDER_RIGHT_INSET: Rems = Rems(9.);
const PLAYER_LEFT_WIDTH: Rems = Rems(22.5);
const PLAYER_CENTER_WIDTH: Rems = Rems(27.5);
const PLAYER_RIGHT_WIDTH: Rems = Rems(15.);
const PROGRESS_SLIDER_WIDTH: Rems = Rems(21.25);
const PROGRESS_TIME_WIDTH: Rems = Rems(2.25);
const PROGRESS_GAP: Rems = Rems(0.5);
/// The width the compact player keeps for everything but the progress slider,
/// and the narrowest that slider gets.
const COMPACT_PLAYER_RESERVED_WIDTH: Rems = Rems(31.25);
const COMPACT_PROGRESS_SLIDER_MIN_WIDTH: Rems = Rems(10.);
const COMPACT_BREAKPOINT: Rems = Rems(60.);
const COMPACT_PLAYER_BREAKPOINT: Rems = Rems(71.);
/// The collapsed rail; the traffic-light cluster is positioned so its centre
/// sits on this rail's axis.
const COLLAPSED_SIDEBAR_WIDTH: Rems = Rems(4.875);
const EXPANDED_SIDEBAR_WIDTH: Rems = Rems(14.5);
const COMPACT_EXPANDED_SIDEBAR_WIDTH: Rems = Rems(12.5);
/// The sidebar container's padding; the top is overridden to clear the
/// traffic lights.
const SIDEBAR_CONTENT_PAD: Rems = Rems(1.);
/// The brand row's expanded leading padding and its logo size.
const BRAND_ROW_PAD: Rems = Rems(0.875);
const BRAND_LOGO_SIZE: Rems = Rems(2.);
/// A nav row's expanded leading padding, and the width of its glyph: the
/// glyph itself, not the 20pt box that holds it, which is the trap here.
const NAV_ROW_PAD: Rems = Rems(0.75);
const NAV_GLYPH_WIDTH: Rems = Rems(1.0625);
/// Span of the traffic-light cluster, close button through zoom (60pt on
/// macOS 26). The buttons' sizes and spacing are AppKit metrics; their origin
/// is ours via `traffic_light_position`.
const TRAFFIC_LIGHT_CLUSTER_WIDTH: Pixels = px(60.);
/// The cluster's top inset, matching the OS default for this window style.
const TRAFFIC_LIGHT_INSET_Y: Pixels = px(9.);
/// The hover-and-selection pill behind a collapsed sidebar row.
const SIDEBAR_FILL_COLLAPSED: Rems = Rems(2.625);
/// How far the collapsed pill sits in from the row's left edge.
const SIDEBAR_FILL_INSET: Rems = Rems(0.125);
const CATALOG_STALE_TIME: Duration = Duration::from_secs(5 * 60);
const COMPACT_PLAYER_LEFT_WIDTH: Rems = Rems(13.75);
const COMPACT_PLAYER_RIGHT_WIDTH: Rems = Rems(6.);

fn sidebar_transition_duration(
    current_width: Rems,
    target_width: Rems,
    expanded_width: Rems,
) -> Duration {
    let remaining_distance = (target_width - current_width).0.abs();
    let full_distance = (expanded_width - COLLAPSED_SIDEBAR_WIDTH).0;
    let remaining_fraction = (remaining_distance / full_distance).clamp(0., 1.);
    Duration::from_millis((180. * remaining_fraction).round().max(60.) as u64)
}

/// Leading padding that puts a row's leading content on the collapsed rail
/// axis at progress 0 and back on its expanded padding at progress 1.
fn sidebar_row_pad(expanded_pad: Rems, content_width: Rems, progress: f32) -> Rems {
    let collapsed = COLLAPSED_SIDEBAR_WIDTH / 2. - SIDEBAR_CONTENT_PAD - content_width / 2.;
    collapsed + (expanded_pad - collapsed) * progress
}

/// The pill behind a sidebar row: a content-hugging box when collapsed, the
/// full row when expanded. Returns (width, left inset, leading padding).
fn sidebar_fill_geometry(
    expanded_pad: Rems,
    content_width: Rems,
    row_width: Rems,
    progress: f32,
) -> (Rems, Rems, Rems) {
    let left = SIDEBAR_FILL_INSET * (1. - progress);
    let width = SIDEBAR_FILL_COLLAPSED + (row_width - SIDEBAR_FILL_COLLAPSED) * progress;
    let pad = sidebar_row_pad(expanded_pad, content_width, progress) - left;
    (width, left, pad)
}

/// Close-button origin that centres the traffic-light cluster on the
/// collapsed rail axis, rather than trusting the OS default inset to land
/// there. AppKit places the cluster once, at the rem size the window opens
/// with.
fn traffic_light_position(rem_size: Pixels) -> gpui_kit::Point<Pixels> {
    let rail_width = COLLAPSED_SIDEBAR_WIDTH.to_pixels(rem_size);
    point(
        (rail_width - TRAFFIC_LIGHT_CLUSTER_WIDTH) / 2.,
        TRAFFIC_LIGHT_INSET_Y,
    )
}

fn uses_compact_content_layout(viewport_width: Pixels, rem_size: Pixels) -> bool {
    viewport_width < COMPACT_BREAKPOINT.to_pixels(rem_size)
}

fn is_compact_content_layout(window: &Window) -> bool {
    uses_compact_content_layout(window.viewport_size().width, window.rem_size())
}

fn uses_compact_player_layout(viewport_width: Pixels, rem_size: Pixels) -> bool {
    viewport_width < COMPACT_PLAYER_BREAKPOINT.to_pixels(rem_size)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Route {
    Home,
    LikedSongs,
    Recent,
    LocalFiles,
    Search,
    Playlists,
    Playlist,
    Artist,
    Album,
    Profile,
    Settings,
    WhatsNew,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionState {
    Starting,
    Failed,
    SetupRequired,
    AuthorizationRequired,
    Connecting,
    /// Signed in with a free account, which cannot stream through Sonder.
    PremiumRequired,
    Ready,
}

fn volume_for_pointer(pointer_x: Pixels, window_width: Pixels, rem_size: Pixels) -> f32 {
    let slider_start = window_width - VOLUME_SLIDER_RIGHT_INSET.to_pixels(rem_size);
    let slider_width = VOLUME_SLIDER_WIDTH.to_pixels(rem_size);
    ((pointer_x - slider_start) / slider_width).clamp(0., 1.)
}

fn format_duration(duration_ms: u32) -> String {
    let seconds = duration_ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn next_request_id(request_id: &mut u64) -> u64 {
    *request_id = request_id.wrapping_add(1);
    *request_id
}

/// Wall clock, not `Instant`: `Instant` does not advance while the machine is
/// asleep, so a sleep would leave stale data looking fresh.
fn catalog_data_is_fresh(loaded_at: Option<SystemTime>) -> bool {
    loaded_at.is_some_and(|loaded_at| {
        loaded_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed < CATALOG_STALE_TIME)
    })
}

async fn receive_backend_event_batch(
    events: &mut crate::backend::EventReceiver,
) -> Option<Vec<BackendEvent>> {
    let first = events.recv().await?;
    let mut batch = vec![first];
    while batch.len() < 128 {
        let Ok(event) = events.try_recv() else {
            break;
        };
        batch.push(event);
    }
    Some(batch)
}

/// Stream of backend events from the worker thread.
type BackendEvents = crate::backend::EventReceiver;

mod actions;
mod appearance;
mod artist_header;
mod assets;
mod bootstrap;
mod cached_search;
mod catalog;
mod chrome;
mod components;
mod credits_dialog;
mod events;
mod fonts;
mod icons;
mod launch_intro;
mod library;
mod library_pages;
mod link_downloads;
mod link_popover;
mod lyrics_view;
mod media_controls;
mod onboarding;
mod page;
mod playback_clock;
mod playback_help;
mod player;
mod player_bar;
mod profile;
mod release_watch;
mod review;
mod review_image;
mod router;
mod scrubber;
mod services;
mod session;
mod settings;
mod sidebar;
mod smooth_scroll;
mod soloist_setup;
mod tokens;
mod top_listening;
mod track_list;
mod track_row;
#[cfg(target_os = "macos")]
#[path = "updater_macos.rs"]
mod updater;
#[cfg(target_os = "windows")]
#[path = "updater_windows.rs"]
mod updater;
mod whats_new;
mod windows;
mod workspace;

pub fn run() {
    bootstrap::run();
}

#[cfg(test)]
mod event_bridge_tests {
    use super::{
        BackendEvent, CATALOG_STALE_TIME, catalog_data_is_fresh, next_request_id,
        receive_backend_event_batch,
    };
    use std::time::{Duration, SystemTime};

    #[tokio::test]
    async fn batches_events_that_are_already_queued() {
        let (_shutdown, shutdown_receiver) = tokio::sync::watch::channel(false);
        let (sender, mut receiver) = crate::backend::event_channel(shutdown_receiver);
        sender.send(BackendEvent::SetupRequired).await.unwrap();
        sender
            .send(BackendEvent::CatalogReady { generation: 0 })
            .await
            .unwrap();

        let events = receive_backend_event_batch(&mut receiver).await.unwrap();

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], BackendEvent::SetupRequired));
        assert!(matches!(
            events[1],
            BackendEvent::CatalogReady { generation: 0 }
        ));
    }

    #[tokio::test]
    async fn closes_after_all_senders_are_dropped() {
        let (_shutdown, shutdown_receiver) = tokio::sync::watch::channel(false);
        let (sender, mut receiver) = crate::backend::event_channel(shutdown_receiver);
        drop(sender);

        assert!(receive_backend_event_batch(&mut receiver).await.is_none());
    }

    #[test]
    fn generations_advance_with_wrapping_request_ids() {
        let mut generation = u64::MAX;
        assert_eq!(next_request_id(&mut generation), 0);
        assert_eq!(next_request_id(&mut generation), 1);
    }

    #[test]
    fn catalog_data_expires_after_the_stale_time() {
        assert!(!catalog_data_is_fresh(None));
        assert!(catalog_data_is_fresh(Some(SystemTime::now())));
        assert!(!catalog_data_is_fresh(Some(
            SystemTime::now() - CATALOG_STALE_TIME - Duration::from_secs(1)
        )));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{
        BRAND_LOGO_SIZE, BRAND_ROW_PAD, COLLAPSED_SIDEBAR_WIDTH, NAV_GLYPH_WIDTH, NAV_ROW_PAD,
        SIDEBAR_CONTENT_PAD, SIDEBAR_FILL_COLLAPSED, SIDEBAR_FILL_INSET,
        TRAFFIC_LIGHT_CLUSTER_WIDTH, resolve_dark_mode, sidebar_fill_geometry, sidebar_row_pad,
        sidebar_transition_duration, traffic_light_position, uses_compact_content_layout,
        uses_compact_player_layout, volume_for_pointer,
    };
    use crate::storage::ThemePreference;
    use gpui_kit::{Pixels, Rems, WindowAppearance, px};

    const DEFAULT_REM_SIZE: Pixels = px(16.);

    fn px_at_default_rem(length: Rems) -> f32 {
        f32::from(length.to_pixels(DEFAULT_REM_SIZE))
    }

    fn rems_from_px(pixels: f32) -> Rems {
        Rems(pixels / f32::from(DEFAULT_REM_SIZE))
    }

    #[test]
    fn theme_preference_resolves_against_window_appearance() {
        assert!(resolve_dark_mode(
            ThemePreference::System,
            WindowAppearance::Dark
        ));
        assert!(!resolve_dark_mode(
            ThemePreference::System,
            WindowAppearance::Light
        ));
        assert!(!resolve_dark_mode(
            ThemePreference::Light,
            WindowAppearance::Dark
        ));
        assert!(resolve_dark_mode(
            ThemePreference::Dark,
            WindowAppearance::Light
        ));
    }

    #[test]
    fn pointer_position_is_clamped_to_volume_range() {
        let window_width = px(1280.);
        let volume_at =
            |pointer_x| volume_for_pointer(px(pointer_x), window_width, DEFAULT_REM_SIZE);

        assert_eq!(volume_at(1100.), 0.);
        assert_eq!(volume_at(1196.), 0.5);
        assert_eq!(volume_at(1300.), 1.);
    }

    #[test]
    fn responsive_breakpoints_are_exclusive() {
        assert!(uses_compact_content_layout(px(959.), DEFAULT_REM_SIZE));
        assert!(!uses_compact_content_layout(px(960.), DEFAULT_REM_SIZE));
        assert!(uses_compact_player_layout(px(1135.), DEFAULT_REM_SIZE));
        assert!(!uses_compact_player_layout(px(1136.), DEFAULT_REM_SIZE));
    }

    #[test]
    fn sidebar_transition_duration_scales_with_remaining_distance() {
        let expanded = rems_from_px(232.);
        let duration_from = |width| sidebar_transition_duration(width, expanded, expanded);
        assert_eq!(duration_from(COLLAPSED_SIDEBAR_WIDTH).as_millis(), 180);
        assert_eq!(duration_from(rems_from_px(155.)).as_millis(), 90);
        assert_eq!(duration_from(rems_from_px(220.)).as_millis(), 60);
    }

    #[test]
    fn collapsed_sidebar_row_pads_match_the_verified_geometry() {
        // The measured values from cadence-5ym; a change to the rail width,
        // the content pad, or a row's content size must be a conscious one.
        assert_eq!(
            px_at_default_rem(sidebar_row_pad(BRAND_ROW_PAD, BRAND_LOGO_SIZE, 0.)),
            7.
        );
        assert_eq!(
            px_at_default_rem(sidebar_row_pad(NAV_ROW_PAD, NAV_GLYPH_WIDTH, 0.)),
            14.5
        );
    }

    #[test]
    fn traffic_lights_sit_on_the_collapsed_rail_axis() {
        let origin = traffic_light_position(DEFAULT_REM_SIZE).x;
        assert_eq!(
            origin + TRAFFIC_LIGHT_CLUSTER_WIDTH / 2.,
            COLLAPSED_SIDEBAR_WIDTH.to_pixels(DEFAULT_REM_SIZE) / 2.
        );
        // The cluster must also fit inside the rail, not just centre on it.
        assert!(origin >= px(0.));
    }

    #[test]
    fn collapsed_fill_centres_on_the_rail_axis() {
        // The pill hugs its content symmetrically only while its own centre
        // sits on the rail axis.
        assert_eq!(
            SIDEBAR_CONTENT_PAD + SIDEBAR_FILL_INSET + SIDEBAR_FILL_COLLAPSED / 2.,
            COLLAPSED_SIDEBAR_WIDTH / 2.
        );
    }

    #[test]
    fn sidebar_fill_hugs_content_collapsed_and_spans_the_row_expanded() {
        let row_width = rems_from_px(200.);
        let (width, left, pad) = sidebar_fill_geometry(NAV_ROW_PAD, NAV_GLYPH_WIDTH, row_width, 0.);
        assert_eq!((width, left), (SIDEBAR_FILL_COLLAPSED, SIDEBAR_FILL_INSET));
        assert_eq!(
            SIDEBAR_CONTENT_PAD + left + pad + NAV_GLYPH_WIDTH / 2.,
            COLLAPSED_SIDEBAR_WIDTH / 2.
        );

        let (width, left, pad) = sidebar_fill_geometry(NAV_ROW_PAD, NAV_GLYPH_WIDTH, row_width, 1.);
        assert_eq!((width, left, pad), (row_width, Rems::ZERO, NAV_ROW_PAD));
    }
}

#[cfg(test)]
mod design_tests {
    use super::*;

    fn base(dark: bool) -> CadencePalette {
        if dark {
            CadencePalette::DARK
        } else {
            CadencePalette::LIGHT
        }
    }

    #[test]
    fn classic_is_the_base_palette_and_fully_opaque() {
        for dark in [false, true] {
            let classic = base(dark).with_design(Design::Classic, dark);
            for fill in [
                classic.window,
                classic.sidebar,
                classic.page,
                classic.player,
                classic.tile,
                classic.tile_hover,
                classic.sidebar_rest,
                classic.page_rest,
            ] {
                assert_eq!(fill.alpha, 1.);
            }
            assert!(classic.page == base(dark).page && classic.canvas == base(dark).canvas);
            assert!(classic.design == Design::Classic);
        }
    }

    #[test]
    fn every_design_keeps_text_readable_on_its_own_surface() {
        let luminance =
            |rgb: u32| ((rgb >> 16) & 255) * 2126 + ((rgb >> 8) & 255) * 7152 + (rgb & 255) * 722;
        for design in Design::ALL {
            let p = base(true).with_design(design, true);
            assert!(luminance(p.text_primary) > luminance(p.surface) * 3);
            assert!(luminance(p.text_muted) > luminance(p.surface) * 2);
            assert!(p.design == design);
        }
    }

    #[test]
    fn ambient_and_crate_surfaces_are_see_through_over_their_backdrop() {
        for design in [Design::Ambient, Design::Crate] {
            for dark in [false, true] {
                let p = base(dark).with_design(design, dark);
                for fill in [p.sidebar, p.page, p.player, p.sidebar_rest, p.page_rest] {
                    assert_eq!(fill.alpha, 0.);
                }
                assert!(p.tile.alpha > 0. && p.tile.alpha < 1.);
            }
        }
    }
}
