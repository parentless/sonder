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

pub(crate) const RAIL: f32 = 78.0;
pub(crate) const WIDE: f32 = 232.0;
pub(crate) const ROW_H: f32 = 56.0;
pub(crate) const BAR_H: f32 = 76.0;
pub(crate) const CORNER: f32 = 20.0;

/// The sidebar's spring: quicker than the default reflow (0.4 s).
pub(crate) fn quick() -> rune::Spring {
    rune::Spring::new(0.26, 0.0)
}

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
    for layer in crate::views::layers(s, &t) {
        app = app.child(layer);
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

pub(crate) fn pill(key: &'static str, label: &'static str, primary: bool, t: &Theme) -> El {
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
        let mut item = row()
            .key(key)
            .w_full()
            .h(42.0)
            .px(if s.wide { 12.0 } else { 14.0 })
            .gap(12.0)
            .center_y()
            .radius(12.0)
            .fill(if on { t.active } else { Color::TRANSPARENT })
            .hover(if on { t.active } else { t.hover })
            .press_scale(0.98)
            .on_press(move |s: &mut State| s.go(page.clone()));
        if let Some(glyph) = glyph {
            item = item.child(icon(glyph, 17.0, t.text));
        }
        // The label exists only while the sidebar is wide; mounting and unmounting it
        // is what fades it, so there is no label animation of my own to keep in step.
        if s.wide {
            item = item.child(
                text(label)
                    .key(format!("{key}-label"))
                    .font(type_::BODY)
                    .color(t.text)
                    .nowrap()
                    .transition(Transition::opacity()),
            );
        }
        item
    };

    // The chevron only closes the sidebar. While it is a rail, the logo opens it again.
    let collapse = row()
        .key("rail-toggle")
        .size(30.0, 30.0)
        .center()
        .radius(8.0)
        .hover(t.hover)
        .on_press(|s: &mut State| s.wide = false)
        .child(match icons {
            Some(i) => icon(&i.back, 18.0, t.muted),
            None => el().size(18.0, 18.0),
        });
    let mark = match icons {
        Some(i) => el().size(28.0, 28.0).pixels(i.logo.pixels()),
        None => el().size(28.0, 28.0).radius(8.0).fill(t.tile),
    };
    let logo = if s.wide {
        row().key("logo").size(32.0, 32.0).center().child(mark)
    } else {
        row()
            .key("logo")
            .size(40.0, 40.0)
            .center()
            .radius(10.0)
            .hover(t.hover)
            .press_scale(0.94)
            .on_press(|s: &mut State| s.wide = true)
            .child(mark)
    };
    let brand = row()
        .h(44.0)
        .px(if s.wide { 8.0 } else { 2.0 })
        .gap(10.0)
        .center_y()
        .child(logo)
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
            collapse
        } else {
            el().key("rail-toggle-spacer")
        });

    let section = |key: &'static str, label: &'static str| {
        row().key(key).w_full().px(12.0).pb(4.0).child(
            text(label)
                .font(type_::LABEL)
                .color(t.muted)
                .nowrap()
                .transition(Transition::opacity()),
        )
    };
    let i = icons;
    let mut nav_group = col().key("nav").w_full().gap(4.0);
    if s.wide {
        nav_group = nav_group.child(section("sec-library", "Library"));
    }
    nav_group = nav_group
        .child(nav("nav-home", "Home", i.map(|i| &i.home), Page::Home))
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
    if !s.local.folders.is_empty() {
        nav_group = nav_group.child(nav(
            "nav-local",
            "Local Files",
            i.map(|i| &i.folder),
            Page::Local,
        ));
    }
    let mut lists = col()
        .key("side-lists")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(4.0);
    if s.wide {
        lists = lists.child(section("sec-playlists", "Playlists"));
    }
    lists = lists.child(sidebar_playlists(s));
    col()
        .key("sidebar")
        .w(if s.wide { WIDE } else { RAIL })
        .h_full()
        .padding(16.0)
        .gap(16.0)
        .fill(t.side)
        .clip_rounded(0.0)
        .motion(quick())
        .child(brand)
        .child(nav_group)
        .child(lists)
}

