//! A small player in a window of its own, in two shapes: a square mini player
//! with the cover on top, and a slim strip for a corner of the screen.
//!
//! It shows what the main window's player already knows. It keeps no queue,
//! cover or clock of its own: it redraws when the shared player notifies,
//! which is when the song changes or a whole second passes.

use std::{cell::Cell, rc::Rc};

use super::*;
use gpui_kit::TestSupportExt as _;

/// The square shape. The bottom gap is deliberately generous so the buttons
/// do not sit tight against the window's edge.
pub(super) const MINI_SIZE: (f32, f32) = (340., 548.);
/// The strip.
pub(super) const STRIP_SIZE: (f32, f32) = (520., 124.);

/// The padding around everything in the square shape.
const MINI_PAD: f32 = 24.;
const MINI_BOTTOM: f32 = 32.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Shape {
    Mini,
    Strip,
}

impl Shape {
    pub(super) fn size(self) -> (f32, f32) {
        match self {
            Self::Mini => MINI_SIZE,
            Self::Strip => STRIP_SIZE,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Mini => Self::Strip,
            Self::Strip => Self::Mini,
        }
    }
}

/// Raised when the listener wants the main window back.
pub(super) struct ExpandRequested;

impl EventEmitter<ExpandRequested> for MiniPlayer {}

pub(super) struct MiniPlayer {
    player: Entity<player::Player>,
    shape: Shape,
    /// Where the progress bar was last drawn, for turning a click into a time.
    track_bounds: Rc<Cell<Bounds<Pixels>>>,
    image_cache: Entity<image_cache::BoundedImageCache>,
    _player_events: Subscription,
    _artwork_events: Subscription,
}

impl MiniPlayer {
    pub(super) fn new(shape: Shape, cx: &mut Context<Self>) -> Self {
        let player = services::AppServices::player(cx);
        let image_cache = services::AppServices::image_cache(cx);
        Self {
            _player_events: cx.observe(&player, |_, _, cx| cx.notify()),
            _artwork_events: cx.subscribe(
                &image_cache,
                |_, _, _: &image_cache::ArtworkInvalidated, cx| cx.notify(),
            ),
            player,
            shape,
            track_bounds: Rc::default(),
            image_cache,
        }
    }

    #[cfg(test)]
    pub(super) fn shape(&self) -> Shape {
        self.shape
    }

