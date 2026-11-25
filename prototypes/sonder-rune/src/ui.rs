//! The screens. Pure functions of `State`: nothing here changes anything except
//! through the handlers it hands to Rune.

use std::borrow::Cow;

use crate::theme::type_;
use cadence::model::Track;
use rune::tok::motion;
use rune::{
    Color, Cx, El, ScrollSpec, SurfaceFit, Transition, col, el, row, spacer, text, text_input,
};

use crate::icons::{Icons, icon};
use crate::theme::{self, Theme};
use crate::{Conn, Page, State};

const RAIL: f32 = 64.0;
const WIDE: f32 = 208.0;
const ROW_H: f32 = 56.0;
const BAR_H: f32 = 76.0;
const CORNER: f32 = 20.0;

pub fn root(s: &State, _: &mut Cx<State>) -> El {
    let t = theme::dark();
    let shell = col().w_full().h_full().fill(t.side);
    // Until there is a library to show (cached or fresh), the first screen is the
    // sign-in flow, not an empty app.
    if !s.loaded && s.conn != Conn::Ready {
        return shell.child(gate(s, &t));
    }
    let mut app = shell
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(sidebar(s, &t))
                .child(page(s, &t)),
        )
        .child(player_bar(s, &t));
    if s.np {
        app = app.child(now_playing(s, &t));
    }
    app
}

// ───────────────────────────── sign-in ─────────────────────────────

fn gate(s: &State, t: &Theme) -> El {
    let (title, body, action): (&str, &str, Option<El>) = match s.conn {
        Conn::Starting => ("Starting Sonder", "Checking your Spotify connection.", None),
        Conn::Setup => (
            "Set up Spotify",
            "Paste the Client ID from your Spotify developer app to get started.",
            Some(client_id_form(s, t)),
        ),
        Conn::Login => (
            "Connect Spotify",
            "Sonder opens your browser to approve the connection.",
            Some(
                pill("login", "Log in with Spotify", true, t).on_press(|s: &mut State| {
                    s.conn = Conn::Connecting;
                    s.message = None;
                    s.config_serial += 1;
                    let generation = s.config_serial;
                    s.send(cadence::backend::BackendCommand::Authenticate { generation });
                }),
            ),
        ),
        Conn::Connecting => ("Connecting", "Finish signing in in your browser.", None),
        Conn::Premium => (
            "Spotify Premium needed",
            "Sonder streams through Spotify Premium accounts.",
            None,
        ),
        Conn::Failed => ("Something went wrong", "Quit and open Sonder again.", None),
        Conn::Ready => ("Loading your library", "Your songs are on their way.", None),
    };
    let mut card = col()
        .w(420.0)
        .gap(10.0)
        .center_y()
        .items_center()
        .child(text(title).font(type_::TITLE).color(t.text))
        .child(text(body).font(type_::BODY).color(t.muted));
    if let Some(error) = &s.message {
        card = card.child(
            text(error.clone())
                .font(type_::LABEL)
                .color(theme::c("#ff8a8a")),
        );
    }
    if let Some(action) = action {
        card = card.child(el().h(8.0)).child(action);
    }
    col().w_full().h_full().center().fill(t.page).child(card)
}

fn client_id_form(s: &State, t: &Theme) -> El {
    let field = text_input(&s.client_id, type_::BODY.lh(22.0))
        .key("client-id")
        .label("Spotify Client ID")
        .single_line()
        .grow(1.0)
        .color(t.text)
        .placeholder("Client ID", t.faint)
        .on_edit(|s: &mut State| &mut s.client_id)
        .on_submit(|s: &mut State, _| submit_client_id(s));
    col()
        .w_full()
        .gap(12.0)
        .items_center()
        .child(
            row()
                .h(40.0)
                .w(360.0)
                .px(14.0)
                .center_y()
                .radius(20.0)
                .fill(t.pill)
                .child(field),
        )
        .child(
            pill("client-id-go", "Continue", true, t).on_press(|s: &mut State| submit_client_id(s)),
        )
}