/// Every playlist, pinned ones first. Rows are virtual so a long list loads only the covers on screen.
fn sidebar_playlists(s: &State) -> El {
    let mut order: Vec<usize> = (0..s.playlists.len()).collect();
    order.sort_by_key(|&i| !s.pinned.contains(&s.playlists[i].source_id));
    let order = std::rc::Rc::new(order);
    let count = order.len();
    let (o1, o2) = (order.clone(), order);
    col()
        .key("side-playlists")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .scroll_edge_fade(18.0)
        .child(
            col()
                .key("side-rows")
                .w_full()
                .growth(Default::default())
                .virtual_rows(
                    count,
                    44.0,
                    move |s: &State, i| Cow::Owned(format!("sp{}", s.playlists[o1[i]].source_id)),
                    move |s: &State, i| side_playlist(s, o2[i]),
                ),
        )
}

fn side_playlist(s: &State, index: usize) -> El {
    let t = theme::dark();
    let playlist = &s.playlists[index];
    let id = playlist.source_id.clone();
    let on = s.page == Page::Playlist(id.clone());
    let mut r = row()
        .key(format!("sp-{id}"))
        .w_full()
        .h(44.0)
        .px(if s.wide { 12.0 } else { 8.0 })
        .gap(8.0)
        .center_y()
        .radius(8.0)
        .fill(if on { t.active } else { Color::TRANSPARENT })
        .hover(t.hover)
        .press_scale(0.98)
        .on_press(move |s: &mut State| s.go(Page::Playlist(id.clone())))
        .child(cover_url(s, playlist.artwork_url.as_deref(), 30.0, 7.0, &t));
    if s.wide {
        r = r.child(
            text(playlist.name.to_string())
                .key(format!("sp-label-{}", playlist.source_id))
                .font(type_::BODY)
                .color(t.text)
                .nowrap()
                .truncate()
                .transition(Transition::opacity()),
        );
    }
    r
}

// ─────────────────────────────── page ───────────────────────────────

fn greeting(s: &State) -> String {
    let name = s
        .profile
        .as_ref()
        .map(|p| {
            p.display_name
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned()
        })
        .filter(|n| !n.is_empty());
    match name {
        Some(name) => format!("Welcome back, {name}"),
        None => "Welcome back".into(),
    }
}

