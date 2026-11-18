//! Setting up Spotify's own player (Soloist), in a dialog over the window.
//!
//! Opened when Spotify won't stream to an account in the built-in player, or
//! from Settings. It asks for the listener's Soloist key if none is saved,
//! then follows what the backend reports: downloading Spotify's player, then
//! waiting for the listener to choose it in a Spotify app once. The backend
//! does the work; this only shows where it has got to.

use super::playback_help::{
    actions, badge, body, buttons, dialog_card, note, overlay, primary_button, quiet_link, title,
};
use super::*;
use crate::soloist::{self, Step};
use gpui_kit::TestSupportExt as _;

pub(super) enum SoloistSetupEvent {
    /// Go ahead with this key, or the saved one if there is none.
    Start(Option<String>),
    /// Give up; close the dialog.
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// Asking the backend whether a key is saved.
    Checking,
    /// Waiting for a key, or for another go.
    NeedKey {
        error: Option<SharedString>,
        /// Nothing needs pasting: the saved key can simply be tried again.
        retry_saved: bool,
    },
    Downloading,
    Pairing,
}

pub(super) struct SoloistSetup {
    phase: Phase,
    /// Made on the first draw, which is the first time there is a window to
    /// make it in.
    key: Option<Entity<InputState>>,
    _subscription: Option<Subscription>,
}

impl EventEmitter<SoloistSetupEvent> for SoloistSetup {}

impl SoloistSetup {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::Checking,
            key: None,
            _subscription: None,
        }
    }

    fn key_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(key) = &self.key {
            return key.clone();
        }
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Paste your Soloist key"));
        self._subscription = Some(cx.subscribe(
            &key,
            |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.start(cx),
                _ => {}
            },
        ));
        self.key = Some(key.clone());
        key
    }

    /// Follows the backend. `Done` is for the workspace, which closes this.
    pub(super) fn set_step(&mut self, step: Step, cx: &mut Context<Self>) {
        self.phase = match step {
            Step::NeedKey => Phase::NeedKey {
                error: None,
                retry_saved: false,
            },
            Step::Downloading => Phase::Downloading,
            Step::Pairing => Phase::Pairing,
            Step::Failed(message) => Phase::NeedKey {
                error: Some(message.into()),
                retry_saved: self.pasted_key(cx).is_none(),
            },
            Step::Done => return,
        };
        cx.notify();
    }

    fn pasted_key(&self, cx: &App) -> Option<String> {
        let key = self.key.as_ref()?.read(cx).value().trim().to_owned();
        (!key.is_empty()).then_some(key)
    }

    fn can_start(&self, cx: &App) -> bool {
        match &self.phase {
            Phase::NeedKey { retry_saved, .. } => *retry_saved || self.pasted_key(cx).is_some(),
            _ => false,
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let key = self.pasted_key(cx);
        self.phase = Phase::Checking;
        cx.emit(SoloistSetupEvent::Start(key));
        cx.notify();
    }

    fn working(
        palette: CadencePalette,
        id: &'static str,
        heading: &'static str,
        detail: &'static str,
    ) -> Stateful<Div> {
        dialog_card(palette, id)
            .child(
                div()
                    .size_10()
                    .rounded(tokens::CONTROL_RADIUS)
                    .bg(rgb(palette.surface_raised))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new().small()),
            )
            .child(title(palette, heading))
            .child(body(palette, detail))
    }

    fn cancel_row(&self, palette: CadencePalette, window: &Window, cx: &mut Context<Self>) -> Div {
        actions().child(
            quiet_link(palette, "soloist-setup-cancel", "Cancel", window)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SoloistSetupEvent::Cancel))),
        )
    }

    fn key_card(
        &self,
        palette: CadencePalette,
        error: Option<SharedString>,
        retry_saved: bool,
        input: &Entity<InputState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let can_start = self.can_start(cx);
        dialog_card(palette, "soloist-setup-key")
            .child(badge(palette, CadenceIcon::Speaker))
            .child(title(palette, "Set up Spotify’s player"))
            .child(body(
                palette,
                "Spotify’s own player may play songs that Sonder’s built-in one can’t. It runs quietly in the background, needs Premium, and takes a few minutes to set up once.",
            ))
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(step_line(
                        palette,
                        "1",
                        "Open the Spotify developer dashboard and create your Soloist key.",
                    ))
                    .child(step_line(palette, "2", "Paste it here."))
                    .child(
                        div()
                            .mt_1()
                            .child(Input::new(input).id("soloist-setup-key-input").w_full()),
                    ),
            )
            .when_some(error, |card, error| {
                card.child(
                    div()
                        .id("soloist-setup-error")
                        .mt_3()
                        .px_3()
                        .py_2p5()
                        .rounded(tokens::CONTROL_RADIUS)
                        .bg(rgb(palette.surface_raised))
                        .text_size(tokens::CAPTION_TEXT)
                        .line_height(relative(1.45))
                        .text_color(rgb(palette.text))
                        .child(error)
                        .test_support(),
                )
            })
            .child(note(
                palette,
                "Your key stays in your system keychain. Spotify’s player is downloaded from Spotify.",
            ))
            .child(
                actions()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                quiet_link(palette, "soloist-setup-cancel", "Cancel", window)
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SoloistSetupEvent::Cancel)
                                    })),
                            )
                            .child(
                                quiet_link(
                                    palette,
                                    "soloist-setup-dashboard",
                                    "Open the dashboard",
                                    window,
                                )
                                .on_click(|_, _, cx| cx.open_url(soloist::KEY_PAGE_URL)),
                            ),
                    )
                    .child(buttons().child({
                        let label = if retry_saved { "Try again" } else { "Continue" };
                        let button = primary_button(palette, "soloist-setup-continue", label)
                            .test_support();
                        if can_start {
                            button.on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                        } else {
                            button.opacity(0.45).cursor_default()
                        }
                    })),
            )
    }
}

fn step_line(palette: CadencePalette, number: &'static str, text: &'static str) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2p5()
        .text_sm()
        .line_height(relative(1.5))
        .child(
            div()
                .w_4()
                .flex_none()
                .text_color(rgb(palette.text_muted))
                .child(number),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(rgb(palette.text))
                .child(text),
        )
}

impl Render for SoloistSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let input = self.key_input(window, cx);
        let card = match self.phase.clone() {
            Phase::Checking => Self::working(
                palette,
                "soloist-setup-checking",
                "Getting ready…",
                "Checking what Sonder needs to start Spotify’s player.",
            )
            .child(self.cancel_row(palette, window, cx)),
            Phase::Downloading => Self::working(
                palette,
                "soloist-setup-downloading",
                "Downloading Spotify’s player",
                "This comes straight from Spotify and is about 10 MB.",
            )
            .child(self.cancel_row(palette, window, cx)),
            Phase::Pairing => Self::working(
                palette,
                "soloist-setup-pairing",
                "Choose Sonder in Spotify",
                "Open Spotify on your phone or another computer on this network, tap the devices icon, and choose “Sonder”. You only do this once.",
            )
            .child(self.cancel_row(palette, window, cx)),
            Phase::NeedKey { error, retry_saved } => {
                self.key_card(palette, error, retry_saved, &input, window, cx)
            }
        };
        overlay(palette, card)
    }
}