fn submit_client_id(s: &mut State) {
    let id = s.client_id.text.trim().to_owned();
    if id.is_empty() {
        return;
    }
    s.config_serial += 1;
    let generation = s.config_serial;
    s.conn = Conn::Connecting;
    s.send(cadence::backend::BackendCommand::ConfigureSpotify {
        generation,
        client_id: id,
    });
}

fn pill(key: &'static str, label: &'static str, primary: bool, t: &Theme) -> El {
    let (fill, ink) = if primary {
        (theme::c("#f5f3ef"), theme::c("#111111"))
    } else {
        (t.pill, t.pill_text)
    };
    row()
        .key(key)
        .h(36.0)
        .px(18.0)
        .center()
        .radius(18.0)
        .fill(fill)
        .press_scale(0.97)
        .child(text(label).font(type_::BODY).color(ink))
}

// ───────────────────────────── sidebar ─────────────────────────────

fn sidebar(s: &State, t: &Theme) -> El {
    let icons = s.icons.as_ref();
    let nav = |key: &'static str, label: &'static str, glyph: Option<&rune::Image>, page: Page| {
        let on = s.page == page;
        let ink = if on { t.text } else { t.muted };
        let mut item = row()
            .key(key)
            .h(38.0)
            .px(13.0)
            .gap(12.0)
            .center_y()
            .radius(10.0)
            .fill(if on { t.active } else { Color::TRANSPARENT })
            .hover(t.hover)
            .press_scale(0.98)
            .on_press(move |s: &mut State| s.go(page.clone()));
        if let Some(glyph) = glyph {
            item = item.child(icon(glyph, 20.0, ink));
        }
        // The label exists only while the sidebar is wide; mounting and unmounting it
        // is what fades it, so there is no label animation of my own to keep in step.
        if s.wide {
            item = item.child(
                text(label)
                    .key(format!("{key}-label"))
                    .font(type_::BODY)
                    .color(ink)
                    .nowrap()
                    .transition(Transition::opacity()),
            );
        }
        item
    };

    let toggle = row()
        .key("rail-toggle")
        .size(30.0, 30.0)
        .center()
        .radius(8.0)
        .hover(t.hover)
        .on_press(|s: &mut State| s.wide = !s.wide)
        .child(match icons {
            Some(i) => icon(if s.wide { &i.back } else { &i.forward }, 18.0, t.muted),
            None => el().size(18.0, 18.0),
        });
    let brand = row()
        .h(44.0)
        .px(8.0)
        .gap(10.0)
        .center_y()
        .child(el().size(28.0, 28.0).radius(8.0).fill(t.tile))
        .child(if s.wide {
            text("Sonder")
                .key("brand")
                .font(type_::TITLE)
                .color(t.text)
                .nowrap()
                .transition(Transition::opacity())
        } else {
            el().key("brand")
        })
        .child(spacer())
        .child(if s.wide {
            toggle
        } else {
            el().key("rail-toggle-spacer")
        });

    let mut side = col()
        .key("sidebar")
        .w(if s.wide { WIDE } else { RAIL })
        .h_full()
        .padding(10.0)
        .gap(2.0)
        .fill(t.side)
        .clip_rounded(0.0)
        .motion(motion::REFLOW)
        .child(brand);
    if !s.wide {
        side = side.child(
            row().h(34.0).center().child(
                row()
                    .key("rail-toggle-collapsed")
                    .size(30.0, 30.0)
                    .center()
                    .radius(8.0)
                    .hover(t.hover)
                    .on_press(|s: &mut State| s.wide = true)
                    .child(icons.map_or_else(
                        || el().size(18.0, 18.0),
                        |i| icon(&i.forward, 18.0, t.muted),
                    )),
            ),
        );
    }
    let i = icons;
    side = side
        .child(nav(
            "nav-liked",
            "Liked Songs",
            i.map(|i| &i.heart),
            Page::Liked,
        ))
        .child(nav(
            "nav-playlists",
            "Playlists",
            i.map(|i| &i.playlists),
            Page::Playlists,
        ))
        .child(nav(
            "nav-recent",
            "Recently played",
            i.map(|i| &i.recent),
            Page::Recent,
        ));
    side.child(spacer())
}

