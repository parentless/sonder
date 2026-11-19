//! The "What's new" page: a dated list of what each release added.
//!
//! The list is plain static data, so the page costs nothing until opened and
//! holds no timers or requests. Add the newest release at the top of [`MONTHS`]
//! when bumping the version.
use super::*;
use directories::ProjectDirs;
use gpui_kit::TestSupportExt as _;
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

struct Month {
    name: &'static str,
    days: &'static [Day],
}

use Tag::{Better, Fixed, New};

/// Newest first. Dates are the day each release was published. Add the next
/// release at the top of the first month when bumping the version.
const MONTHS: &[Month] = &[
    Month {
        name: "November 2026",
        days: &[
            Day {
                date: "Nov 18, 2026",
                releases: &[
                    Release {
                        version: "0.8.62",
                        notes: &[Note(
                            Better,
                            "A new search dropdown: cover art on every result, the letters you typed in bold, a quiet label on the right for songs, albums and artists, counts on each section, and placeholders while Spotify answers.",
                        )],
                    },
                    Release {
                        version: "0.8.61",
                        notes: &[Note(
                            Fixed,
                            "Saving a link that failed with an odd error, such as “Invalid argument”, now tries again in a plainer way before giving up, and a failed link shows its whole message.",
                        )],
                    },
                    Release {
                        version: "0.8.60",
                        notes: &[Note(
                            Fixed,
                            "Saving audio from a link works on far more pages. Sonder now looks through a page for the song itself, in a player, a download link or an embedded video, and checks it really is audio before saving. WAV saves no longer fail on songs that have cover art.",
                        )],
                    },
                    Release {
                        version: "0.8.59",
                        notes: &[Note(
                            Fixed,
                            "Local files play through Spotify’s own player too. They never needed Spotify, so they now play whichever player you use, and still play when Spotify can’t connect.",
                        )],
                    },
                    Release {
                        version: "0.8.58",
                        notes: &[
                            Note(
                                New,
                                "Spotify’s own player: when Spotify won’t stream to an account in Sonder’s built-in player, you can set up Spotify’s Soloist instead, or wait and try again. Settings has a Player choice too. It needs your own Soloist key from the Spotify developer dashboard, and picking “Sonder” in a Spotify app once.",
                            ),
                            Note(
                                Better,
                                "The refused-account message no longer blames accounts made since 2024. It says some accounts get refused for a while and others every time.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.57",
                        notes: &[
                            Note(
                                Better,
                                "Resume and Sample are smaller, quieter buttons with room around them, so they sit inside their card and row instead of out-weighing them.",
                            ),
                            Note(
                                Better,
                                "New from artists you play shows more albums as the window gets wider, instead of spreading three covers far apart.",
                            ),
                            Note(
                                Better,
                                "Your top tracks and the time range are gone from Home, which now ends after Worth a listen. Save as playlist stays on every album.",
                            ),
                        ],
                    },
                ],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[Release {
                    version: "0.8.56",
                    notes: &[
                        Note(
                            New,
                            "Save as playlist: one button on Home's Your top tracks and on every album makes a private Spotify playlist from its songs. Spotify asks for the playlist permission once, so sign out of Sonder and sign in again the first time.",
                        ),
                        Note(
                            Better,
                            "New from artists you play now also looks at artists you follow on Spotify, not just the ones you play most. That needs the same one-time sign in again to share your follows.",
                        ),
                        Note(
                            Better,
                            "Home lines up: the shortcut tiles, the Continue card, the new albums and the Worth a listen rows share the same left and right edges, and covers fill their tiles at any zoom. Your top artists is gone; the time range now sits on Your top tracks.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[Release {
                    version: "0.8.55",
                    notes: &[
                        Note(
                            Better,
                            "Home's opening rows now follow the new design closely: tiles with the cover flush to the edge, a play button that appears on hover for Liked Songs, Recently played and Local Files, a Continue card with a progress bar, and larger new-release covers.",
                        ),
                        Note(
                            Better,
                            "The artist to try is now a short row with a Sample button and a close button for Not for me. The announced release sits beside it.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[Release {
                    version: "0.8.54",
                    notes: &[
                        Note(
                            New,
                            "Home starts with a greeting and shortcuts back to your Liked Songs, what you played lately, your latest album, a playlist and your top artist, then the song to carry on with and new albums from artists you play.",
                        ),
                        Note(
                            New,
                            "Right-click a song and choose Credits to see who wrote, produced, mixed and played on it. Credits come from MusicBrainz, so newer songs may not have any yet.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[Release {
                    version: "0.8.53",
                    notes: &[Note(
                        Fixed,
                        "The red dot on your avatar now holds still when reduced motion is on.",
                    )],
                }],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[Release {
                    version: "0.8.52",
                    notes: &[
                        Note(
                            New,
                            "Home suggests an artist to try next, found because they sound like ones you play. Hear their top songs right there, open them, or tap Not for me and they never come back.",
                        ),
                        Note(
                            New,
                            "Delete a Local Files song from its right-click menu. It goes to the Recycle Bin, so you can get it back.",
                        ),
                        Note(
                            Better,
                            "The time range now sits beside Your top artists, which is what it changes. Starting a song from a long list is lighter, and the app file is smaller.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 17, 2026",
                releases: &[
                    Release {
                        version: "0.8.51",
                        notes: &[
                            Note(
                                Fixed,
                                "The From link dialog starts empty every time, and its Download button stays in view however many downloads are listed.",
                            ),
                            Note(Better, "The dialog fades in while it settles into place."),
                        ],
                    },
                    Release {
                        version: "0.8.50",
                        notes: &[Note(
                            Better,
                            "From link now opens as a dialog in the middle of the window, with the page dimmed behind it. Close it with the × button, Escape, or a click outside.",
                        )],
                    },
                ],
            },
            Day {
                date: "Nov 16, 2026",
                releases: &[Release {
                    version: "0.8.49",
                    notes: &[
                        Note(
                            New,
                            "Local Files has a From link button. Paste a link to a song and Sonder saves it into one of your folders as Best, MP3, FLAC, M4A, Ogg or WAV, with its cover and tags. Paste several links at once, or a playlist, and they save in turn.",
                        ),
                        Note(
                            Better,
                            "The first time you save from a page that isn’t a plain audio file, Sonder fetches the downloader and converter it needs and keeps them up to date. Direct links to audio files need nothing extra.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 15, 2026",
                releases: &[Release {
                    version: "0.8.48",
                    notes: &[
                        Note(
                            Fixed,
                            "Buttons, cards, rows and sliders now show the hand cursor when you hover them, so it is clear what you can click.",
                        ),
                        Note(
                            Better,
                            "The volume slider is cleaner: the dot only appears while you hover or drag the line, and it no longer has a border.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Nov 14, 2026",
                releases: &[
                    Release {
                        version: "0.8.47",
                        notes: &[Note(
                            Better,
                            "The intro now plays through every time it appears, so it can no longer be skipped by clicking or pressing Escape. It still only plays once.",
                        )],
                    },
                    Release {
                        version: "0.8.46",
                        notes: &[Note(
                            New,
                            "The short intro now plays once for everyone, the first time you open Sonder after this update, with no reset needed. Click or press Escape to skip it.",
                        )],
                    },
                    Release {
                        version: "0.8.45",
                        notes: &[
                            Note(
                                New,
                                "A short intro plays the very first time you open Sonder: a glow lands on the icon and opens into the app. Click or press Escape to skip it.",
                            ),
                            Note(
                                Fixed,
                                "Album placeholders on artist pages now fill the rest of a half-empty row before starting a new one.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.44",
                        notes: &[
                            Note(
                                New,
                                "Home is now a briefing: story cards for new releases, announced releases and artists to come back to, beside your top artists with arrows for who moved.",
                            ),
                            Note(
                                Better,
                                "Artist pages show placeholder albums while releases load, and while more load in.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.43",
                        notes: &[
                            Note(
                                New,
                                "Home shows your top artists as an orbit: bigger bubbles for higher ranks, with arrows for who is rising or falling.",
                            ),
                            Note(
                                Better,
                                "The time range on Home is one menu instead of three buttons.",
                            ),
                            Note(
                                New,
                                "Artist pages have a You tab with your own top and liked songs, and show where the artist ranks for you.",
                            ),
                            Note(
                                Better,
                                "The lyrics button is always in the player, and Space plays or pauses after you click any button.",
                            ),
                            Note(
                                Better,
                                "Skipping songs, saving your place and loading covers use less work.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.42",
                        notes: &[
                            Note(
                                Better,
                                "Sonder checks for updates once your library has loaded, so startup stays fast.",
                            ),
                            Note(
                                Better,
                                "An available update shows in the sidebar only; the avatar dot is just for new notes.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.41",
                        notes: &[
                            Note(
                                Better,
                                "Artist pages show 5 popular songs, with Show more for the rest.",
                            ),
                            Note(
                                Better,
                                "A cleaner What’s new page: no borders around the months.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.40",
                        notes: &[
                            Note(
                                New,
                                "What’s new now lives in the account menu, with a red dot on your avatar until you open it.",
                            ),
                            Note(
                                New,
                                "The full history back to the first release, folded by month.",
                            ),
                        ],
                    },
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
                            Note(
                                Better,
                                "Larger album cover, a loading skeleton and smoother lyric glide.",
                            ),
                        ],
                    },
                    Release {
                        version: "0.8.31",
                        notes: &[
                            Note(New, "Live Spotify results in the search dropdown."),
                            Note(Better, "A flat logo with no rim."),
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
                        version: "0.8.27",
                        notes: &[Note(
                            Fixed,
                            "Coming soon asks MusicBrainz once per artist and keeps partial results.",
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
                        notes: &[Note(
                            Better,
                            "The mascot is gone and the app has a new icon.",
                        )],
                    },
                    Release {
                        version: "0.8.22",
                        notes: &[Note(Better, "Renamed to Sonder, with a new ripple mark.")],
                    },
                    Release {
                        version: "0.8.21",
                        notes: &[Note(
                            New,
                            "Home: your top artists and tracks. The Favorites page is removed.",
                        )],
                    },
                    Release {
                        version: "0.8.20",
                        notes: &[Note(
                            New,
                            "A Premium-required screen, volume normalization and a tray icon.",
                        )],
                    },
                ],
            },
            Day {
                date: "Nov 10, 2026",
                releases: &[
                    Release {
                        version: "0.8.19",
                        notes: &[Note(
                            New,
                            "Settings tabs, an artwork cache on disk and opt-in Last.fm scrobbling.",
                        )],
                    },
                    Release {
                        version: "0.8.17 – 0.8.18",
                        notes: &[Note(
                            Better,
                            "Discographies load on demand, newest first, and visible artwork loads first.",
                        )],
                    },
                    Release {
                        version: "0.8.16",
                        notes: &[
                            Note(New, "Gallery search and album-artist links."),
                            Note(Better, "Sonder exits when its window closes on Windows."),
                        ],
                    },
                    Release {
                        version: "0.8.14 – 0.8.15",
                        notes: &[Note(
                            Better,
                            "Less memory for audio and artwork, and a seamless artist backdrop.",
                        )],
                    },
                ],
            },
            Day {
                date: "Nov 9, 2026",
                releases: &[
                    Release {
                        version: "0.8.12 – 0.8.13",
                        notes: &[
                            Note(New, "Cached search suggestions."),
                            Note(Fixed, "Rail tooltips and playlist recovery."),
                        ],
                    },
                    Release {
                        version: "0.8.10 – 0.8.11",
                        notes: &[Note(
                            Better,
                            "Smoother scrolling and steadier track navigation.",
                        )],
                    },
                    Release {
                        version: "0.8.8 – 0.8.9",
                        notes: &[
                            Note(New, "Spotify Likes and built-in Inter fonts."),
                            Note(Better, "Artists scroll as one page with a docking header."),
                        ],
                    },
                ],
            },
            Day {
                date: "Nov 8, 2026",
                releases: &[
                    Release {
                        version: "0.8.1 – 0.8.7",
                        notes: &[
                            Note(
                                New,
                                "Sidebar covers, row play controls and a saved UI zoom.",
                            ),
                            Note(New, "Lightweight shuffle and artist search."),
                            Note(Better, "Playlists load in pages as you scroll."),
                        ],
                    },
                    Release {
                        version: "0.8.0",
                        notes: &[Note(
                            New,
                            "The Windows version: native integration, a glass look and built-in updates.",
                        )],
                    },
                ],
            },
        ],
    },
    Month {
        name: "September 2026",
        days: &[
            Day {
                date: "Sep 28, 2026",
                releases: &[Release {
                    version: "0.7.2",
                    notes: &[Note(
                        Better,
                        "Every control reachable by keyboard, and reduced motion is honoured.",
                    )],
                }],
            },
            Day {
                date: "Sep 22, 2026",
                releases: &[
                    Release {
                        version: "0.7.1",
                        notes: &[Note(
                            Fixed,
                            "Audio recovers by itself after a Bluetooth route change.",
                        )],
                    },
                    Release {
                        version: "0.7.0",
                        notes: &[Note(
                            New,
                            "A draggable progress bar, and seeks land where you drop.",
                        )],
                    },
                ],
            },
            Day {
                date: "Sep 18, 2026",
                releases: &[Release {
                    version: "0.6.2",
                    notes: &[Note(
                        Fixed,
                        "History records a song once it plays, and unplayable songs are skipped instead of dropping the queue.",
                    )],
                }],
            },
            Day {
                date: "Sep 10, 2026",
                releases: &[
                    Release {
                        version: "0.6.1",
                        notes: &[Note(Better, "Updates stay manual.")],
                    },
                    Release {
                        version: "0.6.0",
                        notes: &[
                            Note(New, "Built-in updates."),
                            Note(
                                New,
                                "Artwork, title and artists in the player link to their pages.",
                            ),
                        ],
                    },
                ],
            },
            Day {
                date: "Sep 8, 2026",
                releases: &[Release {
                    version: "0.5.0 – 0.5.1",
                    notes: &[Note(
                        Better,
                        "A bundled icon set and signed macOS releases.",
                    )],
                }],
            },
            Day {
                date: "Sep 4, 2026",
                releases: &[Release {
                    version: "0.4.3",
                    notes: &[Note(
                        Better,
                        "Your cached library shows at launch, and Play waits until playback connects.",
                    )],
                }],
            },
            Day {
                date: "Sep 1, 2026",
                releases: &[Release {
                    version: "0.4.2",
                    notes: &[Note(New, "Player mascots (since removed).")],
                }],
            },
        ],
    },
    Month {
        name: "August 2026",
        days: &[
            Day {
                date: "Aug 24, 2026",
                releases: &[Release {
                    version: "0.4.1",
                    notes: &[Note(Better, "Audio output switched from SDL to CPAL.")],
                }],
            },
            Day {
                date: "Aug 21, 2026",
                releases: &[Release {
                    version: "0.4.0",
                    notes: &[
                        Note(
                            New,
                            "Autoplay keeps going with similar songs when the queue ends (a setting).",
                        ),
                        Note(Fixed, "Play after the queue ends."),
                        Note(
                            Better,
                            "Calmer Spotify requests, and the library refreshes when you return.",
                        ),
                    ],
                }],
            },
            Day {
                date: "Aug 18, 2026",
                releases: &[Release {
                    version: "0.3.0",
                    notes: &[Note(
                        Better,
                        "The library refreshes when Cadence comes to the front, and the search box clears with its results.",
                    )],
                }],
            },
            Day {
                date: "Aug 16, 2026",
                releases: &[
                    Release {
                        version: "0.2.0",
                        notes: &[Note(Better, "Safer sign-in, storage and background tasks.")],
                    },
                    Release {
                        version: "0.1.0",
                        notes: &[Note(New, "The first release of Cadence.")],
                    },
                ],
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
    /// Which months are unfolded. Folded months build no rows at all.
    open: Vec<bool>,
}

impl WhatsNewPage {
    pub(super) fn new() -> Self {
        Self {
            scroll: gpui_kit::ScrollHandle::new(),
            open: MONTHS.iter().enumerate().map(|(i, _)| i == 0).collect(),
        }
    }

    /// Back to the newest entry, with only the newest month open, whenever
    /// the page is opened again.
    pub(super) fn reopened(&mut self, cx: &mut Context<Self>) {
        for (index, open) in self.open.iter_mut().enumerate() {
            *open = index == 0;
        }
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

/// The versions a month spans, such as "0.8.0 – 0.8.40", read from its notes.
fn month_range(month: &Month) -> String {
    let newest = month.days[0].releases[0].version;
    let oldest = month.days[month.days.len() - 1]
        .releases
        .last()
        .map_or("", |release| release.version);
    let high = newest.rsplit('–').next().unwrap_or(newest).trim();
    let low = oldest.split('–').next().unwrap_or(oldest).trim();
    format!("{low} – {high}")
}

/// How many releases a label such as "0.8.1 – 0.8.7" stands for.
fn release_count(version: &str) -> usize {
    let patch = |part: &str| {
        part.trim()
            .rsplit('.')
            .next()
            .and_then(|p| p.parse::<usize>().ok())
    };
    match version.split_once('–') {
        Some((first, last)) => match (patch(first), patch(last)) {
            (Some(first), Some(last)) if last >= first => last - first + 1,
            _ => 1,
        },
        None => 1,
    }
}

fn day_block(palette: CadencePalette, day: &'static Day, skip: usize) -> Option<Div> {
    let releases = &day.releases[skip..];
    if releases.is_empty() {
        return None;
    }
    Some(
        div()
            .relative()
            .pl_6()
            .pb_5()
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
            })),
    )
}

impl Render for WhatsNewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let newest = &MONTHS[0].days[0];
        let hero = &newest.releases[0];
        let months = MONTHS.iter().enumerate().map(|(index, month)| {
            let open = self.open[index];
            let count: usize = month
                .days
                .iter()
                .flat_map(|day| day.releases)
                .map(|release| release_count(release.version))
                .sum();
            div()
                .mb_3()
                .child(
                    components::bare_button(("whats-new-month", index))
                        .test_support()
                        .w_full()
                        .justify_start()
                        .gap_3()
                        .px_1()
                        .py_3p5()
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(month.name),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(tokens::BODY_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(format!("{count} updates · {}", month_range(month))),
                        )
                        .child(components::icon(
                            if open {
                                CadenceIcon::ChevronUp
                            } else {
                                CadenceIcon::ChevronDown
                            },
                            tokens::INLINE_ICON,
                            palette.text_muted,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open[index] = !this.open[index];
                            cx.notify();
                        })),
                )
                .when(open, |card| {
                    card.child(
                        div()
                            .px_4()
                            .pt_1()
                            .children(month.days.iter().enumerate().filter_map(
                                |(day_index, day)| {
                                    // The newest release is the card above.
                                    let skip = usize::from(index == 0 && day_index == 0);
                                    day_block(palette, day, skip)
                                },
                            )),
                    )
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
                                .mb_6()
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
                                                .child(newest.date),
                                        ),
                                )
                                .child(notes_list(palette, hero.notes)),
                        )
                        .child(div().children(months)),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_label_counts_every_release_it_covers() {
        assert_eq!(release_count("0.8.39"), 1);
        assert_eq!(release_count("0.8.32 – 0.8.34"), 3);
        assert_eq!(release_count("0.5.0 – 0.5.1"), 2);
    }

    #[test]
    fn the_newest_release_matches_the_package_version() {
        assert_eq!(
            MONTHS[0].days[0].releases[0].version,
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn each_month_names_the_versions_it_spans() {
        assert_eq!(
            month_range(&MONTHS[0]),
            format!("0.8.0 – {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(month_range(&MONTHS[MONTHS.len() - 1]), "0.1.0 – 0.4.1");
    }
}
