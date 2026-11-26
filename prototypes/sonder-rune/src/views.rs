//! The bigger pieces over and beside the pages: Home, Local Files, the lyrics
//! panel, credits, the song menu and the notes that float above the player.

use cadence::model::{Album, HomeRange, Track};
use rune::tok::motion;
use rune::{Color, El, ScrollSpec, Transition, col, el, row, spacer, text};

use crate::State;
use crate::icons::icon;
use crate::theme::{self, Theme, type_};
use crate::ui::{BAR_H, CORNER, RAIL, WIDE, clock, cover, cover_url, pill, quick};

// ───────────────────────────── layers over the shell ─────────────────────────────

/// Everything that floats: the profile menu, the song menu, credits, notes. Each is a
/// full-size layer that closes when the space around it is pressed.
pub fn layers(s: &State, t: &Theme) -> Vec<El> {
    let mut out = Vec::new();
    if s.lyrics.open {
        out.push(lyrics_panel(s, t));
    }
    if let Some(menu) = crate::ui::dropdown_layers(s, t) {
        out.extend(menu);
    }
    if let Some(menu) = &s.track_menu {
        out.push(track_menu(s, t, menu));
    }
    if let Some(c) = &s.credits {
        out.push(credits_dialog(s, t, c));
    }
    if let Some((note, _)) = &s.toast {
        out.push(
            el().key("toast")
                .pin(Some(0.0), None, Some(0.0), Some(BAR_H + 16.0))
                .h(40.0)
                .center()
                .transition(Transition::offset(0.0, 10.0))
                .child(
                    row()
                        .h(36.0)
                        .px(16.0)
                        .center()
                        .radius(18.0)
                        .fill(theme::c("#2f2f33"))
                        .child(text(note.clone()).font(type_::BODY).color(t.text).nowrap()),
                ),
        );
    }
    out
}

// ───────────────────────────── song menu ─────────────────────────────

fn track_menu(s: &State, t: &Theme, menu: &crate::TrackMenu) -> El {
    let track = menu.track.clone();
    let item = |key: &'static str, glyph: Option<&rune::Image>, label: &'static str| {
        row()
            .key(key)
            .w_full()
            .h(38.0)
            .px(10.0)
            .gap(10.0)
            .center_y()
            .radius(10.0)
            .hover(t.hover)
            .press_scale(0.98)
            .child(glyph.map_or_else(|| el().size(17.0, 17.0), |g| icon(g, 17.0, t.muted)))
            .child(text(label).font(type_::BODY).color(t.text).nowrap())
    };
    let i = s.icons.as_ref();
    let mut list = col()
        .key("track-menu")
        .pin(None, Some(menu.y.clamp(70.0, 380.0)), Some(56.0), None)
        .w(214.0)
        .padding(6.0)
        .gap(2.0)
        .radius(14.0)
        .fill(theme::c("#242427"))
        .box_shadow(rune::Shadow::core_animation(
            0.0,
            12.0,
            28.0,
            Color::BLACK.alpha(0.45),
        ))
        .transition(Transition::offset(0.0, -6.0))
        .child(item("tm-next", i.map(|i| &i.queue), "Play next").on_press({
            let track = track.clone();
            move |s: &mut State| s.play_next(track.clone())
        }))
        .child(
            item("tm-queue", i.map(|i| &i.queue), "Add to queue").on_press({
                let track = track.clone();
                move |s: &mut State| s.add_to_queue(track.clone())
            }),
        )
        .child(
            item("tm-credits", i.map(|i| &i.lyrics), "Credits").on_press({
                let track = track.clone();
                move |s: &mut State| s.open_credits(track.clone())
            }),
        );
    if let Some(id) = track.artists.first().and_then(|a| a.source_id.clone()) {
        list = list.child(
            item("tm-artist", i.map(|i| &i.note), "Go to artist").on_press(move |s: &mut State| {
                s.track_menu = None;
                s.go(crate::Page::Artist(id.clone()));
            }),
        );
    }
    if let Some(id) = track.album_ref.as_ref().and_then(|a| a.source_id.clone()) {
        list = list.child(
            item("tm-album", i.map(|i| &i.playlists), "Go to album").on_press(
                move |s: &mut State| {
                    s.track_menu = None;
                    s.go(crate::Page::Album(id.clone()));
                },
            ),
        );
    }
    el().key("track-menu-layer")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            el().key("track-menu-scrim")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.track_menu = None),
        )
        .child(list)
}

