//! The "What's new" page: a dated list of what each release added.
//!
//! The list is plain static data, so the page costs nothing until opened and
//! holds no timers or requests. Add the newest release at the top of [`DAYS`]
//! when bumping the version.
use super::*;
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

#[derive(Clone, Copy)]
enum Tag {
    New,
    Better,
    Fixed,
}

struct Note(Tag, &'static str);

struct Release {
    version: &'static str,
    notes: &'static [Note],
}

struct Day {
    date: &'static str,
    releases: &'static [Release],
}

use Tag::{Better, Fixed, New};

/// Newest first. Dates are the day each release was published.
const DAYS: &[Day] = &[
    Day {
        date: "Nov 14, 2026",
        releases: &[
            Release {
                version: "0.8.39",
                notes: &[
                    Note(New, "This What’s new page, in the sidebar and in Settings."),
                    Note(
                        New,
                        "An update card in the sidebar: update, or restart to finish, in one click.",
                    ),
                    Note(
                        New,
                        "Install updates automatically (off by default): downloads quietly while you listen and installs when you close Sonder.",
                    ),
                ],
            },
            Release {
                version: "0.8.38",
                notes: &[Note(
                    Better,
                    "Empty and error pages share one clear look: an icon, a title and what to do next.",
                )],
            },
            Release {
                version: "0.8.37",
                notes: &[Note(
                    Better,
                    "When Spotify refuses a song, Sonder now explains why instead of skipping through your queue.",
                )],
            },
            Release {
                version: "0.8.36",
                notes: &[
                    Note(
                        New,
                        "Album review: the next song unlocks once you have fully heard the current one.",
                    ),
                    Note(
                        Fixed,
                        "Copy image now pastes into Paint, Discord and Office.",
                    ),
                ],
            },
            Release {
                version: "0.8.35",
                notes: &[
                    Note(
                        Better,
                        "Artist name and cover glide out of the toolbar as you scroll.",
                    ),
                    Note(Better, "Lyrics use less memory while streaming."),
                ],
            },
        ],
    },
    Day {
        date: "Nov 13, 2026",
        releases: &[
            Release {
                version: "0.8.32 – 0.8.34",
                notes: &[
                    Note(
                        New,
                        "Lyrics in a side panel, a full page or a subtitle strip (off by default).",
                    ),
                    Note(New, "Live Spotify results in the search dropdown."),
                    Note(
                        Better,
                        "Larger album cover, a loading skeleton and smoother lyric glide.",
                    ),
                ],
            },
            Release {
                version: "0.8.30",
                notes: &[Note(
                    New,
                    "Local Files: play music from folders on this computer.",
                )],
            },
        ],
    },
    Day {
        date: "Nov 12, 2026",
        releases: &[
            Release {
                version: "0.8.29",
                notes: &[Note(
                    New,
                    "Album review mode with a shareable image, search filters and a top result.",
                )],
            },
            Release {
                version: "0.8.28",
                notes: &[Note(
                    Better,
                    "Remembers Spotify cooldowns across restarts so Home stops over-asking.",
                )],
            },
            Release {
                version: "0.8.26",
                notes: &[Note(
                    New,
                    "New-release watch: a NEW badge, a tray alert and a Coming soon row.",
                )],
            },
            Release {
                version: "0.8.25",
                notes: &[Note(
                    New,
                    "New from artists on Home, a streaming quality setting and Copy Spotify link.",
                )],
            },
        ],
    },
    Day {
        date: "Nov 11, 2026",
        releases: &[
            Release {
                version: "0.8.24",
                notes: &[
                    Note(
                        New,
                        "Home switches between the last 4 weeks, 6 months and all time.",
                    ),
                    Note(New, "Open on startup, and a 70–130% zoom scale."),
                ],
            },
            Release {
                version: "0.8.23",
                notes: &[Note(New, "A new app icon.")],
            },
        ],
    },
];

fn seen_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("dev", "twoducks", "cadence")?
            .config_dir()
            .join("whats-new-seen.txt"),
    )
}

/// Whether this version's notes have not been opened yet. One small read when
/// the sidebar is built. Tests always report unseen so screenshots are stable.
pub(super) fn unseen() -> bool {
    if cfg!(test) {
        return true;
    }
    seen_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .is_none_or(|seen| seen.trim() != env!("CARGO_PKG_VERSION"))
}