// ─────────────────────────────── page ───────────────────────────────

fn page_title(s: &State) -> (String, String) {
    let source = s.source();
    match &s.page {
        Page::Liked => ("Liked Songs".into(), format!("{} songs", source.total)),
        Page::Recent => ("Recently played".into(), format!("{} songs", source.total)),
        Page::Playlists => (
            "Playlists".into(),
            format!("{} playlists", s.playlists.len()),
        ),
        Page::Playlist(id) => {
            let playlist = s.playlists.iter().find(|p| &p.source_id == id);
            (
                playlist.map_or_else(|| "Playlist".into(), |p| p.name.to_string()),
                playlist.map_or_else(String::new, |p| {
                    format!("{} · {} songs", p.owner, p.track_count)
                }),
            )
        }
    }
}

fn page(s: &State, t: &Theme) -> El {
    let (title, meta) = page_title(s);
    let tint = theme::tint(&title);
    let mut page = col()
        .key("page")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .fill(t.page)
        .clip_corners([CORNER, 0.0, 0.0, CORNER])
        .motion(motion::REFLOW)
        // The album-colour glow behind the header, a real radial gradient.
        .child(
            el().key("glow")
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(300.0)
                .radial_gradient(
                    [0.28, 0.0],
                    [760.0, 300.0],
                    [(0.0, tint.alpha(0.30)), (1.0, Color::TRANSPARENT)],
                ),
        )
        .child(top_bar(s, t));
    page = page.child(header(s, t, &title, &meta));
    if s.page == Page::Playlists {
        return page.child(playlist_grid(s, t));
    }
    page.child(column_header(t)).child(track_list(s, t))
}

fn top_bar(s: &State, t: &Theme) -> El {
    let field = text_input(&s.query, type_::BODY.lh(22.0))
        .key("search")
        .label("Search this page")
        .single_line()
        .grow(1.0)
        .color(t.text)
        .placeholder("Search this page", t.faint)
        .on_edit(|s: &mut State| &mut s.query)
        .on_input_change(|s: &mut State, _| s.refresh());
    row()
        .h(60.0)
        .px(22.0)
        .gap(10.0)
        .center_y()
        .child(
            row()
                .h(36.0)
                .w(340.0)
                .px(12.0)
                .gap(8.0)
                .center_y()
                .radius(18.0)
                .fill(Color::WHITE.alpha(0.07))
                .child(
                    s.icons
                        .as_ref()
                        .map_or_else(|| el().size(18.0, 18.0), |i| icon(&i.search, 18.0, t.muted)),
                )
                .child(field),
        )
        .child(spacer())
        .child(
            text(s.message.clone().unwrap_or_default())
                .font(type_::LABEL)
                .color(theme::c("#ff8a8a"))
                .nowrap(),
        )
}

fn cover(s: &State, track: Option<&Track>, size: f32, radius: f32, t: &Theme) -> El {
    let tile = el()
        .size(size, size)
        .radius(radius)
        .fill(t.tile)
        .clip_rounded(radius);
    let url = track.and_then(|track| track.artwork_url.as_deref());
    match (
        url.and_then(|u| s.art.as_ref().and_then(|a| a.get(u))),
        s.icons.as_ref(),
    ) {
        (Some(pixels), _) => {
            tile.child(el().w_full().h_full().pixels_fit(&pixels, SurfaceFit::Fill))
        }
        (None, Some(icons)) => {
            tile.center()
                .child(icon(&icons.note, (size * 0.42).min(26.0), t.faint))
        }
        (None, None) => tile,
    }
}