// ───────────────────────────── credits ─────────────────────────────

fn credits_dialog(s: &State, t: &Theme, c: &crate::CreditsState) -> El {
    let mut body = col().w_full().gap(14.0);
    if c.loading {
        body = body.child(
            text("Looking up who made this…")
                .font(type_::BODY)
                .color(t.muted),
        );
    } else if let Some(credits) = c.data.as_ref().filter(|d| !d.is_empty()) {
        for (n, role) in credits.roles.iter().take(8).enumerate() {
            let mut people = col().gap(2.0);
            for person in &role.people {
                let mut line = person.name.clone();
                if let Some(detail) = &person.detail {
                    line = format!("{line} · {detail}");
                }
                people = people.child(text(line).font(type_::BODY).color(t.text));
            }
            body = body.child(
                row()
                    .key(format!("cr-{n}"))
                    .w_full()
                    .gap(16.0)
                    .child(
                        text(role.label.clone())
                            .font(type_::LABEL)
                            .color(t.faint)
                            .w(110.0),
                    )
                    .child(people),
            );
        }
    } else {
        body = body.child(
            text(if c.track.isrc.is_none() {
                "This song has no recording code, so there are no credits to look up."
            } else {
                "No credits found for this song."
            })
            .font(type_::BODY)
            .color(t.muted),
        );
    }
    let card = col()
        .key("credits-card")
        .w(480.0)
        .padding(24.0)
        .gap(16.0)
        .radius(22.0)
        .fill(theme::c("#222226"))
        .box_shadow(rune::Shadow::core_animation(
            0.0,
            24.0,
            48.0,
            Color::BLACK.alpha(0.5),
        ))
        .transition(Transition::scale(0.96))
        .on_press(|_: &mut State| {})
        .child(
            row()
                .w_full()
                .gap(14.0)
                .center_y()
                .child(cover(s, Some(&c.track), 52.0, 8.0, t))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(text("Credits").font(type_::LABEL).color(t.faint))
                        .child(
                            text(c.track.title.to_string())
                                .font(type_::TITLE)
                                .color(t.text)
                                .nowrap()
                                .truncate(),
                        )
                        .child(
                            text(c.track.artist.to_string())
                                .font(type_::LABEL)
                                .color(t.muted)
                                .nowrap()
                                .truncate(),
                        ),
                ),
        )
        .child(body);
    el().key("credits-layer")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(Color::BLACK.alpha(0.5))
        .transition(Transition::opacity())
        .on_press(|s: &mut State| s.credits = None)
        .child(col().w_full().h_full().center().child(card))
}

// ───────────────────────────── lyrics ─────────────────────────────

/// The lyrics panel slides up over the page. Synced lyrics show the lines around the sung
/// one; as it changes the lines slide and the sung one grows and brightens.
fn lyrics_panel(s: &State, t: &Theme) -> El {
    let now = s.now.as_ref();
    let tint = theme::tint(now.map_or("", |n| n.title.as_ref()));
    let left = if s.wide { WIDE } else { RAIL };
    let head = row()
        .w_full()
        .px(28.0)
        .pt(18.0)
        .gap(14.0)
        .center_y()
        .child(cover(s, now, 48.0, 8.0, t))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(now.map_or_else(String::new, |n| n.title.to_string()))
                        .font(type_::TITLE)
                        .color(t.text)
                        .nowrap()
                        .truncate(),
                )
                .child(
                    text(now.map_or_else(String::new, |n| n.artist.to_string()))
                        .font(type_::LABEL)
                        .color(t.muted)
                        .nowrap()
                        .truncate(),
                ),
        )
        .child(
            pill("ly-credits", "Credits", false, t).on_press(|s: &mut State| {
                if let Some(track) = s.now.clone() {
                    s.open_credits(track);
                }
            }),
        )
        .child(
            row()
                .key("ly-close")
                .size(36.0, 36.0)
                .center()
                .radius(18.0)
                .hover(t.hover)
                .press_scale(0.92)
                .on_press(|s: &mut State| s.lyrics.open = false)
                .child(
                    s.icons
                        .as_ref()
                        .map_or_else(|| el().size(18.0, 18.0), |i| icon(&i.close, 18.0, t.muted)),
                ),
        );

    let status = |note: &str| {
        col()
            .key("ly-status")
            .grow(1.0)
            .center()
            .transition(Transition::opacity())
            .child(text(note.to_owned()).font(type_::BODY).color(t.muted))
    };
    let body = match (&s.lyrics.data, s.lyrics.loading) {
        (_, true) => status("Looking for lyrics…"),
        (None, false) => status("No lyrics found for this song."),
        (Some(l), false) if l.is_empty() => status("No lyrics found for this song."),
        (Some(l), false) if l.synced => synced_lines(s, t, l),
        (Some(l), false) => plain_lines(t, l),
    };

    el().key("lyrics")
        .pin(Some(left), Some(0.0), Some(0.0), Some(BAR_H))
        .fill(theme::c("#141418"))
        .clip_corners([CORNER, 0.0, 0.0, CORNER])
        .radial_gradient(
            [0.3, 0.1],
            [900.0, 700.0],
            [(0.0, tint.alpha(0.42)), (1.0, Color::TRANSPARENT)],
        )
        .transition(Transition::offset(0.0, 36.0))
        .child(col().w_full().h_full().child(head).child(body))
}

