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
    /// Set up Spotify's own player, which may play where the built-in one can't.
    SetUpSoloist,
    /// Go back to the built-in player.
    UseBuiltIn,
}

pub(super) fn playback_problem_dialog(
    palette: CadencePalette,
    problem: &PlaybackProblem,
    account_name: Option<SharedString>,
    engine: crate::playback::Engine,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Div {
    let card = match problem {
        PlaybackProblem::AccountRefused => account_refused(palette, account_name, window, act),
        PlaybackProblem::NotConnected { detail } => {
            not_connected(palette, detail, engine, window, act)
        }
        PlaybackProblem::AwaitingApproval { url } => awaiting_approval(palette, url, window, act),
    };
    overlay(palette, card)
}

/// Dims the window and sits a dialog card in its middle.
pub(super) fn overlay(palette: CadencePalette, card: Stateful<Div>) -> Div {
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
    let soloist = crate::soloist::supported();
    let use_another = act.clone();
    let set_up = act.clone();
    let dismiss = act;
    dialog_card(palette, "streaming-refused")
        .child(badge(palette, CadenceIcon::Lock))
        .child(title(palette, "Spotify won’t stream to this account"))
        .child(body(
            palette,
            format!(
                "Spotify is refusing to unlock songs for {who} in Sonder’s built-in player. Some accounts get this for a while and then it clears. Others get it every time."
            ),
        ))
        .child(note(
            palette,
            if soloist {
                "Spotify’s own player may play songs that Sonder’s can’t. Or wait a bit and try again. Your library, playlists and search still work here."
            } else {
                "Wait a bit and try again, or use another account. Your library, playlists and search still work here."
            },
        ))
        .child(
            div().mt_6().flex().justify_end().child(
                buttons()
                    .child(
                        components::settings_button(
                            palette,
                            "streaming-refused-later",
                            if soloist { "Try again later" } else { "OK" },
                        )
                        .test_support()
                        .on_click({
                            let dismiss = dismiss.clone();
                            move |_, _, cx| dismiss(PlaybackHelpAction::Dismiss, cx)
                        }),
                    )
                    .when(soloist, |row| {
                        row.child(
                            primary_button(
                                palette,
                                "streaming-refused-soloist",
                                "Set up Spotify’s player",
                            )
                            .test_support()
                            .on_click(move |_, _, cx| set_up(PlaybackHelpAction::SetUpSoloist, cx)),
                        )
                    }),
            ),
        )
        .child(
            div()
                .mt_4()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    quiet_link(palette, "streaming-refused-why", "Why this happens", window)
                        .on_click(|_, _, cx| cx.open_url(STREAMING_REFUSED_URL)),
                )
                .child(
                    quiet_link(
                        palette,
                        "streaming-refused-switch",
                        "Use another account",
                        window,
                    )
                    .on_click(move |_, _, cx| use_another(PlaybackHelpAction::UseAnotherAccount, cx)),
                ),
        )
}

fn not_connected(
    palette: CadencePalette,
    detail: &SharedString,
    engine: crate::playback::Engine,
    window: &Window,
    act: impl Fn(PlaybackHelpAction, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let soloist = engine == crate::playback::Engine::Soloist;
    let dismiss = act.clone();
    let retry = act.clone();
    let built_in = act.clone();
    let reset = act;
    dialog_card(palette, "playback-not-connected")
        .child(badge(palette, CadenceIcon::SpeakerMuted))
        .child(title(palette, "Playback isn’t connected"))
        .child(body(
            palette,
            if soloist {
                "Sonder can show your library, but it can’t play anything until Spotify’s player connects. Setting it up again checks your key and starts it afresh."
            } else {
                "Sonder can show your library, but it can’t play anything until Spotify’s player connects. Setting it up again opens a Spotify page to approve, and usually fixes this."
            },
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
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            quiet_link(palette, "playback-not-connected-later", "Not now", window)
                                .on_click(move |_, _, cx| dismiss(PlaybackHelpAction::Dismiss, cx)),
                        )
                        .when(soloist, |links| {
                            links.child(
                                quiet_link(
                                    palette,
                                    "playback-not-connected-built-in",
                                    "Use the built-in player",
                                    window,
                                )
                                .on_click(move |_, _, cx| {
                                    built_in(PlaybackHelpAction::UseBuiltIn, cx)
                                }),
                            )
                        }),
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

pub(super) fn dialog_card(palette: CadencePalette, id: &'static str) -> Stateful<Div> {
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
pub(super) fn badge(palette: CadencePalette, glyph: CadenceIcon) -> Div {
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

pub(super) fn title(palette: CadencePalette, text: &'static str) -> Div {
    div()
        .mt_4()
        .text_xl()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_primary))
        .child(text)
}

pub(super) fn body(palette: CadencePalette, text: impl Into<SharedString>) -> Div {
    div()
        .mt_2()
        .text_sm()
        .line_height(relative(1.5))
        .text_color(rgb(palette.text))
        .child(text.into())
}

pub(super) fn note(palette: CadencePalette, text: &'static str) -> Div {
    div()
        .mt_2()
        .text_sm()
        .line_height(relative(1.5))
        .text_color(rgb(palette.text_muted))
        .child(text)
}

/// A quiet way out on the left, the real choices on the right.
pub(super) fn actions() -> Div {
    div().mt_6().flex().items_center().justify_between().gap_4()
}

pub(super) fn buttons() -> Div {
    div().flex().items_center().gap_2()
}

pub(super) fn quiet_link(
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

pub(super) fn primary_button(
    palette: CadencePalette,
    id: &'static str,
    label: &'static str,
) -> Stateful<Div> {
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