    /// Switches between the two shapes, resizing the window to fit.
    fn switch_shape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shape = self.shape.other();
        let (width, height) = self.shape.size();
        window.resize(size(px(width), px(height)));
        cx.notify();
    }

    fn button(
        &self,
        palette: CadencePalette,
        id: &'static str,
        glyph: CadenceIcon,
        selected: bool,
        on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        components::icon_button(palette, id, glyph)
            .when(selected, |button| button.bg(rgb(palette.selection)))
            .on_click(on_click)
            .test_support()
    }

    fn transport(
        &self,
        palette: CadencePalette,
        playing: bool,
        with_shuffle: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let player = self.player.read(cx);
        let shuffle = player.shuffle();
        let repeat = player.repeat();
        let shuffle_button = self.button(
            palette,
            "mini-shuffle",
            CadenceIcon::Shuffle,
            shuffle,
            cx.listener(|this, _, _, cx| {
                this.player
                    .update(cx, |player, cx| player.toggle_shuffle(cx))
            }),
        );
        let previous = self.button(
            palette,
            "mini-previous",
            CadenceIcon::SkipBack,
            false,
            cx.listener(|this, _, _, cx| this.player.update(cx, |player, cx| player.previous(cx))),
        );
        let next = self.button(
            palette,
            "mini-next",
            CadenceIcon::SkipForward,
            false,
            cx.listener(|this, _, _, cx| this.player.update(cx, |player, cx| player.next(cx))),
        );
        let repeat_button = self.button(
            palette,
            "mini-repeat",
            if repeat == model::Repeat::One {
                CadenceIcon::RepeatOne
            } else {
                CadenceIcon::Repeat
            },
            repeat != model::Repeat::Off,
            cx.listener(|this, _, _, cx| {
                this.player.update(cx, |player, cx| player.cycle_repeat(cx))
            }),
        );
        div()
            .flex()
            .items_center()
            .gap_2()
            .when(with_shuffle, |row| row.child(shuffle_button))
            .child(previous)
            .child(
                components::filled_button(palette, "mini-play")
                    .aria_label(if playing { "Pause" } else { "Play" })
                    .hover(|style| style.opacity(0.9))
                    .size_10()
                    .rounded_full()
                    .bg(rgb(palette.text_primary))
                    .child(components::icon(
                        if playing {
                            CadenceIcon::Pause
                        } else {
                            CadenceIcon::Play
                        },
                        tokens::PLAYBACK_ICON,
                        palette.on_accent,
                    ))
                    .test_support()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.player.update(cx, |player, cx| player.toggle(cx))
                    })),
            )
            .child(next)
            .child(repeat_button)
    }

    /// A thin bar with the times either side; a click seeks.
    fn progress(
        &self,
        palette: CadencePalette,
        position_ms: u32,
        duration_ms: u32,
        cx: &mut Context<Self>,
    ) -> Div {
        let fraction = if duration_ms > 0 {
            (position_ms as f32 / duration_ms as f32).clamp(0., 1.)
        } else {
            0.
        };
        let bounds = self.track_bounds.clone();
        let measure = self.track_bounds.clone();
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .text_size(tokens::CAPTION_TEXT)
            .text_color(rgb(palette.text_muted))
            .child(
                div()
                    .w(Rems(2.25))
                    .flex_none()
                    .text_right()
                    .child(format_duration(position_ms)),
            )
            .child(
                div()
                    .id("mini-progress")
                    .test_support()
                    .relative()
                    .flex_1()
                    .h(Rems(1.25))
                    .cursor_pointer()
                    .child(
                        // The bar is thin; the row above it is the click target.
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(Rems(0.5))
                            .h(Rems(0.25))
                            .rounded_full()
                            .bg(rgb(palette.control_hover))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(rgb(palette.text_primary)),
                            ),
                    )
                    .child(
                        gpui_kit::canvas(move |bounds, _, _| measure.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                            let track = bounds.get();
                            if duration_ms == 0 || track.size.width <= px(0.) {
                                return;
                            }
                            let along = ((event.position().x - track.left()) / track.size.width)
                                .clamp(0., 1.);
                            let target = (along * duration_ms as f32) as u32;
                            this.player.update(cx, |player, cx| player.seek(target, cx));
                        }),
                    ),
            )
            .child(
                div()
                    .w(Rems(2.25))
                    .flex_none()
                    .child(format_duration(duration_ms)),
            )
    }

    fn window_buttons(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let shape = self.shape;
        div()
            .flex()
            .items_center()
            .child(
                components::icon_button_sized(
                    palette,
                    "mini-shape",
                    if shape == Shape::Mini {
                        CadenceIcon::ChevronUp
                    } else {
                        CadenceIcon::ChevronDown
                    },
                    Rems(1.),
                )
                .test_support()
                .on_click(cx.listener(|this, _, window, cx| this.switch_shape(window, cx))),
            )
            .child(
                components::icon_button_sized(
                    palette,
                    "mini-expand",
                    CadenceIcon::ArrowUpRight,
                    Rems(1.),
                )
                .test_support()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ExpandRequested))),
            )
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let (track, playing, position_ms) = {
            let player = self.player.read(cx);
            (
                player.shared_now_playing(),
                player.playing(),
                player.position_ms(cx.background_executor().now()),
            )
        };
        let duration_ms = track.as_ref().map_or(0, |track| track.duration_ms);
        let title: SharedString = track
            .as_ref()
            .map_or("Nothing playing".into(), |track| track.title.clone());
        let artist: SharedString = track
            .as_ref()
            .map_or("".into(), |track| track.artist.clone());
        let artwork_url = track.as_ref().and_then(|track| track.artwork_url.clone());
        let _ = window;

        let root = div()
            .id("mini-player")
            .test_support()
            .size_full()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .font_family("Inter")
            .flex();
        match self.shape {
            Shape::Mini => root
                .flex_col()
                .px(px(MINI_PAD))
                .pt(px(12.))
                .pb(px(MINI_BOTTOM))
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .child(self.window_buttons(palette, cx)),
                )
                .child(components::fluid_artwork(
                    palette,
                    &self.image_cache,
                    artwork_url.as_deref(),
                    Rems(1.),
                    CadenceIcon::MusicNote,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(Rems(1.25))
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(title),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(tokens::BODY_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(artist),
                        ),
                )
                .child(self.progress(palette, position_ms, duration_ms, cx))
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .child(self.transport(palette, playing, true, cx)),
                )
                .into_any_element(),
            Shape::Strip => root
                .items_center()
                .px(px(16.))
                .gap(px(16.))
                .child(
                    div()
                        .w(px(88.))
                        .flex_none()
                        .child(components::fluid_artwork(
                            palette,
                            &self.image_cache,
                            artwork_url.as_deref(),
                            Rems(0.75),
                            CadenceIcon::MusicNote,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(tokens::BODY_TEXT)
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .text_color(rgb(palette.text_primary))
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(tokens::CAPTION_TEXT)
                                                .text_color(rgb(palette.text_muted))
                                                .child(artist),
                                        ),
                                )
                                .child(self.window_buttons(palette, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_center()
                                .child(self.transport(palette, playing, false, cx)),
                        )
                        .child(self.progress(palette, position_ms, duration_ms, cx)),
                )
                .into_any_element(),
        }
    }
}