fn synced_lines(s: &State, t: &Theme, lyrics: &cadence::lyrics::Lyrics) -> El {
    let current = s.lyrics.line.unwrap_or(0);
    let first = current.saturating_sub(3);
    let last = (current + 5).min(lyrics.lines.len());
    let mut lines = col().w_full().max_w(760.0).gap(6.0);
    for i in first..last {
        let distance = i.abs_diff(current);
        let (alpha, size) = match distance {
            0 => (1.0, 1.0),
            1 => (0.5, 0.86),
            2 => (0.32, 0.86),
            3 => (0.2, 0.86),
            _ => (0.12, 0.86),
        };
        let start = lyrics.lines[i].start_ms;
        let words = lyrics.lines[i].text.trim();
        lines = lines.child(
            row()
                .key(format!("ly-{i}"))
                .w_full()
                .py(7.0)
                .radius(14.0)
                .origin(0.0, 0.5)
                .scale(size)
                .opacity(alpha)
                .motion(quick())
                .transition(Transition::opacity())
                .hover(Color::WHITE.alpha(0.05))
                .on_press(move |s: &mut State| {
                    if let Some(now) = &s.now {
                        s.seek_to(start as f32 / now.duration_ms.max(1) as f32);
                    }
                })
                .child(
                    text(if words.is_empty() { "♪" } else { words }.to_owned())
                        .font(rune::tok::TextStyle {
                            size: 30.0,
                            weight: 700.0,
                            ..type_::TITLE
                        })
                        .color(t.text),
                ),
        );
    }
    col()
        .key("ly-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(64.0)
        .justify_center()
        .child(lines)
}

fn plain_lines(t: &Theme, lyrics: &cadence::lyrics::Lyrics) -> El {
    let mut lines = col().w_full().max_w(760.0).gap(8.0).py(24.0);
    for (i, line) in lyrics.lines.iter().enumerate() {
        lines = lines.child(
            text(line.text.clone())
                .key(format!("pl-{i}"))
                .font(rune::tok::TextStyle {
                    size: 20.0,
                    weight: 600.0,
                    ..type_::TITLE
                })
                .color(t.text.alpha(0.85)),
        );
    }
    col()
        .key("ly-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .scroll_edge_fade(28.0)
        .child(col().w_full().px(64.0).child(lines))
}

// ───────────────────────────── home ─────────────────────────────

/// Home: where to go next, then what you have been playing.
pub fn home_body(s: &State, t: &Theme) -> El {
    let tile = |key: &'static str,
                label: &'static str,
                glyph: Option<&rune::Image>,
                go: fn(&mut State)| {
        row()
            .key(key)
            .h(64.0)
            .grow(1.0)
            .min_w(150.0)
            .px(16.0)
            .gap(12.0)
            .center_y()
            .radius(16.0)
            .fill(Color::WHITE.alpha(0.06))
            .hover(Color::WHITE.alpha(0.1))
            .press_scale(0.98)
            .on_press(go)
            .child(glyph.map_or_else(|| el().size(20.0, 20.0), |g| icon(g, 20.0, t.text)))
            .child(text(label).font(type_::BODY).color(t.text).nowrap())
    };
    let i = s.icons.as_ref();
    let mut shortcuts =
        row()
            .w_full()
            .gap(12.0)
            .child(tile("hm-liked", "Liked Songs", i.map(|i| &i.heart), |s| {
                s.go(crate::Page::Liked)
            }));
    shortcuts = shortcuts
        .child(tile(
            "hm-recent",
            "Recently played",
            i.map(|i| &i.recent),
            |s| s.go(crate::Page::Recent),
        ))
        .child(tile(
            "hm-playlists",
            "Playlists",
            i.map(|i| &i.playlists),
            |s| s.go(crate::Page::Playlists),
        ));
    if !s.local.folders.is_empty() {
        shortcuts = shortcuts.child(tile("hm-local", "Local Files", i.map(|i| &i.folder), |s| {
            s.go(crate::Page::Local)
        }));
    }

    let range = |key: &'static str, label: &'static str, r: HomeRange| {
        let on = s.home.range == r;
        row()
            .key(key)
            .h(30.0)
            .px(14.0)
            .center()
            .radius(15.0)
            .fill(if on { t.active } else { Color::TRANSPARENT })
            .hover(t.hover)
            .press_scale(0.97)
            .on_press(move |s: &mut State| s.set_home_range(r))
            .child(
                text(label)
                    .font(type_::LABEL)
                    .color(if on { t.text } else { t.muted }),
            )
    };
    let heading = |label: &'static str| text(label).font(type_::TITLE).color(t.text);

    let mut body = col().w_full().px(26.0).py(8.0).gap(22.0).child(shortcuts);

    let wait = |note: &str| text(note.to_owned()).font(type_::BODY).color(t.muted);
    // Top artists
    let mut artists = row().w_full().gap(14.0).flex_wrap();
    match &s.home.data {
        Some(data) if !data.artists.is_empty() => {
            for artist in data.artists.iter().take(10) {
                let id = artist.source_id.clone();
                artists = artists.child(
                    col()
                        .key(format!("ha-{}", artist.source_id))
                        .w(92.0)
                        .gap(8.0)
                        .items_center()
                        .radius(14.0)
                        .press_scale(0.97)
                        .on_press(move |s: &mut State| s.go(crate::Page::Artist(id.clone())))
                        .child(cover_url(s, artist.artwork_url.as_deref(), 84.0, 42.0, t))
                        .child(
                            text(artist.name.to_string())
                                .font(type_::LABEL)
                                .color(t.text)
                                .nowrap()
                                .truncate()
                                .w(90.0),
                        ),
                );
            }
        }
        Some(_) => artists = artists.child(wait("Nothing to show for this period yet.")),
        None if s.home.loading => artists = artists.child(wait("Loading your top artists…")),
        None => artists = artists.child(wait("Top artists appear once Spotify answers.")),
    }
    body = body
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(heading("Your top artists"))
                .child(spacer())
                .child(range("hr-weeks", "4 weeks", HomeRange::Weeks))
                .child(range("hr-months", "6 months", HomeRange::Months))
                .child(range("hr-all", "All time", HomeRange::AllTime)),
        )
        .child(artists);

    // Songs: your top ones, else what you played last.
    let (label, pool): (&'static str, Vec<Track>) = match &s.home.data {
        Some(d) if !d.tracks.is_empty() => {
            ("Your top songs", d.tracks.iter().take(8).cloned().collect())
        }
        _ => (
            "Jump back in",
            s.recent.tracks.iter().take(8).cloned().collect(),
        ),
    };
    if !pool.is_empty() {
        let shared: std::rc::Rc<Vec<Track>> = std::rc::Rc::new(pool);
        let mut list = col().w_full().gap(2.0);
        for (n, track) in shared.iter().enumerate() {
            let pool = shared.clone();
            list = list.child(
                row()
                    .key(format!("hs-{n}"))
                    .w_full()
                    .h(52.0)
                    .px(12.0)
                    .gap(12.0)
                    .center_y()
                    .radius(10.0)
                    .hover_group()
                    .hover(t.hover)
                    .on_double_press(move |s: &mut State| s.play_found((*pool).clone(), n))
                    .child(cover(s, Some(track), 38.0, 6.0, t))
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .child(
                                text(track.title.to_string())
                                    .font(type_::BODY)
                                    .color(t.text)
                                    .nowrap()
                                    .truncate(),
                            )
                            .child(
                                text(track.artist.to_string())
                                    .font(type_::LABEL)
                                    .color(t.muted)
                                    .nowrap()
                                    .truncate(),
                            ),
                    )
                    .child(
                        text(clock(track.duration_ms))
                            .font(type_::LABEL)
                            .color(t.muted),
                    )
                    .child(crate::ui::more_button(s, track, t)),
            );
        }
        body = body.child(heading(label)).child(list);
    }

    col()
        .key("home")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(body)
}