fn page_title(s: &State) -> (String, String) {
    let source = s.source();
    match &s.page {
        Page::Liked => ("Liked Songs".into(), format!("{} songs", source.total)),
        Page::Recent => ("Recently played".into(), format!("{} songs", source.total)),
        Page::Playlists => (
            "Playlists".into(),
            format!("{} playlists", s.playlists.len()),
        ),
        Page::Settings => ("Settings".into(), "Your account and playback".into()),
        Page::Home => (greeting(s), "Pick up where you left off".into()),
        Page::Local => (
            "Local Files".into(),
            format!(
                "{} songs from {} folders",
                source.total,
                s.local.folders.len()
            ),
        ),
        Page::Album(id) | Page::Artist(id) => {
            let detail = s.details.get(id).cloned().unwrap_or_default();
            (
                if detail.title.is_empty() {
                    "Loading…".into()
                } else {
                    detail.title
                },
                detail.sub,
            )
        }
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
        .motion(quick())
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
        .child(el().h(60.0));
    if matches!(s.page, Page::Settings | Page::Home) {
        page = page.child(
            col()
                .px(26.0)
                .pt(8.0)
                .pb(14.0)
                .gap(4.0)
                .child(
                    text(title.clone())
                        .font(
                            rune::tok::TextStyle {
                                size: 32.0,
                                ..type_::TITLE
                            }
                            .lh(38.0),
                        )
                        .color(t.text),
                )
                .child(text(meta.clone()).font(type_::BODY).color(t.muted)),
        );
    } else {
        page = page.child(header(s, t, &title, &meta));
    }
    // Last, so the dropdown draws over the page beneath it.
    if s.page == Page::Settings {
        return page.child(settings_body(s, t)).child(top_bar(s, t));
    }
    if s.page == Page::Home {
        return page
            .child(crate::views::home_body(s, t))
            .child(top_bar(s, t));
    }
    if let Page::Artist(id) = &s.page {
        return page
            .child(crate::views::artist_body(s, t, id))
            .child(top_bar(s, t));
    }
    if s.page == Page::Playlists {
        return page.child(playlist_grid(s, t)).child(top_bar(s, t));
    }
    page.child(column_header(t))
        .child(track_list(s, t))
        .child(top_bar(s, t))
}

fn top_bar(s: &State, t: &Theme) -> El {
    let field = text_input(&s.query, type_::BODY.lh(22.0))
        .key("search")
        .label("Search Spotify")
        .single_line()
        .grow(1.0)
        .color(t.text)
        .placeholder("Search Spotify", t.faint)
        .on_edit(|s: &mut State| &mut s.query)
        .on_input_change(|s: &mut State, _| s.on_search_input())
        .on_focus_change(|s: &mut State, focused, _| {
            if focused {
                s.search.open = !s.query.text.trim().is_empty();
            }
        })
        .on_key_preview(|s: &mut State, key, tx| {
            if key.key == rune::Key::Escape && s.search.open {
                s.search.open = false;
                tx.blur();
                return true;
            }
            false
        });
    let bar = row()
        .h(36.0)
        .w(SEARCH_W)
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
        .child(field);
    let wrap = col().key("search-wrap").w(SEARCH_W).h(36.0).child(bar);
    row()
        .pin(Some(0.0), Some(0.0), Some(0.0), None)
        .h(60.0)
        .px(22.0)
        .gap(10.0)
        .center_y()
        .child(wrap)
        .child(spacer())
        .child(
            text(s.message.clone().unwrap_or_default())
                .font(type_::LABEL)
                .color(theme::c("#ff8a8a"))
                .nowrap(),
        )
        .child(profile_button(s, t))
}

/// The listener's picture. It opens a menu that slides down from it, like the search panel.
fn profile_button(s: &State, t: &Theme) -> El {
    let name = s
        .profile
        .as_ref()
        .map_or_else(String::new, |p| p.display_name.to_string());
    let initial = name
        .chars()
        .next()
        .map_or("?".to_owned(), |c| c.to_uppercase().to_string());
    let face = {
        let base = el()
            .size(34.0, 34.0)
            .radius(17.0)
            .fill(theme::c("#3a3a42"))
            .clip_rounded(17.0);
        let url = s.profile.as_ref().and_then(|p| p.artwork_url.as_deref());
        match url.and_then(|u| s.art.as_ref().and_then(|a| a.get(u))) {
            Some(pixels) => {
                base.child(el().w_full().h_full().pixels_fit(&pixels, SurfaceFit::Fill))
            }
            None => base
                .center()
                .child(text(initial).font(type_::TITLE).color(t.text)),
        }
    };
    let avatar = row()
        .key("avatar")
        .size(38.0, 38.0)
        .center()
        .radius(19.0)
        .hover(t.hover)
        .press_scale(0.94)
        .on_press(|s: &mut State| s.menu = !s.menu)
        .child(face);
    col().key("avatar-wrap").size(38.0, 38.0).child(avatar)
}

fn avatar_menu(s: &State, t: &Theme) -> Option<El> {
    let name = s
        .profile
        .as_ref()
        .map_or_else(String::new, |p| p.display_name.to_string());
    let item = |key: &'static str, glyph: Option<&rune::Image>, label: &'static str| {
        row()
            .key(key)
            .w_full()
            .h(40.0)
            .px(10.0)
            .gap(10.0)
            .center_y()
            .radius(10.0)
            .hover(t.hover)
            .press_scale(0.98)
            .child(glyph.map_or_else(|| el().size(18.0, 18.0), |g| icon(g, 18.0, t.muted)))
            .child(text(label).font(type_::BODY).color(t.text).nowrap())
    };
    let i = s.icons.as_ref();
    Some(
        col()
            .key("avatar-menu")
            .pin(None, Some(62.0), Some(22.0), None)
            .w(210.0)
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
            .child(
                row().w_full().h(34.0).px(10.0).center_y().child(
                    text(if name.is_empty() {
                        "Signed in".to_owned()
                    } else {
                        name
                    })
                    .font(type_::LABEL)
                    .color(t.faint)
                    .nowrap()
                    .truncate(),
                ),
            )
            .child(
                item("menu-settings", i.map(|i| &i.settings), "Settings")
                    .on_press(|s: &mut State| s.go(Page::Settings)),
            )
            .child(
                item("menu-logout", i.map(|i| &i.logout), "Log out").on_press(|s: &mut State| {
                    s.menu = false;
                    s.config_serial += 1;
                    let generation = s.config_serial;
                    s.send(cadence::backend::BackendCommand::Logout { generation });
                }),
            ),
    )
}

// ───────────────────────────── settings ─────────────────────────────

fn settings_body(s: &State, t: &Theme) -> El {
    let card = |title: &'static str, children: Vec<El>| {
        let mut c = col()
            .w_full()
            .padding(18.0)
            .gap(12.0)
            .radius(16.0)
            .fill(Color::WHITE.alpha(0.05))
            .child(text(title).font(type_::TITLE).color(t.text));
        for child in children {
            c = c.child(child);
        }
        c
    };
    let line = |label: &'static str, value: String| {
        row()
            .w_full()
            .gap(12.0)
            .center_y()
            .child(text(label).font(type_::BODY).color(t.muted).w(150.0))
            .child(
                text(value)
                    .font(type_::BODY)
                    .color(t.text)
                    .nowrap()
                    .truncate(),
            )
    };
    let name = s.profile.as_ref().map_or_else(
        || "Not loaded yet".to_owned(),
        |p| p.display_name.to_string(),
    );
    let account = card(
        "Account",
        vec![
            line("Signed in as", name),
            row()
                .gap(10.0)
                .child(
                    pill("set-logout", "Log out", false, t).on_press(|s: &mut State| {
                        s.config_serial += 1;
                        let generation = s.config_serial;
                        s.send(cadence::backend::BackendCommand::Logout { generation });
                    }),
                )
                .child(pill("set-reset", "Change Spotify app", false, t).on_press(
                    |s: &mut State| {
                        s.config_serial += 1;
                        let generation = s.config_serial;
                        s.send(
                            cadence::backend::BackendCommand::ResetSpotifyConfiguration {
                                generation,
                            },
                        );
                    },
                )),
        ],
    );
    let playback = card(
        "Playback",
        vec![
            line("Volume", format!("{}%", (s.volume * 100.0).round())),
            line(
                "Shuffle",
                if s.shuffle { "On".into() } else { "Off".into() },
            ),
            line("Covers cache", "Up to 48 MB on disk".into()),
        ],
    );
    let mut folders: Vec<El> = Vec::new();
    for (n, folder) in s.local.folders.iter().enumerate() {
        let path = folder.clone();
        folders.push(
            row()
                .key(format!("lf-{n}"))
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    text(folder.display().to_string())
                        .font(type_::BODY)
                        .color(t.text)
                        .nowrap()
                        .truncate()
                        .grow(1.0),
                )
                .child(
                    row()
                        .key(format!("lf-rm-{n}"))
                        .h(28.0)
                        .px(12.0)
                        .center()
                        .radius(14.0)
                        .fill(t.pill)
                        .hover(t.hover)
                        .press_scale(0.97)
                        .on_press(move |s: &mut State| {
                            if let Some(bridge) = &s.bridge {
                                bridge.remove_local_folder(path.clone());
                            }
                        })
                        .child(text("Remove").font(type_::LABEL).color(t.pill_text)),
                ),
        );
    }
    if folders.is_empty() {
        folders.push(
            text("Add a folder of music files to play them here.")
                .font(type_::BODY)
                .color(t.muted),
        );
    }
    folders.push(row().child(
        pill("lf-add", "Add folder", false, t).on_press(|s: &mut State| {
            if let Some(bridge) = &s.bridge {
                bridge.add_local_folder();
            }
        }),
    ));
    let local = card("Local Files", folders);
    let about = card(
        "About",
        vec![
            line("App", "Sonder on Rune (preview)".into()),
            line(
                "Library",
                format!(
                    "{} liked songs, {} playlists",
                    s.liked.total,
                    s.playlists.len()
                ),
            ),
        ],
    );
    col()
        .key("settings")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(
            col()
                .w_full()
                .px(26.0)
                .py(10.0)
                .gap(14.0)
                .child(account)
                .child(playback)
                .child(local)
                .child(about),
        )
}

