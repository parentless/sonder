//! The dialog for playback problems only the listener can act on: playback
//! that would not connect, a playback sign-in waiting in the browser, and an
//! account Spotify won't stream to.

use super::*;
use gpui_kit::TestSupportExt as _;
use player::PlaybackProblem;

/// Where the account restriction is explained in full.
const STREAMING_REFUSED_URL: &str = "https://github.com/librespot-org/librespot/issues/1649";

/// What the dialog's buttons ask for. The workspace carries them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlaybackHelpAction {
    Dismiss,
    Retry,
    Reset,
    UseAnotherAccount,
}

pub(super) fn playback_problem_dialog(
    palette: CadencePalette,
    problem: &PlaybackProblem,
    account_name: Option<SharedString>,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Div {
    let card = match problem {
        PlaybackProblem::AccountRefused => account_refused(palette, account_name, window, act),
        PlaybackProblem::NotConnected { detail } => not_connected(palette, detail, window, act),
        PlaybackProblem::AwaitingApproval { url } => awaiting_approval(palette, url, window, act),
    };
    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(palette.scrim)
        .flex()
        .items_center()
        .justify_center()
        .child(card.test_support())
}

fn account_refused(
    palette: CadencePalette,
    account_name: Option<SharedString>,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let who = account_name.map_or_else(
        || "this account".to_owned(),
        |name| format!("{name}’s account"),
    );
    let use_another = act.clone();
    let dismiss = act;
    dialog_card(palette, "streaming-refused")
        .child(badge(palette, CadenceIcon::Lock))
        .child(title(palette, "Spotify won’t stream to this account"))
        .child(body(
            palette,
            format!(
                "Spotify refuses to unlock songs for {who} in players like Sonder. It does this to many accounts made since 2024, and nothing on this computer can change it."
            ),
        ))
        .child(note(
            palette,
            "Your library, playlists and search still work here.",
        ))
        .child(
            actions()
                .child(
                    quiet_link(palette, "streaming-refused-why", "Why this happens", window)
                        .on_click(|_, _, cx| cx.open_url(STREAMING_REFUSED_URL)),
                )
                .child(
                    buttons()
                        .child(
                            components::settings_button(
                                palette,
                                "streaming-refused-switch",
                                "Use another account",
                            )
                            .test_support()
                            .on_click(move |_, _, cx| {
                                use_another(PlaybackHelpAction::UseAnotherAccount, cx)
                            }),
                        )
                        .child(
                            primary_button(palette, "streaming-refused-ok", "OK")
                                .test_support()
                                .on_click(move |_, _, cx| dismiss(PlaybackHelpAction::Dismiss, cx)),
                        ),
                ),
        )
}

fn not_connected(
    palette: CadencePalette,
    detail: &SharedString,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let dismiss = act.clone();
    let retry = act.clone();
    let reset = act;
    dialog_card(palette, "playback-not-connected")
        .child(badge(palette, CadenceIcon::SpeakerMuted))
        .child(title(palette, "Playback isn’t connected"))
        .child(body(
            palette,
            "Sonder can show your library, but it can’t play anything until Spotify’s player connects. Setting it up again opens a Spotify page to approve, and usually fixes this.",
        ))
        .when(!detail.is_empty(), |card| {
            card.child(
                div()
                    .id("playback-not-connected-detail")
                    .mt_4()
                    .px_3()
                    .py_2p5()
                    .rounded(tokens::CONTROL_RADIUS)
                    .bg(rgb(palette.surface_raised))
                    .text_size(tokens::CAPTION_TEXT)
                    .line_height(relative(1.45))
                    .text_color(rgb(palette.text_muted))
                    .overflow_hidden()
                    .max_h(gpui_kit::rems(4.5))
                    .child(detail.clone())
                    .test_support(),
            )
        })
        .child(
            actions()
                .child(
                    quiet_link(palette, "playback-not-connected-later", "Not now", window)
                        .on_click(move |_, _, cx| dismiss(PlaybackHelpAction::Dismiss, cx)),
                )
                .child(
                    buttons()
                        .child(
                            components::settings_button(
                                palette,
                                "playback-not-connected-retry",
                                "Try again",
                            )
                            .test_support()
                            .on_click(move |_, _, cx| retry(PlaybackHelpAction::Retry, cx)),
                        )
                        .child(
                            primary_button(palette, "playback-not-connected-reset", "Set up again")
                                .test_support()
                                .on_click(move |_, _, cx| reset(PlaybackHelpAction::Reset, cx)),
                        ),
                ),
        )
}

fn awaiting_approval(
    palette: CadencePalette,
    url: &SharedString,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let url = url.clone();
    dialog_card(palette, "playback-approval")
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
        .child(title(palette, "Approve playback in your browser"))
        .child(body(
            palette,
            "On the Spotify page that just opened, choose Agree. This closes by itself once Spotify confirms.",
        ))
        .child(
            actions()
                .child(
                    quiet_link(palette, "playback-approval-cancel", "Cancel", window)
                        .on_click(move |_, _, cx| act(PlaybackHelpAction::Dismiss, cx)),
                )
                .child(
                    buttons().child(
                        components::settings_button(
                            palette,
                            "playback-approval-reopen",
                            "Open the page again",
                        )
                        .test_support()
                        .on_click(move |_, _, cx| cx.open_url(url.as_ref())),
                    ),
                ),
        )
}

fn dialog_card(palette: CadencePalette, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .w(tokens::DIALOG_WIDTH)
        .p_6()
        .rounded_2xl()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .shadow_lg()
        .flex()
        .flex_col()
}

/// A quiet tile that names the kind of problem before the words do.
fn badge(palette: CadencePalette, glyph: CadenceIcon) -> Div {
    div()
        .size_10()
        .rounded(tokens::CONTROL_RADIUS)
        .bg(rgb(palette.surface_raised))
        .flex()
        .items_center()
        .justify_center()
        .child(components::icon(
            glyph,
            tokens::CONTROL_ICON,
            palette.text_primary,
        ))
}

fn title(palette: CadencePalette, text: &'static str) -> Div {
    div()
        .mt_4()
        .text_xl()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_primary))
        .child(text)
}

fn body(palette: CadencePalette, text: impl Into<SharedString>) -> Div {
    div()
        .mt_2()
        .text_sm()
        .line_height(relative(1.5))
        .text_color(rgb(palette.text))
        .child(text.into())
}

fn note(palette: CadencePalette, text: &'static str) -> Div {
    div()
        .mt_2()
        .text_sm()
        .line_height(relative(1.5))
        .text_color(rgb(palette.text_muted))
        .child(text)
}

/// A quiet way out on the left, the real choices on the right.
fn actions() -> Div {
    div().mt_6().flex().items_center().justify_between().gap_4()
}

fn buttons() -> Div {
    div().flex().items_center().gap_2()
}

fn quiet_link(
    palette: CadencePalette,
    id: &'static str,
    label: &'static str,
    window: &Window,
) -> gpui_kit::base::Button {
    components::link(palette, id, window)
        .text_size(tokens::BODY_TEXT)
        .text_color(rgb(palette.text_muted))
        .child(label)
}

fn primary_button(palette: CadencePalette, id: &'static str, label: &'static str) -> Stateful<Div> {
    components::filled_button(palette, id)
        .h_10()
        .px_3p5()
        .rounded(tokens::CONTROL_RADIUS)
        .bg(rgb(palette.text_primary))
        .text_size(tokens::BODY_TEXT)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.surface))
        .hover(|style| style.opacity(0.88))
        .child(label)
}
