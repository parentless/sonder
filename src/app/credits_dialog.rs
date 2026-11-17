//! The credits for one song, in a dialog over the whole window.
//!
//! Opened from a song's menu. The lookup starts when the dialog is made and ends
//! with it: closing drops the request, so nothing is asked of MusicBrainz once
//! it is no longer wanted.

use super::*;
use crate::{backend::BackendCommand, credits::Credits};
use gpui_kit::TestSupportExt as _;

const DIALOG_WIDTH: Rems = Rems(26.);

pub(super) enum CreditsEvent {
    Close,
}

enum State {
    Loading,
    Ready(Credits),
    Failed,
}

pub(super) struct CreditsDialog {
    track: model::Track,
    backend: BackendHandle,
    state: State,
    request: Option<gpui_kit::Task<()>>,
}

impl EventEmitter<CreditsEvent> for CreditsDialog {}

impl CreditsDialog {
    pub(super) fn new(track: model::Track, backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let mut dialog = Self {
            track,
            backend,
            state: State::Loading,
            request: None,
        };
        dialog.load(cx);
        dialog
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(isrc) = self
            .track
            .isrc
            .clone()
            .filter(|isrc| !isrc.trim().is_empty())
        else {
            self.state = State::Ready(Credits::default());
            return;
        };
        self.state = State::Loading;
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadCredits {
            isrc,
            respond,
        });
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(credits) => State::Ready(credits),
                    Err(error) => {
                        log::debug!("credits: {error}");
                        State::Failed
                    }
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn person_row(palette: CadencePalette, person: &crate::credits::Person) -> Div {
        div()
            .flex()
            .items_baseline()
            .gap_2()
            .child(
                div()
                    .text_color(rgb(palette.text_primary))
                    .child(person.name.clone()),
            )
            .when_some(person.detail.clone(), |row, detail| {
                row.child(
                    div()
                        .min_w_0()
                        .text_size(tokens::CAPTION_TEXT)
                        .text_color(rgb(palette.text_muted))
                        .child(detail),
                )
            })
    }

    fn message(palette: CadencePalette, title: &str, detail: &str) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .py_4()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_primary))
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .text_color(rgb(palette.text_muted))
                    .child(detail.to_owned()),
            )
    }
}

impl Render for CreditsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let body: AnyElement = match &self.state {
            State::Loading => Self::message(
                palette,
                "Looking up credits…",
                "Asking MusicBrainz who worked on this song.",
            )
            .id("credits-loading")
            .test_support()
            .into_any_element(),
            State::Failed => Self::message(
                palette,
                "Couldn’t load credits",
                "MusicBrainz didn’t answer. Check your connection and try again.",
            )
            .child(
                components::settings_button(palette, "credits-retry", "Try again")
                    .test_support()
                    .mt_2()
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
            .into_any_element(),
            State::Ready(credits) if credits.is_empty() => Self::message(
                palette,
                "No credits yet",
                "MusicBrainz doesn’t list who made this song. Newer and smaller releases are often missing.",
            )
            .id("credits-empty")
            .test_support()
            .into_any_element(),
            State::Ready(credits) => div()
                .id("credits-list")
                .test_support()
                .flex()
                .flex_col()
                .gap_3()
                .children(credits.roles.iter().enumerate().map(|(index, role)| {
                    div()
                        .id(("credits-role", index))
                        .test_support()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(tokens::CAPTION_TEXT)
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_muted))
                                .child(role.label.to_uppercase()),
                        )
                        .children(role.people.iter().map(|person| Self::person_row(palette, person)))
                }))
                .into_any_element(),
        };
        div()
            .id("credits-dialog")
            .test_support()
            .occlude()
            .w(DIALOG_WIDTH)
            .max_h(window.viewport_size().height * 0.86)
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .shadow_lg()
            .text_color(rgb(palette.text))
            .text_size(tokens::BODY_TEXT)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette.text_primary))
                                    .child("Credits"),
                            )
                            .child(
                                div()
                                    .id("credits-song")
                                    .truncate()
                                    .text_size(tokens::CAPTION_TEXT)
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!("{} · {}", self.track.title, self.track.artist)),
                            ),
                    )
                    .child(
                        components::icon_button_sized(
                            palette,
                            "credits-close",
                            CadenceIcon::Close,
                            tokens::INLINE_ICON,
                        )
                        .test_support()
                        .size_8()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CreditsEvent::Close))),
                    ),
            )
            .child(
                div()
                    .id("credits-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .text_size(tokens::CAPTION_TEXT)
                    .text_color(rgb(palette.text_muted))
                    .child("Credits from MusicBrainz, written by volunteers."),
            )
    }
}