const SEARCH_W: f32 = 440.0;

fn page_left(s: &State) -> f32 {
    if s.wide { WIDE } else { RAIL }
}

/// The search panel and the profile menu, each with a layer behind it that closes it.
pub(crate) fn dropdown_layers(s: &State, t: &Theme) -> Option<Vec<El>> {
    let mut out = Vec::new();
    if s.search.open && !s.query.text.trim().is_empty() {
        out.push(
            el().key("search-scrim")
                .pin(Some(0.0), Some(60.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.search.open = false),
        );
        out.push(search_dropdown(s, t));
    }
    if s.menu {
        out.push(
            el().key("menu-scrim")
                .pin(Some(0.0), Some(60.0), Some(0.0), Some(0.0))
                .on_press(|s: &mut State| s.menu = false),
        );
        out.extend(avatar_menu(s, t));
    }
    (!out.is_empty()).then_some(out)
}

/// The panel under the search field: songs already loaded, then Spotify's answer.
/// It slides up out of the field and fades, and leaves the same way, faster.
fn search_dropdown(s: &State, t: &Theme) -> El {
    let section = |label: &'static str| text(label).font(type_::LABEL).color(t.faint).nowrap();
    let result = |key: String, art: El, title: String, sub: String| {
        row()
            .key(key)
            .w_full()
            .h(48.0)
            .px(8.0)
            .gap(10.0)
            .center_y()
            .radius(10.0)
            .hover(t.hover)
            .press_scale(0.99)
            .child(art)
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .child(
                        text(title)
                            .font(type_::BODY)
                            .color(t.text)
                            .nowrap()
                            .truncate(),
                    )
                    .child(
                        text(sub)
                            .font(type_::LABEL)
                            .color(t.muted)
                            .nowrap()
                            .truncate(),
                    ),
            )
    };

    let mut list = col().w_full().gap(2.0);
    let mut any = false;
    if !s.search.library.is_empty() {
        any = true;
        list = list.child(pad(section("In your library")));
        for (i, track) in s.search.library.iter().enumerate() {
            let pool = s.search.library.clone();
            list = list.child(
                result(
                    format!("sr-lib-{i}"),
                    cover(s, Some(track), 32.0, 6.0, t),
                    track.title.to_string(),
                    track.artist.to_string(),
                )
                .on_press(move |s: &mut State| s.play_found(pool.clone(), i)),
            );
        }
    }
    let live = s
        .search
        .live
        .as_ref()
        .filter(|(q, _)| q == s.query.text.trim())
        .map(|(_, r)| r);
    if let Some(live) = live {
        let tracks: Vec<Track> = live
            .tracks
            .iter()
            .filter(|t| !s.search.library.iter().any(|l| l.source_id == t.source_id))
            .take(3)
            .cloned()
            .collect();
        if !tracks.is_empty() {
            any = true;
            list = list.child(pad(section("Songs")));
            for (i, track) in tracks.iter().enumerate() {
                let pool = tracks.clone();
                list = list.child(
                    result(
                        format!("sr-song-{i}"),
                        cover(s, Some(track), 32.0, 6.0, t),
                        track.title.to_string(),
                        track.artist.to_string(),
                    )
                    .on_press(move |s: &mut State| s.play_found(pool.clone(), i)),
                );
            }
        }
        if !live.artists.is_empty() {
            any = true;
            list = list.child(pad(section("Artists")));
            for (i, artist) in live.artists.iter().take(2).enumerate() {
                list = list.child(
                    result(
                        format!("sr-artist-{i}"),
                        cover_url(s, artist.artwork_url.as_deref(), 32.0, 16.0, t),
                        artist.name.to_string(),
                        "Artist".into(),
                    )
                    .on_press({
                        let id = artist.source_id.clone();
                        move |s: &mut State| {
                            s.search.open = false;
                            s.go(Page::Artist(id.clone()));
                        }
                    }),
                );
            }
        }
        if !live.albums.is_empty() {
            any = true;
            list = list.child(pad(section("Albums")));
            for (i, album) in live.albums.iter().take(2).enumerate() {
                list = list.child(
                    result(
                        format!("sr-album-{i}"),
                        cover_url(s, album.artwork_url.as_deref(), 32.0, 6.0, t),
                        album.name.to_string(),
                        album.release_date.clone().unwrap_or_else(|| "Album".into()),
                    )
                    .on_press({
                        let id = album.source_id.clone();
                        move |s: &mut State| {
                            s.search.open = false;
                            s.go(Page::Album(id.clone()));
                        }
                    }),
                );
            }
        }
    }
    let waiting = s.search.loading || s.search.due.is_some();
    if waiting && live.is_none() {
        list = list.child(pad(section("Searching Spotify…")));
    } else if !any && !waiting {
        list = list.child(pad(section("No results")));
    }

    col()
        .key("search-drop")
        .pin(Some(page_left(s) + 22.0), Some(62.0), None, None)
        .w(SEARCH_W)
        .padding(8.0)
        .radius(16.0)
        .fill(theme::c("#242427"))
        .box_shadow(rune::Shadow::core_animation(
            0.0,
            14.0,
            30.0,
            Color::BLACK.alpha(0.45),
        ))
        .motion(motion::REFLOW)
        .transition(Transition::offset(0.0, -6.0))
        .child(list)
}