fn header(s: &State, t: &Theme, title: &str, meta: &str) -> El {
    let source = s.source();
    let lead = source.tracks.first();
    let art = match &s.page {
        Page::Liked => el()
            .size(132.0, 132.0)
            .radius(18.0)
            .linear_gradient(
                std::f32::consts::PI * 0.75,
                [(0.0, theme::c("#8a5ad0")), (1.0, theme::c("#4a6fd4"))],
            )
            .center()
            .child(s.icons.as_ref().map_or_else(
                || el().size(44.0, 44.0),
                |i| icon(&i.heart_fill, 52.0, Color::WHITE),
            )),
        _ => cover(s, lead, 132.0, 18.0, t),
    };
    let playable = s.page != Page::Playlists && !source.tracks.is_empty();
    let mut actions = row().gap(10.0);
    if playable {
        actions = actions
            .child(pill("page-play", "Play", true, t).on_press(|s: &mut State| s.play_from(0)))
            .child(
                pill("page-shuffle", "Shuffle", false, t).on_press(|s: &mut State| {
                    s.shuffle = true;
                    s.send(cadence::backend::BackendCommand::SetShuffle(true));
                    s.play_from(0);
                }),
            );
    }
    let mut head = row()
        .key("header")
        .px(26.0)
        .pt(8.0)
        .pb(18.0)
        .gap(22.0)
        .items_end();
    if s.page != Page::Playlists {
        head = head.child(art);
    }
    head.child(
        col()
            .gap(6.0)
            .child(
                text(title.to_owned())
                    .font(
                        rune::tok::TextStyle {
                            size: 40.0,
                            ..type_::TITLE
                        }
                        .lh(46.0),
                    )
                    .color(t.text)
                    .nowrap(),
            )
            .child(text(meta.to_owned()).font(type_::BODY).color(t.muted))
            .child(el().h(4.0))
            .child(actions),
    )
}

fn column_header(t: &Theme) -> El {
    let label = |s: &'static str| text(s).font(type_::LABEL).color(t.faint);
    row()
        .key("columns")
        .h(30.0)
        .px(34.0)
        .gap(14.0)
        .center_y()
        .child(el().w(34.0).child(label("#")))
        .child(el().w(40.0))
        .child(col().grow(1.0).child(label("Title")))
        .child(el().w(220.0).child(label("Album")))
        .child(el().w(28.0))
        .child(el().w(44.0).child(label("Time")))
}