pub(super) fn mark_seen() {
    if cfg!(test) {
        return;
    }
    let Some(path) = seen_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = fs::write(path, env!("CARGO_PKG_VERSION")) {
        log::warn!("could not remember the opened notes: {error}");
    }
}

pub(super) struct WhatsNewPage {
    scroll: gpui_kit::ScrollHandle,
}

impl WhatsNewPage {
    pub(super) fn new() -> Self {
        Self {
            scroll: gpui_kit::ScrollHandle::new(),
        }
    }

    /// Back to the newest entry whenever the page is opened again.
    pub(super) fn reopened(&mut self, cx: &mut Context<Self>) {
        self.scroll
            .set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
        cx.notify();
    }
}

fn tag_chip(palette: CadencePalette, tag: Tag) -> Div {
    let light = palette.canvas == CadencePalette::LIGHT.canvas;
    let (label, fg, bg) = match (tag, light) {
        (New, true) => ("New", 0x1558B8, 0xDFEAFC),
        (Fixed, true) => ("Fixed", 0x1D7A45, 0xDCF1E4),
        (Better, true) => ("Better", 0x6B3FB0, 0xECE3F8),
        (New, false) => ("New", 0x7CB7FF, 0x1D3350),
        (Fixed, false) => ("Fixed", 0x8FD6A6, 0x1C3A28),
        (Better, false) => ("Better", 0xD9B4FF, 0x3A2A52),
    };
    div()
        .flex_none()
        .w(Rems(3.5))
        .flex()
        .justify_center()
        .py_0p5()
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(tokens::CAPTION_TEXT)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(label)
}

fn notes_list(palette: CadencePalette, notes: &'static [Note]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .children(notes.iter().map(|Note(tag, text)| {
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(div().pt_0p5().child(tag_chip(palette, *tag)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(tokens::BODY_TEXT)
                        .line_height(relative(1.5))
                        .text_color(rgb(palette.text))
                        .child(*text),
                )
        }))
}

impl Render for WhatsNewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let latest = &DAYS[0];
        let hero = &latest.releases[0];
        // The latest release is the card above, so the timeline starts after it.
        let days = DAYS.iter().enumerate().filter_map(|(index, day)| {
            let releases = if index == 0 {
                &day.releases[1..]
            } else {
                day.releases
            };
            (!releases.is_empty()).then(|| {
                div()
                    .relative()
                    .pl_6()
                    .pb_6()
                    .child(
                        div()
                            .absolute()
                            .left(Rems(0.28))
                            .top(Rems(0.9))
                            .bottom_0()
                            .w(px(1.))
                            .bg(rgb(palette.border)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(Rems(0.3125))
                            .size_2p5()
                            .rounded_full()
                            .bg(rgb(palette.text_primary)),
                    )
                    .child(
                        div()
                            .mb_2()
                            .text_color(rgb(palette.text_primary))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(day.date),
                    )
                    .children(releases.iter().map(|release| {
                        div()
                            .mb_3()
                            .child(
                                div()
                                    .mb_1()
                                    .text_size(tokens::CAPTION_TEXT)
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette.text_muted))
                                    .child(format!("Version {}", release.version)),
                            )
                            .child(notes_list(palette, release.notes))
                    }))
            })
        });
        div().size_full().flex().justify_center().child(
            div()
                .id("whats-new-page")
                .h_full()
                .w_full()
                .max_w(tokens::SETTINGS_MAX_WIDTH)
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .w_full()
                        .px_10()
                        .pt_7()
                        .pb_16()
                        .child(
                            div()
                                .mb_5()
                                .text_size(tokens::BODY_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child("Everything added to Sonder, newest first."),
                        )
                        .child(
                            div()
                                .mb_8()
                                .p_5()
                                .rounded_2xl()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface_raised))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2p5()
                                        .mb_2()
                                        .child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(rgb(palette.text_primary))
                                                .text_color(rgb(palette.canvas))
                                                .text_size(tokens::CAPTION_TEXT)
                                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                .child("Latest"),
                                        )
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette.text_primary))
                                                .child(hero.version),
                                        )
                                        .child(
                                            div()
                                                .text_size(tokens::BODY_TEXT)
                                                .text_color(rgb(palette.text_muted))
                                                .child(latest.date),
                                        ),
                                )
                                .child(notes_list(palette, hero.notes)),
                        )
                        .child(div().children(days)),
                ),
        )
    }
}