fn pad(label: El) -> El {
    row().w_full().h(26.0).px(8.0).center_y().child(label)
}

pub(crate) fn cover(s: &State, track: Option<&Track>, size: f32, radius: f32, t: &Theme) -> El {
    cover_url(
        s,
        track.and_then(|t| t.artwork_url.as_deref()),
        size,
        radius,
        t,
    )
}

pub(crate) fn cover_url(s: &State, url: Option<&str>, size: f32, radius: f32, t: &Theme) -> El {
    let tile = el()
        .size(size, size)
        .radius(radius)
        .fill(t.tile)
        .clip_rounded(radius);
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
        Page::Artist(id) => {
            let art = s.details.get(id).and_then(|d| d.art.as_deref());
            cover_url(s, art, 132.0, 66.0, t)
        }
        Page::Album(id) => {
            let art = s.details.get(id).and_then(|d| d.art.as_deref());
            cover_url(
                s,
                art.or(lead.and_then(|l| l.artwork_url.as_deref())),
                132.0,
                18.0,
                t,
            )
        }
        _ => cover(s, lead, 132.0, 18.0, t),
    };
    let playable = !matches!(s.page, Page::Playlists | Page::Settings) && !source.tracks.is_empty();
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
        .child(el().w(28.0))
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

pub(crate) fn playing_here(s: &State, track: &Track) -> bool {
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
        let tint = if liked { t.text } else { t.muted };
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

    let selected = s.selected.as_deref() == Some(track.source_id.as_str());
    row()
        .key(format!("row-{index}"))
        .w_full()
        .h(ROW_H)
        .px(20.0)
        .gap(14.0)
        .center_y()
        .radius(10.0)
        .hover_group()
        .fill(if selected {
            t.active
        } else {
            Color::TRANSPARENT
        })
        .hover(if selected { t.active } else { t.hover })
        // One click picks the row; two play it.
        .on_press({
            let id = track.source_id.clone();
            move |s: &mut State| s.selected = Some(id.clone())
        })
        .on_double_press(move |s: &mut State| s.play_from(index))
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
        .child(more_button(s, track, &t))
        .on_secondary_press_local({
            let track = track.clone();
            move |s: &mut State, e, _| s.open_track_menu(track.clone(), e.window_y)
        })
}

