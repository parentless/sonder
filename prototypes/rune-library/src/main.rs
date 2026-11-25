//! Sonder's Library page, rebuilt in Rune as a prototype.
//!
//! Three things to judge: 20,000 virtual rows scrolling, typing in the search
//! field, and the sidebar and dialog moving on springs with real exits.
//! `cargo run --release`; headless: `-- --snap library.png`.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::HashSet;

use rune::tok::{self, motion, radius, type_};
use rune::{AppSpec, Color, Cx, El, ScrollSpec, TextEdit, col, el, row, spacer, text, text_input};

const TRACKS: usize = 20_000;
const ROW_H: f32 = 52.0;
const RAIL: f32 = 60.0;
const WIDE: f32 = 224.0;

struct Track {
    title: String,
    artist: String,
    album: String,
    secs: u32,
}

struct State {
    tracks: Vec<Track>,
    /// Row indexes that match the search, in order.
    shown: Vec<usize>,
    query: TextEdit,
    liked: HashSet<usize>,
    playing: Option<usize>,
    wide: bool,
    credits: bool,
}

impl Default for State {
    fn default() -> Self {
        const ARTISTS: [&str; 8] = [
            "Harry Styles",
            "Radiohead",
            "Phoebe Bridgers",
            "Frank Ocean",
            "Tame Impala",
            "Mitski",
            "The National",
            "Big Thief",
        ];
        const WORDS: [&str; 10] = [
            "Midnight", "Static", "Honey", "Parade", "Glass", "Harbor", "Velvet", "Orbit", "Paper",
            "Signal",
        ];
        let tracks: Vec<Track> = (0..TRACKS)
            .map(|i| Track {
                title: format!("{} {} {}", WORDS[i % 10], WORDS[(i / 10) % 10], i + 1),
                artist: ARTISTS[i % 8].to_string(),
                album: format!("{} Sessions", WORDS[(i / 7) % 10]),
                secs: 150 + (i as u32 * 37) % 150,
            })
            .collect();
        State {
            shown: (0..tracks.len()).collect(),
            tracks,
            query: TextEdit::new(""),
            liked: HashSet::new(),
            playing: Some(2),
            wide: true,
            credits: false,
        }
    }
}

impl State {
    fn refilter(&mut self) {
        let q = self.query.text.to_lowercase();
        self.shown = (0..self.tracks.len())
            .filter(|&i| {
                q.is_empty()
                    || self.tracks[i].title.to_lowercase().contains(&q)
                    || self.tracks[i].artist.to_lowercase().contains(&q)
            })
            .collect();
    }
}