fn track_list(s: &State, t: &Theme) -> El {
    let source = s.source();
    if let Some(error) = &source.error {
        return col()
            .grow(1.0)
            .center()
            .child(text(error.clone()).font(type_::BODY).color(t.muted));
    }
    if s.shown.is_empty() {
        let note = if source.done || s.page == Page::Liked {
            "Nothing here yet."
        } else {
            "Loading songs…"
        };
        return col()
            .grow(1.0)
            .center()
            .child(text(note).font(type_::BODY).color(t.muted));
    }
    col()
        .key("list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        // Rows fade into the top and bottom edges while there is more to scroll.
        .scroll_edge_fade(28.0)
        .child(
            col()
                .key("rows")
                .w_full()
                .px(14.0)
                .growth(Default::default())
                .virtual_rows(
                    s.shown.len(),
                    ROW_H,
                    |s: &State, i| Cow::Owned(format!("r{}", s.shown[i])),
                    |s: &State, i| track_row(s, s.shown[i], i),
                ),
        )
}

fn playing_here(s: &State, track: &Track) -> bool {
    s.now
        .as_ref()
        .is_some_and(|now| now.source_id == track.source_id)
}

fn track_row(s: &State, index: usize, position: usize) -> El {
    let t = theme::dark();
    let source = s.source();
    let track = &source.tracks[index];
    let liked = s.liked_ids.contains(&track.source_id);
    let here = playing_here(s, track);
    let ink = if here { theme::c("#8fb0ff") } else { t.text };

    let heart = {
        let glyph = s
            .icons
            .as_ref()
            .map(|i| if liked { &i.heart_fill } else { &i.heart });
        let tint = if liked { t.heart } else { t.muted };
        let mut button = el()
            .key(format!("like-{index}"))
            .size(28.0, 28.0)
            .radius(14.0)
            .center()
            .hover(Color::WHITE.alpha(0.08))
            .press_scale(0.88)
            .on_press({
                let track = track.clone();
                move |s: &mut State| s.like(track.clone())
            })
            .child(glyph.map_or_else(|| el().size(18.0, 18.0), |g| icon(g, 18.0, tint)));
        // A song you have not liked shows its heart only while the row is hovered.
        if !liked {
            button = button.group_opacity(0.0, 1.0);
        }
        button
    };

    let number = if here && s.playing {
        text("♪").font(type_::BODY).color(ink)
    } else {
        text((position + 1).to_string())
            .font(type_::LABEL)
            .color(t.faint)
    };

    row()
        .key(format!("row-{index}"))
        .w_full()
        .h(ROW_H)
        .px(20.0)
        .gap(14.0)
        .center_y()
        .radius(10.0)
        .hover_group()
        .hover(t.hover)
        .on_press(move |s: &mut State| s.play_from(index))
        .child(el().w(34.0).child(number))
        .child(cover(s, Some(track), 40.0, 6.0, &t))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(
                    text(track.title.to_string())
                        .font(type_::BODY)
                        .color(ink)
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
            el().w(220.0).child(
                text(track.album.to_string())
                    .font(type_::LABEL)
                    .color(t.muted)
                    .nowrap()
                    .truncate(),
            ),
        )
        .child(heart)
        .child(
            el().w(44.0).child(
                text(clock(track.duration_ms))
                    .font(type_::LABEL)
                    .color(t.muted),
            ),
        )
}

fn clock(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn playlist_grid(s: &State, t: &Theme) -> El {
    let mut grid = row()
        .key("grid")
        .w_full()
        .px(26.0)
        .py(8.0)
        .gap(18.0)
        .flex_wrap();
    for playlist in &s.playlists {
        let id = playlist.source_id.clone();
        let art = playlist
            .artwork_url
            .as_deref()
            .and_then(|u| s.art.as_ref().and_then(|a| a.get(u)));
        let tile = el()
            .size(150.0, 150.0)
            .radius(14.0)
            .fill(t.tile)
            .clip_rounded(14.0);
        let tile = match (art, s.icons.as_ref()) {
            (Some(pixels), _) => {
                tile.child(el().w_full().h_full().pixels_fit(&pixels, SurfaceFit::Fill))
            }
            (None, Some(i)) => tile.center().child(icon(&i.playlists, 32.0, t.faint)),
            (None, None) => tile,
        };
        grid = grid.child(
            col()
                .key(format!("pl-{}", playlist.source_id))
                .w(150.0)
                .gap(6.0)
                .radius(14.0)
                .hover_group()
                .press_scale(0.98)
                .on_press(move |s: &mut State| s.go(Page::Playlist(id.clone())))
                .child(tile)
                .child(
                    text(playlist.name.to_string())
                        .font(type_::BODY)
                        .color(t.text)
                        .nowrap()
                        .truncate(),
                )
                .child(
                    text(format!("{} songs", playlist.track_count))
                        .font(type_::LABEL)
                        .color(t.muted),
                ),
        );
    }
    col()
        .key("grid-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(grid)
}

// ───────────────────────────── player bar ─────────────────────────────

fn player_bar(s: &State, t: &Theme) -> El {
    let icons: Option<&Icons> = s.icons.as_ref();
    let now = s.now.as_ref();
    let liked = now.is_some_and(|n| s.liked_ids.contains(&n.source_id));

    // The cover is one element that flies: this one while the sheet is closed, the big
    // one in `now_playing` while it is open. They share an id, so Rune carries it between.
    let mut art = cover(s, now, 48.0, 8.0, t).key("bar-art").press_scale(0.96);
    if !s.np && now.is_some() {
        art = art.matched("now-art").on_press(|s: &mut State| s.np = true);
    }
    let heart = now.cloned().map(|track| {
        let glyph = icons.map(|i| if liked { &i.heart_fill } else { &i.heart });
        row()
            .key("bar-like")
            .size(30.0, 30.0)
            .radius(15.0)
            .center()
            .hover(t.hover)
            .press_scale(0.9)
            .on_press(move |s: &mut State| s.like(track.clone()))
            .child(glyph.map_or_else(
                || el().size(18.0, 18.0),
                |g| icon(g, 18.0, if liked { t.heart } else { t.muted }),
            ))
    });
    let left = row()
        .w(300.0)
        .gap(12.0)
        .center_y()
        .child(art)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(
                    text(now.map_or_else(String::new, |n| n.title.to_string()))
                        .font(type_::BODY)
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
        .child(heart.unwrap_or_else(|| el().size(30.0, 30.0)));

    let button = |key: &'static str, glyph: Option<&rune::Image>| {
        row()
            .key(key)
            .size(34.0, 34.0)
            .center()
            .radius(17.0)
            .hover(t.hover)
            .press_scale(0.92)
            .child(glyph.map_or_else(|| el().size(20.0, 20.0), |g| icon(g, 20.0, t.text)))
    };
    let play = row()
        .key("play")
        .size(40.0, 40.0)
        .center()
        .radius(20.0)
        .fill(theme::c("#f5f3ef"))
        .press_scale(0.94)
        .on_press(|s: &mut State| s.toggle_play())
        .child(icons.map_or_else(
            || el().size(22.0, 22.0),
            |i| {
                icon(
                    if s.playing { &i.pause } else { &i.play },
                    22.0,
                    theme::c("#111111"),
                )
            },
        ));
    let duration = now.map_or(0, |n| n.duration_ms).max(1);
    let fraction = (s.progress_ms() as f32 / duration as f32).clamp(0.0, 1.0);
    let bar_w = 360.0;
    let center = col()
        .w(bar_w + 100.0)
        .gap(4.0)
        .items_center()
        .child(
            row()
                .gap(14.0)
                .center_y()
                .child(
                    button("shuffle", icons.map(|i| &i.shuffle)).on_press(|s: &mut State| {
                        s.shuffle = !s.shuffle;
                        s.send(cadence::backend::BackendCommand::SetShuffle(s.shuffle));
                    }),
                )
                .child(
                    button("prev", icons.map(|i| &i.prev)).on_press(|s: &mut State| {
                        s.send(cadence::backend::BackendCommand::Previous)
                    }),
                )
                .child(play)
                .child(
                    button("next", icons.map(|i| &i.next))
                        .on_press(|s: &mut State| s.send(cadence::backend::BackendCommand::Next)),
                ),
        )
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(clock(s.progress_ms()))
                        .font(type_::LABEL)
                        .color(t.faint)
                        .w(40.0),
                )
                .child(
                    el().key("scrub")
                        .w(bar_w - 40.0 - 40.0 + 80.0 - 80.0)
                        .h(4.0)
                        .radius(2.0)
                        .fill(Color::WHITE.alpha(0.16))
                        .child(
                            el().w((bar_w - 80.0) * fraction)
                                .h(4.0)
                                .radius(2.0)
                                .fill(t.text),
                        ),
                )
                .child(
                    text(clock(duration))
                        .font(type_::LABEL)
                        .color(t.faint)
                        .w(40.0),
                ),
        );
    let right = row().w(300.0).gap(8.0).center_y().justify_end().child(
        row()
            .key("np-open")
            .size(34.0, 34.0)
            .center()
            .radius(17.0)
            .hover(t.hover)
            .press_scale(0.92)
            .on_press(|s: &mut State| s.np = !s.np)
            .child(icons.map_or_else(|| el().size(20.0, 20.0), |i| icon(&i.queue, 20.0, t.muted))),
    );

    row()
        .key("player")
        .w_full()
        .h(BAR_H)
        .px(16.0)
        .center_y()
        .justify_between()
        .fill(t.bar)
        .child(left)
        .child(center)
        .child(right)
}

// ───────────────────────────── now playing ─────────────────────────────

/// The Now Playing sheet: the player bar's cover grows into the left half and the
/// queue sits on the right. Opening moves one element; nothing crossfades.
fn now_playing(s: &State, t: &Theme) -> El {
    let now = s.now.as_ref();
    let tint = theme::tint(now.map_or("", |n| n.title.as_ref()));
    let big = {
        let tile = el()
            .key("np-art")
            .size(320.0, 320.0)
            .radius(24.0)
            .fill(t.tile)
            .clip_rounded(24.0)
            .matched("now-art")
            .box_shadow(rune::Shadow::core_animation(
                0.0,
                28.0,
                28.0,
                Color::BLACK.alpha(0.5),
            ));
        let pixels = now
            .and_then(|n| n.artwork_url.as_deref())
            .and_then(|u| s.art.as_ref().and_then(|a| a.big(u).or_else(|| a.get(u))));
        match (pixels, s.icons.as_ref()) {
            (Some(p), _) => tile.child(el().w_full().h_full().pixels_fit(&p, SurfaceFit::Fill)),
            (None, Some(i)) => tile.center().child(icon(&i.note, 48.0, t.faint)),
            (None, None) => tile,
        }
    };
    let mut queue = col()
        .w(320.0)
        .gap(2.0)
        .child(text("Up next").font(type_::TITLE).color(t.text))
        .child(el().h(8.0));
    for (i, track) in s.up_next.iter().take(7).enumerate() {
        queue = queue.child(
            row()
                .key(format!("up-{i}"))
                .h(44.0)
                .px(10.0)
                .gap(10.0)
                .center_y()
                .radius(10.0)
                .hover(Color::WHITE.alpha(0.08))
                .child(cover(s, Some(track), 32.0, 5.0, t))
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
                ),
        );
    }
    let close = row()
        .key("np-close")
        .size(36.0, 36.0)
        .center()
        .radius(18.0)
        .fill(Color::WHITE.alpha(0.12))
        .press_scale(0.94)
        .on_press(|s: &mut State| s.np = false)
        .child(
            s.icons
                .as_ref()
                .map_or_else(|| el().size(18.0, 18.0), |i| icon(&i.down, 18.0, t.text)),
        );

    el().key("np")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(BAR_H))
        .fill(theme::c("#141418"))
        .radial_gradient(
            [0.25, 0.15],
            [900.0, 700.0],
            [(0.0, tint.alpha(0.5)), (1.0, Color::TRANSPARENT)],
        )
        .transition(Transition::opacity())
        .child(el().pin(None, Some(18.0), Some(20.0), None).child(close))
        .child(
            row()
                .w_full()
                .h_full()
                .center()
                .gap(72.0)
                .child(
                    col().gap(18.0).child(big).child(
                        col()
                            .gap(2.0)
                            .child(
                                text(now.map_or_else(String::new, |n| n.title.to_string()))
                                    .font(rune::tok::TextStyle {
                                        size: 22.0,
                                        ..type_::TITLE
                                    })
                                    .color(t.text)
                                    .nowrap(),
                            )
                            .child(
                                text(now.map_or_else(String::new, |n| n.artist.to_string()))
                                    .font(type_::BODY)
                                    .color(t.muted)
                                    .nowrap(),
                            ),
                    ),
                )
                .child(queue),
        )
}