/// The "more" button of a song: opens the same menu as a right click. Shown on hover.
pub(crate) fn more_button(s: &State, track: &Track, t: &Theme) -> El {
    let track = track.clone();
    el().key(format!("more-{}", track.source_id))
        .size(28.0, 28.0)
        .radius(14.0)
        .center()
        .hover(Color::WHITE.alpha(0.08))
        .press_scale(0.88)
        .group_opacity(0.0, 1.0)
        .on_any_press(move |s: &mut State, button, e, _| {
            if button == rune::PointerButton::Primary {
                s.open_track_menu(track.clone(), e.window_y);
            }
        })
        .child(
            s.icons
                .as_ref()
                .map_or_else(|| el().size(18.0, 18.0), |i| icon(&i.more, 18.0, t.muted)),
        )
}

pub(crate) fn clock(ms: u32) -> String {
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
                |g| icon(g, 18.0, if liked { t.text } else { t.muted }),
            ))
    });
    let left = row()
        .w(300.0)
        .gap(12.0)
        .center_y()
        .child(art)
        .child(
            col()
                .max_w(190.0)
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
            .child(glyph.map_or_else(|| el().size(17.0, 17.0), |g| icon(g, 17.0, t.text)))
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
            || el().size(16.0, 16.0),
            |i| {
                icon(
                    if s.playing { &i.pause } else { &i.play },
                    16.0,
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
                    text(clock(s.scrub.map_or_else(
                        || s.progress_ms(),
                        |f| (duration as f32 * f) as u32,
                    )))
                    .font(type_::LABEL)
                    .color(t.faint)
                    .w(40.0),
                )
                .child(slider(
                    "scrub",
                    bar_w - 80.0,
                    s.scrub.unwrap_or(fraction),
                    t,
                    |s: &mut State, f, done| {
                        if done {
                            s.seek_to(f);
                            s.scrub = None;
                        } else {
                            s.scrub = Some(f);
                        }
                    },
                ))
                .child(
                    text(clock(duration))
                        .font(type_::LABEL)
                        .color(t.faint)
                        .w(40.0),
                ),
        );
    let round = |key: &'static str, on: bool| {
        row()
            .key(key)
            .size(34.0, 34.0)
            .center()
            .radius(17.0)
            .fill(if on { t.active } else { Color::TRANSPARENT })
            .hover(t.hover)
            .press_scale(0.92)
    };
    let right = row()
        .w(300.0)
        .gap(8.0)
        .center_y()
        .justify_end()
        .child(row().size(24.0, 24.0).center().child(icons.map_or_else(
            || el().size(18.0, 18.0),
            |i| {
                icon(
                    if s.volume <= 0.0 { &i.muted } else { &i.volume },
                    18.0,
                    t.muted,
                )
            },
        )))
        .child(slider(
            "volume",
            90.0,
            s.volume,
            t,
            |s: &mut State, f, _| s.set_volume(f),
        ))
        .child(el().w(8.0))
        .child(
            round("lyrics-open", s.lyrics.open)
                .on_press(|s: &mut State| s.toggle_lyrics())
                .child(icons.map_or_else(
                    || el().size(18.0, 18.0),
                    |i| {
                        icon(
                            &i.lyrics,
                            18.0,
                            if s.lyrics.open { t.text } else { t.muted },
                        )
                    },
                )),
        )
        .child(
            round("np-open", s.np)
                .on_press(|s: &mut State| s.np = !s.np)
                .child(icons.map_or_else(
                    || el().size(18.0, 18.0),
                    |i| icon(&i.queue, 18.0, if s.np { t.text } else { t.muted }),
                )),
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

/// A bar you click or drag: `change(state, fraction, finished)`.
fn slider(
    key: &'static str,
    width: f32,
    fraction: f32,
    t: &Theme,
    change: fn(&mut State, f32, bool),
) -> El {
    let fraction = fraction.clamp(0.0, 1.0);
    col()
        .key(key)
        .w(width)
        .h(18.0)
        .justify_center()
        .on_drag(move |s: &mut State, d, _| {
            let f = if d.w > 0.0 {
                (d.x / d.w).clamp(0.0, 1.0)
            } else {
                0.0
            };
            change(s, f, d.phase == rune::Phase::Up);
        })
        .child(
            el().w_full()
                .h(4.0)
                .radius(2.0)
                .fill(Color::WHITE.alpha(0.16))
                .child(el().w(width * fraction).h(4.0).radius(2.0).fill(t.text)),
        )
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
                .press_scale(0.98)
                // Pressing a queued song jumps straight to it.
                .on_press(move |s: &mut State| {
                    s.send(cadence::backend::BackendCommand::PlayQueueItem { offset: i })
                })
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