fn time(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let neutral = Color::WHITE.alpha(0.09);

    // Sidebar: the width change alone animates; the labels fade with it.
    let wide = s.wide;
    let nav = move |key: &'static str, label: &'static str, on: bool| {
        let mut item = row()
            .key(key)
            .h(36.0)
            .px(14.0)
            .gap(12.0)
            .center_y()
            .radius(radius::MD)
            .fill(if on { neutral } else { Color::TRANSPARENT })
            .hover(Color::WHITE.alpha(0.06))
            .child(el().size(16.0, 16.0).radius(4.0).fill(if on {
                t.text
            } else {
                t.text_secondary
            }));
        // Labels mount only when wide; their fade in and out is the transition.
        if wide {
            item = item.child(
                text(label)
                    .key(format!("{key}-label"))
                    .font(type_::BODY)
                    .color(if on { t.text } else { t.text_secondary })
                    .nowrap()
                    .transition(rune::Transition::opacity()),
            );
        }
        item
    };
    let sidebar = col()
        .key("sidebar")
        .w(if s.wide { WIDE } else { RAIL })
        .h_full()
        .padding(8.0)
        .gap(2.0)
        .fill(t.bg)
        .clip_rounded(0.0)
        .motion(motion::REFLOW)
        .child(nav("home", "Home", false))
        .child(nav("library", "Library", true))
        .child(nav("search", "Search", false))
        .child(nav("settings", "Settings", false));

    let toggle = el()
        .key("toggle")
        .size(32.0, 32.0)
        .radius(radius::MD)
        .center()
        .hover(Color::WHITE.alpha(0.08))
        .on_press(|s: &mut State| s.wide = !s.wide)
        .child(
            text(if s.wide { "‹" } else { "›" })
                .font(type_::TITLE)
                .color(t.text),
        );

    let search = text_input(&s.query, type_::BODY.lh(22.0))
        .key("search")
        .label("Search your library")
        .single_line()
        .grow(1.0)
        .color(t.text)
        .placeholder("Search your library", t.text_secondary)
        .on_edit(|s: &mut State| &mut s.query)
        .on_input_change(|s: &mut State, _| s.refilter());
    let search = row()
        .h(36.0)
        .w(360.0)
        .px(12.0)
        .center_y()
        .radius(18.0)
        .fill(neutral)
        .child(search);

    let credits = text("Credits")
        .key("credits-open")
        .font(type_::LABEL)
        .color(t.text_secondary)
        .on_press(|s: &mut State| s.credits = true);

    let toolbar = row()
        .h(56.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(toggle)
        .child(search)
        .child(spacer())
        .child(credits);

    let heading = col()
        .px(24.0)
        .pb(8.0)
        .gap(2.0)
        .child(text("Library").font(type_::TITLE).color(t.text))
        .child(
            text(format!("{} songs", s.shown.len()))
                .font(type_::LABEL)
                .color(t.text_secondary),
        );

    let list = col()
        .key("list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(
            col()
                .key("rows")
                .w_full()
                .growth(Default::default())
                .virtual_rows(
                    s.shown.len(),
                    ROW_H,
                    |s: &State, i| Cow::Owned(format!("t{}", s.shown[i])),
                    |s: &State, i| track_row(s, s.shown[i]),
                ),
        );

    let now = s.playing.and_then(|i| s.tracks.get(i));
    let bar = row()
        .h(72.0)
        .w_full()
        .px(16.0)
        .gap(12.0)
        .center_y()
        .fill(t.card)
        .child(el().size(44.0, 44.0).radius(6.0).fill(accent.alpha(0.5)))
        .child(
            col()
                .child(
                    text(now.map(|t| t.title.clone()).unwrap_or_default())
                        .font(type_::BODY)
                        .color(t.text),
                )
                .child(
                    text(now.map(|t| t.artist.clone()).unwrap_or_default())
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                ),
        )
        .child(spacer())
        .child(
            el().w(220.0)
                .h(4.0)
                .radius(2.0)
                .fill(Color::WHITE.alpha(0.15))
                .child(el().w(80.0).h(4.0).radius(2.0).fill(t.text)),
        );

    let page = col()
        .key("page")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .fill(t.card)
        .child(toolbar)
        .child(heading)
        .child(list);

    let mut root = col()
        .w_full()
        .h_full()
        .fill(t.bg)
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(sidebar)
                .child(page),
        )
        .child(bar);

    // Open and close both play: the dim fades, the card eases up, and an exit is
    // an exit, with no timer of mine to keep it on screen.
    if s.credits {
        let card = col()
            .key("credits-card")
            .w(360.0)
            .padding(20.0)
            .gap(10.0)
            .radius(14.0)
            .fill(t.card)
            .border(1.0, Color::WHITE.alpha(0.12))
            .transition(rune::Transition::opacity())
            .child(text("Credits").font(type_::TITLE).color(t.text))
            .child(
                text("Written by Thom Yorke\nPerformed by Jonny Greenwood")
                    .font(type_::BODY)
                    .color(t.text_secondary),
            )
            .child(
                text("Close")
                    .key("credits-close")
                    .font(type_::LABEL)
                    .color(accent)
                    .on_press(|s: &mut State| s.credits = false),
            );
        root = root.child(
            el().key("credits-scrim")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .fill(Color::BLACK.alpha(0.5))
                .center()
                .transition(rune::Transition::opacity())
                .on_press(|s: &mut State| s.credits = false)
                .child(card),
        );
    }
    root
}

fn track_row(s: &State, i: usize) -> El {
    let t = tok::theme(true);
    let track = &s.tracks[i];
    let liked = s.liked.contains(&i);
    let playing = s.playing == Some(i);
    let heart = el()
        .key(format!("like-{i}"))
        .size(28.0, 28.0)
        .radius(14.0)
        .center()
        .hover(Color::WHITE.alpha(0.08))
        .on_press(move |s: &mut State| {
            if !s.liked.remove(&i) {
                s.liked.insert(i);
            }
        })
        .child(
            text(if liked { "♥" } else { "♡" })
                .font(type_::BODY)
                .color(if liked {
                    tok::accent(0, true)
                } else {
                    t.text_secondary
                }),
        );
    row()
        .key(format!("row-{i}"))
        .w_full()
        .h(ROW_H)
        .px(24.0)
        .gap(14.0)
        .center_y()
        .hover(Color::WHITE.alpha(0.05))
        .on_press(move |s: &mut State| s.playing = Some(i))
        .child(
            el().size(36.0, 36.0)
                .radius(4.0)
                .fill(Color::WHITE.alpha(0.10)),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(track.title.clone())
                        .font(type_::BODY)
                        .color(if playing {
                            tok::accent(0, true)
                        } else {
                            t.text
                        })
                        .nowrap(),
                )
                .child(
                    text(track.artist.clone())
                        .font(type_::LABEL)
                        .color(t.text_secondary)
                        .nowrap(),
                ),
        )
        .child(
            text(track.album.clone())
                .font(type_::LABEL)
                .color(t.text_secondary)
                .w(180.0)
                .nowrap(),
        )
        .child(heart)
        .child(
            text(time(track.secs))
                .font(type_::LABEL)
                .color(t.text_secondary)
                .w(44.0),
        )
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("rail", |s| s.wide = false),
            ("credits", |s| s.credits = true),
            ("search", |s| {
                s.query = TextEdit::new("glass");
                s.refilter();
            }),
        ],
        ..AppSpec::new(
            "Sonder (Rune prototype)",
            (1180.0, 760.0),
            State::default,
            view,
        )
    });
}