// ───────────────────────────── artist albums ─────────────────────────────

/// An artist page: their popular songs, then their albums.
pub fn artist_body(s: &State, t: &Theme, id: &str) -> El {
    let mut body = col().w_full().px(26.0).py(8.0).gap(14.0);
    let tracks = s.source().tracks;
    if !tracks.is_empty() {
        let pool: std::rc::Rc<Vec<Track>> =
            std::rc::Rc::new(tracks.iter().take(10).cloned().collect());
        let mut list = col().w_full().gap(2.0);
        for (n, track) in pool.iter().enumerate() {
            let pool = pool.clone();
            list = list.child(
                row()
                    .key(format!("ar-{n}"))
                    .w_full()
                    .h(52.0)
                    .px(12.0)
                    .gap(12.0)
                    .center_y()
                    .radius(10.0)
                    .hover_group()
                    .hover(t.hover)
                    .on_double_press(move |s: &mut State| s.play_found((*pool).clone(), n))
                    .child(
                        el().w(22.0)
                            .child(text((n + 1).to_string()).font(type_::LABEL).color(t.faint)),
                    )
                    .child(cover(s, Some(track), 38.0, 6.0, t))
                    .child(
                        col().grow(1.0).min_w(0.0).child(
                            text(track.title.to_string())
                                .font(type_::BODY)
                                .color(t.text)
                                .nowrap()
                                .truncate(),
                        ),
                    )
                    .child(
                        text(clock(track.duration_ms))
                            .font(type_::LABEL)
                            .color(t.muted),
                    )
                    .child(crate::ui::more_button(s, track, t)),
            );
        }
        body = body
            .child(text("Popular").font(type_::TITLE).color(t.text))
            .child(list);
    } else if s.source().error.is_some() {
        body = body.child(
            text("This artist could not be loaded.")
                .font(type_::BODY)
                .color(t.muted),
        );
    } else {
        body = body.child(text("Loading…").font(type_::BODY).color(t.muted));
    }
    if let Some(albums) = s.artist_albums.get(id).filter(|a| !a.is_empty()) {
        let mut grid = row().w_full().gap(18.0).flex_wrap();
        for album in albums.iter().take(24) {
            grid = grid.child(album_card(s, t, album));
        }
        body = body
            .child(text("Albums").font(type_::TITLE).color(t.text))
            .child(grid);
    }
    col()
        .key("artist")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(body)
}

fn album_card(s: &State, t: &Theme, album: &Album) -> El {
    let id = album.source_id.clone();
    col()
        .key(format!("ac-{}", album.source_id))
        .w(150.0)
        .gap(6.0)
        .radius(14.0)
        .press_scale(0.98)
        .on_press(move |s: &mut State| s.go(crate::Page::Album(id.clone())))
        .child(cover_url(s, album.artwork_url.as_deref(), 150.0, 14.0, t))
        .child(
            text(album.name.to_string())
                .font(type_::BODY)
                .color(t.text)
                .nowrap()
                .truncate(),
        )
        .child(
            text(
                album
                    .release_date
                    .clone()
                    .unwrap_or_default()
                    .chars()
                    .take(4)
                    .collect::<String>(),
            )
            .font(type_::LABEL)
            .color(t.muted),
        )
}

#[allow(dead_code)]
const _: fn() = || {
    let _ = motion::REFLOW;
};
