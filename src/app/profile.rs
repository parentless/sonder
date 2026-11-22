//! The Profile page: who is signed in, and a few things Spotify tells us about
//! how they listen.
//!
//! Everything here comes from Spotify and nothing is counted by Sonder: the
//! name and picture, the last 50 plays (by hour), the totals Sonder already
//! holds for Liked Songs and playlists, how many artists are followed, and the
//! top artists over three time ranges.
//!
//! One request opens the page, answered by the backend: the top artists for
//! each range and the follow count (kept on disk for hours), and the hours of
//! the last plays (kept for twenty minutes). Revisiting within five minutes
//! asks for nothing. The liked-song counts are worked out from the library
//! already in memory, once per change.
use super::page::PageEvent;
use super::*;
use gpui_kit::TestSupportExt as _;
use std::{sync::Arc, time::Instant};

/// Artists listed, the most played over six months first.
const ARTISTS_SHOWN: usize = 5;
/// Fewer plays than this are too few to name a listening habit.
const HABIT_MIN_PLAYS: usize = 10;
/// The page keeps what it has this long before it asks the backend again.
const STALE_AFTER: Duration = Duration::from_secs(5 * 60);
/// The colour a busy hour or a held chart place is drawn in.
const ACCENT_DARK: u32 = 0x6F95F0;
const ACCENT_LIGHT: u32 = 0x3F6FE0;
/// The tint the personality card fades in from.
const PERSONALITY_DARK: u32 = 0x27305A;
const PERSONALITY_LIGHT: u32 = 0xDCE4FF;
/// How long each section takes to settle in, and how far apart they start.
const RISE: Duration = Duration::from_millis(320);
const RISE_STEP: Duration = Duration::from_millis(40);
const HOUR_LABELS: [&str; 5] = ["12 am", "6 am", "12 pm", "6 pm", "11 pm"];
const BARS_HEIGHT: f32 = 96.;
const BAR_GAP: f32 = 5.;

struct Loaded {
    data: Arc<model::ProfileData>,
    at: Instant,
    /// Read from disk, so a request is still on its way to check it.
    saved: bool,
}

/// What the liked songs add up to, kept until the list or the artists change.
struct LikedTally {
    /// Where the list lives and how long it is: a cheap way to tell it changed.
    list: (usize, usize),
    artists: Vec<String>,
    counts: Vec<u32>,
    duration_ms: u64,
}

pub(super) struct ProfilePage {
    backend: BackendHandle,
    session: Entity<session::Session>,
    library: Entity<library::Library>,
    loaded: Option<Loaded>,
    task: Option<gpui_kit::Task<()>>,
    /// The read of the last saved answer, which fills the page at once.
    saved_task: Option<gpui_kit::Task<()>>,
    /// The saved sign-in lacks a permission the top artists need.
    permission_missing: bool,
    failed: bool,
    tally: Option<LikedTally>,
    scroll: gpui_kit::ScrollHandle,
    /// Changes whenever fresh numbers arrive, so the sections settle in again.
    arrival: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for ProfilePage {}

impl ProfilePage {
    pub(super) fn new(backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let session = services::AppServices::session(cx);
        let library = services::AppServices::library(cx);
        let subscriptions = vec![
            cx.subscribe(&session, |this, _, event: &session::SessionEvent, cx| {
                // A different account, or a fresh sign-in that may carry new
                // permissions: forget everything.
                if matches!(
                    event,
                    session::SessionEvent::LoggedOut
                        | session::SessionEvent::Restarted
                        | session::SessionEvent::Ready
                ) {
                    this.forget(cx);
                }
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.observe(&library, |_, _, cx| cx.notify()),
        ];
        Self {
            backend,
            session,
            library,
            loaded: None,
            task: None,
            saved_task: None,
            permission_missing: false,
            failed: false,
            tally: None,
            scroll: gpui_kit::ScrollHandle::new(),
            arrival: 0,
            _subscriptions: subscriptions,
        }
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.loaded = None;
        self.task = None;
        self.saved_task = None;
        self.permission_missing = false;
        self.failed = false;
        self.tally = None;
        cx.notify();
    }

    /// Called whenever the page is opened: back to the top, and the data
    /// brought up to date.
    pub(super) fn opened(&mut self, cx: &mut Context<Self>) {
        self.scroll
            .set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
        self.warm(cx);
    }

    /// Gets the data ready: what was saved last time shows at once, and one
    /// request checks it. Nothing is asked while the page holds a recent
    /// answer or a request is already out, so calling this early (when the
    /// account menu opens) costs the visit that follows nothing.
    pub(super) fn warm(&mut self, cx: &mut Context<Self>) {
        let fresh = self
            .loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.saved && loaded.at.elapsed() < STALE_AFTER);
        if fresh || self.task.is_some() || self.permission_missing {
            cx.notify();
            return;
        }
        self.failed = false;
        if self.loaded.is_none() && self.saved_task.is_none() {
            let saved = catalog::request(&self.backend, |respond| {
                BackendCommand::LoadProfileSaved { respond }
            });
            self.saved_task = Some(cx.spawn(async move |this, cx| {
                let saved = saved.await;
                let _ = this.update(cx, |page, cx| {
                    page.saved_task = None;
                    // The real answer may have come first; it wins.
                    if page.loaded.is_none()
                        && let Ok(Some(data)) = saved
                    {
                        page.show(data, true, cx);
                    }
                });
            }));
        }
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadProfile {
            respond,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |page, cx| {
                page.task = None;
                match result {
                    Ok(data) => page.show(data, false, cx),
                    Err(error) => {
                        page.permission_missing =
                            error.as_str() == model::HomePermissionMissing.to_string();
                        // A saved page stays as it is; only an empty one fails.
                        page.failed = !page.permission_missing && page.loaded.is_none();
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    /// Shows an answer. The sections settle in only when the numbers are new:
    /// a check that confirms what is already on screen changes nothing.
    fn show(&mut self, data: model::ProfileData, saved: bool, cx: &mut Context<Self>) {
        let same = self
            .loaded
            .as_ref()
            .is_some_and(|loaded| *loaded.data == data);
        if same {
            if let Some(loaded) = self.loaded.as_mut() {
                loaded.at = Instant::now();
                loaded.saved = saved;
            }
        } else {
            // Numbers replacing others already on screen just change in
            // place; only a first showing animates in.
            if self.loaded.is_none() {
                self.arrival += 1;
            }
            self.loaded = Some(Loaded {
                data: Arc::new(data),
                at: Instant::now(),
                saved,
            });
        }
        cx.notify();
    }

    /// Puts numbers in place without a request, for screenshots and tests.
    #[cfg(test)]
    pub(super) fn seed(&mut self, data: model::ProfileData, cx: &mut Context<Self>) {
        self.show(data, false, cx);
    }

    /// Keeps the skeleton on screen, for screenshots and tests.
    #[cfg(test)]
    pub(super) fn seed_loading(&mut self, cx: &mut Context<Self>) {
        self.loaded = None;
        self.task = None;
        cx.notify();
    }

    /// The liked songs' total length and each listed artist's share of them.
    /// Counted again only when the list or the artists change, so painting
    /// never walks thousands of songs.
    fn tally(&mut self, liked: &Arc<[model::Track]>, artists: &[&model::Artist]) -> &LikedTally {
        let ids: Vec<String> = artists
            .iter()
            .map(|artist| artist.source_id.clone())
            .collect();
        let list = (liked.as_ptr() as usize, liked.len());
        let current = self
            .tally
            .as_ref()
            .is_some_and(|tally| tally.list == list && tally.artists == ids);
        if !current {
            let counts = ids
                .iter()
                .map(|id| {
                    liked
                        .iter()
                        .filter(|song| {
                            song.artists
                                .iter()
                                .any(|credit| credit.source_id.as_deref() == Some(id.as_str()))
                        })
                        .count() as u32
                })
                .collect();
            self.tally = Some(LikedTally {
                list,
                artists: ids,
                counts,
                duration_ms: liked.iter().map(|song| u64::from(song.duration_ms)).sum(),
            });
        }
        self.tally.as_ref().expect("tally was just filled")
    }
}

/// 12-hour clock text: "12 am", "11 pm".
fn hour_text(hour: usize) -> String {
    let twelve = match hour % 12 {
        0 => 12,
        other => other,
    };
    format!("{twelve} {}", if hour % 24 < 12 { "am" } else { "pm" })
}

/// The busiest hour; the earliest when several tie.
fn busiest_hour(by_hour: &[u32; 24]) -> usize {
    let mut best = 0;
    for (hour, plays) in by_hour.iter().enumerate() {
        if *plays > by_hour[best] {
            best = hour;
        }
    }
    best
}

/// Plays in each hour of the day.
fn plays_by_hour(hours: &[u8]) -> [u32; 24] {
    let mut by_hour = [0u32; 24];
    for hour in hours {
        if let Some(slot) = by_hour.get_mut(usize::from(*hour)) {
            *slot += 1;
        }
    }
    by_hour
}

/// A part of the day, named by the kind of listener who favours it.
struct Habit {
    name: &'static str,
    /// First hour, and the hour after the last, on a 24-hour clock. A window
    /// that wraps past midnight has `to < from`.
    from: usize,
    to: usize,
}

const HABITS: [Habit; 4] = [
    Habit {
        name: "Night owl",
        from: 22,
        to: 4,
    },
    Habit {
        name: "Early bird",
        from: 4,
        to: 11,
    },
    Habit {
        name: "Daytripper",
        from: 11,
        to: 17,
    },
    Habit {
        name: "Sunsetter",
        from: 17,
        to: 22,
    },
];

impl Habit {
    fn covers(&self, hour: usize) -> bool {
        if self.from < self.to {
            (self.from..self.to).contains(&hour)
        } else {
            hour >= self.from || hour < self.to
        }
    }
}

/// The part of the day with the most plays, and how many plays fall in it.
fn habit_of(by_hour: &[u32; 24]) -> (&'static Habit, u32) {
    let within = |habit: &Habit| -> u32 {
        by_hour
            .iter()
            .enumerate()
            .filter(|(hour, _)| habit.covers(*hour))
            .map(|(_, plays)| *plays)
            .sum()
    };
    let mut best = &HABITS[0];
    for habit in &HABITS[1..] {
        if within(habit) > within(best) {
            best = habit;
        }
    }
    (best, within(best))
}

/// 12,345.
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// "271 h", or minutes below one hour.
fn hours_text(duration_ms: u64) -> String {
    let minutes = duration_ms / 60_000;
    if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h", thousands((minutes + 30) / 60))
    }
}

/// A Spotify id that reads as a username (older accounts) rather than the
/// random string newer ones get.
fn handle_of(id: &str) -> Option<&str> {
    let readable = !id.is_empty()
        && id.len() <= 24
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id.chars().any(|c| c.is_ascii_lowercase());
    // Random ids mix digits through the letters; a name does not.
    let mostly_letters = id.chars().filter(char::is_ascii_digit).count() * 3 <= id.len();
    (readable && mostly_letters).then_some(id)
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// The section's entrance: it fades in and settles 6px, each one a beat after
/// the one above it. Skipped when the listener asked for less motion.
fn rise(element: Div, id: &'static str, arrival: u64, index: u32, cx: &App) -> AnyElement {
    if cx.reduce_motion() || cfg!(test) {
        return element.into_any_element();
    }
    let delay = RISE_STEP * index;
    let total = delay + RISE;
    element
        .relative()
        .with_animation(
            (id, arrival),
            Animation::new(total).with_easing(ease_out_quint()),
            move |element, progress| {
                let elapsed = total.as_secs_f32() * progress;
                let local = ((elapsed - delay.as_secs_f32()) / RISE.as_secs_f32()).clamp(0., 1.);
                element.opacity(local).top(px(6. * (1. - local)))
            },
        )
        .into_any_element()
}

/// What the page shows of the signed-in account, gathered once per render.
struct Shown {
    name: SharedString,
    artwork: Option<String>,
    handle: Option<String>,
    id: String,
    liked: Option<usize>,
    playlists: Option<usize>,
}

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let compact = is_compact_content_layout(window);
        let light = palette.canvas == CadencePalette::LIGHT.canvas;
        let accent = if light { ACCENT_LIGHT } else { ACCENT_DARK };

        let profile = self.session.read(cx).profile().cloned();
        let library = self.library.read(cx);
        let liked_songs = library.liked_tracks().clone();
        let shown = Shown {
            name: profile.as_ref().map_or_else(
                || SharedString::new_static("Spotify account"),
                |profile| profile.display_name.clone(),
            ),
            artwork: profile
                .as_ref()
                .and_then(|profile| profile.artwork_url.clone()),
            handle: profile
                .as_ref()
                .and_then(|profile| handle_of(&profile.id))
                .map(str::to_owned),
            id: profile
                .as_ref()
                .map(|profile| profile.id.clone())
                .unwrap_or_default(),
            liked: library.loaded().then(|| liked_songs.len()),
            playlists: library.loaded().then(|| library.playlists().len()),
        };
        let data = self.loaded.as_ref().map(|loaded| loaded.data.clone());
        let arrival = self.arrival;

        let header = self.header(&shown, data.as_deref(), window, palette);
        let body = match (&data, self.permission_missing, self.failed) {
            (Some(data), _, _) => self
                .loaded_body(
                    &shown,
                    data,
                    &liked_songs,
                    palette,
                    accent,
                    light,
                    compact,
                    arrival,
                    cx,
                )
                .into_any_element(),
            (None, true, _) => self.permission_card(palette, cx).into_any_element(),
            (None, false, true) => components::empty_state(
                palette,
                CadenceIcon::Person,
                "Couldn’t load your profile",
                Some("Spotify didn’t answer. Open the page again to retry.".into()),
            )
            .into_any_element(),
            (None, false, false) => skeleton(palette, compact).into_any_element(),
        };

        div().size_full().flex().justify_center().child(
            div()
                .id("profile-page")
                .test_support()
                .h_full()
                .w_full()
                .max_w(Rems(72.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .w_full()
                        .px_8()
                        .pt_6()
                        .pb(Rems(3.5))
                        .child(rise(header, "profile-header", arrival, 0, cx))
                        .child(body),
                ),
        )
    }
}

impl ProfilePage {
    fn header(
        &self,
        shown: &Shown,
        data: Option<&model::ProfileData>,
        window: &Window,
        palette: CadencePalette,
    ) -> Div {
        let avatar_size = Rems(5.).to_pixels(window.rem_size());
        let avatar = Avatar::new()
            .with_size(avatar_size)
            .border_0()
            .name(components::initials(&shown.name));
        let avatar = match &shown.artwork {
            Some(url) => avatar.src(url.clone()),
            None => avatar,
        };
        let followed = data.and_then(|data| data.top.followed);
        let counts: Vec<String> = [
            shown.handle.as_ref().map(|handle| format!("@{handle}")),
            shown.liked.map(|liked| {
                format!(
                    "{} liked {}",
                    thousands(liked as u64),
                    plural(liked, "song", "songs")
                )
            }),
            shown.playlists.map(|count| {
                format!(
                    "{} {}",
                    thousands(count as u64),
                    plural(count, "playlist", "playlists")
                )
            }),
            followed.map(|followed| {
                format!(
                    "{} {} followed",
                    thousands(u64::from(followed)),
                    plural(followed as usize, "artist", "artists")
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        let address =
            (!shown.id.is_empty()).then(|| format!("https://open.spotify.com/user/{}", shown.id));
        div()
            .flex()
            .items_center()
            .gap_5()
            .mb(Rems(1.75))
            .child(div().flex_none().child(avatar))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(Rems(2.125))
                            .line_height(relative(1.1))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(rgb(palette.text_primary))
                            .truncate()
                            .child(shown.name.clone()),
                    )
                    .child(
                        div()
                            .mt_1p5()
                            .min_h(Rems(1.25))
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(counts.join(" · ")),
                    ),
            )
            .when_some(address, |row, address| {
                row.child(
                    div().flex_none().self_end().pb_1().child(
                        components::link(palette, "profile-open-spotify", window)
                            .text_size(Rems(0.78))
                            .text_color(rgb(palette.text_muted))
                            .child("Open in Spotify ↗")
                            .on_click(move |_, _, cx| cx.open_url(&address)),
                    ),
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn loaded_body(
        &mut self,
        shown: &Shown,
        data: &Arc<model::ProfileData>,
        liked_songs: &Arc<[model::Track]>,
        palette: CadencePalette,
        accent: u32,
        light: bool,
        compact: bool,
        arrival: u64,
        cx: &mut Context<Self>,
    ) -> Div {
        // Six months ranks the list; a new account may only have a shorter chart.
        let rows: Vec<&model::Artist> = [1usize, 2, 0]
            .into_iter()
            .map(|index| &data.top.ranges[index])
            .find(|artists| !artists.is_empty())
            .map(|artists| artists.iter().take(ARTISTS_SHOWN).collect())
            .unwrap_or_default();
        let (liked_by, liked_ms) = {
            let tally = self.tally(liked_songs, &rows);
            (tally.counts.clone(), tally.duration_ms)
        };
        let held = |artist: &model::Artist| -> [bool; 3] {
            [0, 1, 2].map(|range| {
                data.top.ranges[range]
                    .iter()
                    .any(|other| other.source_id == artist.source_id)
            })
        };
        let image_cache = services::AppServices::image_cache(cx);

        let top =
            match &data.recent_hours {
                None => self.permission_card(palette, cx).into_any_element(),
                Some(hours) => {
                    let by_hour = plays_by_hour(hours);
                    div()
                        .flex()
                        .gap_4()
                        .when(compact, |row| row.flex_col())
                        .child(rise(
                            div()
                                .flex_grow(1.7)
                                .flex_basis(px(0.))
                                .min_w_0()
                                .child(hours_card(palette, &by_hour, hours.len(), accent)),
                            "profile-hours",
                            arrival,
                            1,
                            cx,
                        ))
                        .child(rise(
                            div().flex_grow(1.).flex_basis(px(0.)).min_w_0().child(
                                personality_card(
                                    palette,
                                    &by_hour,
                                    hours.len(),
                                    if light {
                                        PERSONALITY_LIGHT
                                    } else {
                                        PERSONALITY_DARK
                                    },
                                ),
                            ),
                            "profile-habit",
                            arrival,
                            2,
                            cx,
                        ))
                        .into_any_element()
                }
            };

        let totals = [
            (
                "Liked songs",
                shown.liked.map(|liked| thousands(liked as u64)),
            ),
            (
                "Hours of liked music",
                shown.liked.map(|_| hours_text(liked_ms)),
            ),
            (
                "Playlists",
                shown.playlists.map(|count| thousands(count as u64)),
            ),
            (
                "Artists followed",
                Some(
                    data.top
                        .followed
                        .map_or_else(|| "–".to_owned(), |count| thousands(u64::from(count))),
                ),
            ),
        ];

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(top)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .children(
                        totals
                            .into_iter()
                            .enumerate()
                            .map(|(index, (label, value))| {
                                rise(
                                    div()
                                        .flex_grow(1.)
                                        .flex_basis(stat_basis(compact))
                                        .min_w_0()
                                        .child(stat_card(palette, label, value)),
                                    [
                                        "profile-total-0",
                                        "profile-total-1",
                                        "profile-total-2",
                                        "profile-total-3",
                                    ][index],
                                    arrival,
                                    3 + index as u32,
                                    cx,
                                )
                            }),
                    ),
            )
            .child(rise(
                div().child(artists_card(
                    palette,
                    &image_cache,
                    &rows,
                    shown.liked.is_some().then_some(liked_by.as_slice()),
                    held,
                    accent,
                    cx,
                )),
                "profile-artists",
                arrival,
                7,
                cx,
            ))
    }

    /// The ask for a permission the saved sign-in lacks.
    fn permission_card(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        card(palette)
            .flex()
            .flex_col()
            .items_start()
            .gap_3()
            .child(card_title(palette, "One more permission needed"))
            .child(components::page_detail(
                palette,
                "Your profile reads your top artists and the last plays Spotify keeps for you. Sign in again to allow it. Your settings and playlists stay.",
            ))
            .child(
                components::pill(palette, "profile-sign-in-again", "Sign in again", true)
                    .test_support()
                    .h_10()
                    .mt_1()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session.update(cx, |session, cx| session.logout(cx));
                    })),
            )
    }
}

/// A total's width before growing: four across, or two by two when narrow.
fn stat_basis(compact: bool) -> gpui_kit::Length {
    if compact {
        relative(0.45).into()
    } else {
        Rems(10.).into()
    }
}

/// The shell every section sits in.
fn card(palette: CadencePalette) -> Div {
    div()
        .rounded_2xl()
        .px_5()
        .py(Rems(1.125))
        .bg(palette.tile.color())
}

fn card_title(palette: CadencePalette, title: &'static str) -> Div {
    div()
        .text_size(Rems(1.1875))
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(rgb(palette.text_primary))
        .child(title)
}

fn caption(palette: CadencePalette, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Rems(0.78))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_muted))
        .child(text.into())
}

/// A total, or a bar in its place while the number is still on its way.
fn stat_card(palette: CadencePalette, label: &'static str, value: Option<String>) -> Div {
    card(palette).child(caption(palette, label)).child(
        div()
            .mt_2()
            .h(px(31.5))
            .flex()
            .items_center()
            .text_size(Rems(1.875))
            .line_height(relative(1.05))
            .font_weight(gpui_kit::FontWeight::BOLD)
            .text_color(rgb(palette.text_primary))
            .child(match value {
                Some(value) => div().child(value),
                None => div()
                    .w(px(110.))
                    .h(px(26.))
                    .rounded(px(6.))
                    .bg(rgb(palette.control_hover)),
            }),
    )
}

fn faint(palette: CadencePalette) -> gpui_kit::Hsla {
    gpui_kit::Hsla::from(rgb(palette.text_muted)).opacity(0.7)
}

/// The five hour marks under a row of 24 bars.
fn hour_marks(palette: CadencePalette) -> Div {
    div()
        .flex()
        .justify_between()
        .text_size(tokens::CAPTION_TEXT)
        .text_color(faint(palette))
        .children(HOUR_LABELS.map(|label| div().child(label)))
}

fn card_detail(palette: CadencePalette, text: &'static str) -> Div {
    div()
        .mt_1()
        .text_sm()
        .text_color(rgb(palette.text_muted))
        .child(text)
}

/// Plays in each hour as 24 bars, the busiest hour in the accent colour.
fn hours_card(palette: CadencePalette, by_hour: &[u32; 24], plays: usize, accent: u32) -> Div {
    let busiest = by_hour.iter().copied().max().unwrap_or_default();
    let peak = busiest_hour(by_hour);
    card(palette)
        .h_full()
        .child(card_title(palette, "When you listen"))
        .child(
            card_detail(
                palette,
                "Your last 50 plays, by hour. Spotify keeps no more than that.",
            )
            .mt_0p5()
            .mb(Rems(0.875)),
        )
        .child(if plays == 0 {
            div()
                .h(px(BARS_HEIGHT))
                .flex()
                .items_center()
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child("Nothing played lately.")
        } else {
            div()
                .h(px(BARS_HEIGHT))
                .flex()
                .items_end()
                .gap(px(BAR_GAP))
                .children(by_hour.iter().enumerate().map(|(hour, count)| {
                    let share = if busiest == 0 {
                        0.
                    } else {
                        *count as f32 / busiest as f32
                    };
                    div()
                        .flex_1()
                        .h(relative(share.max(0.04)))
                        .rounded(px(4.))
                        .bg(if busiest > 0 && hour == peak {
                            rgb(accent)
                        } else {
                            rgb(palette.card_hover)
                        })
                }))
        })
        .child(hour_marks(palette).mt_2())
}

/// "…of your last 50 plays were between 10 pm and 4 am. Peak hour: 11 pm." with
/// the hour in bold.
fn personality_text(habit: &Habit, within: u32, plays: usize, peak: &str) -> gpui_kit::StyledText {
    let lead = format!(
        "{within} of your last {plays} plays were between {} and {}. Peak hour: ",
        hour_text(habit.from),
        hour_text(habit.to),
    );
    let text = format!("{lead}{peak}.");
    let range = lead.len()..lead.len() + peak.len();
    gpui_kit::StyledText::new(text).with_highlights([(
        range,
        gpui_kit::HighlightStyle {
            font_weight: Some(gpui_kit::FontWeight::SEMIBOLD),
            ..gpui_kit::HighlightStyle::default()
        },
    )])
}

fn personality_card(palette: CadencePalette, by_hour: &[u32; 24], plays: usize, tint: u32) -> Div {
    let named = plays >= HABIT_MIN_PLAYS;
    let (habit, within) = habit_of(by_hour);
    card(palette)
        .h_full()
        .flex()
        .flex_col()
        .justify_center()
        .bg(gpui_kit::linear_gradient(
            160.,
            gpui_kit::linear_color_stop(rgb(tint), 0.),
            gpui_kit::linear_color_stop(palette.tile.color(), 0.8),
        ))
        .child(caption(palette, "You are a"))
        .child(
            div()
                .mt_2()
                .text_size(Rems(2.5))
                .line_height(relative(1.05))
                .font_weight(gpui_kit::FontWeight::EXTRA_BOLD)
                .text_color(rgb(palette.text_primary))
                .child(if named { habit.name } else { "Newcomer" }),
        )
        .child(
            div()
                .mt(Rems(0.625))
                .text_size(Rems(0.84))
                .line_height(relative(1.5))
                .text_color(rgb(palette.text_muted))
                .child(if named {
                    personality_text(habit, within, plays, &hour_text(busiest_hour(by_hour)))
                        .into_any_element()
                } else {
                    div()
                        .child(
                            "Play a few more songs and Spotify will have enough to name your habit.",
                        )
                        .into_any_element()
                }),
        )
}

const ROW_INSET: f32 = 8.;
const ROW_GAP: f32 = 14.;
const DOT: f32 = 9.;
const DOT_GAP: f32 = 5.;
const LIKED_WIDTH: f32 = 64.;

/// 4 weeks, 6 months and a year: one dot each, filled where the artist is in
/// that chart.
fn range_dots(palette: CadencePalette, held: [bool; 3], accent: u32) -> Div {
    div()
        .flex_none()
        .flex()
        .gap(px(DOT_GAP))
        .children(held.map(|held| {
            div().size(px(DOT)).rounded_full().bg(if held {
                rgb(accent)
            } else {
                rgb(palette.card_hover)
            })
        }))
}

fn genre_chip(palette: CadencePalette, genre: &str) -> Div {
    div()
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .bg(rgb(palette.control_hover))
        .text_size(Rems(0.75))
        .text_color(rgb(palette.text))
        .child(genre.to_owned())
}

fn artists_card(
    palette: CadencePalette,
    image_cache: &Entity<image_cache::BoundedImageCache>,
    artists: &[&model::Artist],
    // None until the library has loaded, so no count is shown.
    liked_by: Option<&[u32]>,
    held: impl Fn(&model::Artist) -> [bool; 3],
    accent: u32,
    cx: &mut Context<ProfilePage>,
) -> Div {
    card(palette)
        .child(card_title(palette, "Your top artists"))
        .child(
            card_detail(
                palette,
                "Who stays in your charts. Dots show 4 weeks, 6 months and a year.",
            )
            .mt_1()
            .mb(Rems(0.75)),
        )
        .when(artists.is_empty(), |card| {
            card.child(
                div()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child("Spotify has no top artists for you yet. Listen a little more."),
            )
        })
        .children(artists.iter().enumerate().map(|(index, artist)| {
            let target = model::ArtistRef {
                name: artist.name.clone(),
                source_id: Some(artist.source_id.clone()),
                spotify_uri: artist.spotify_uri.clone(),
            };
            components::bare_button(("profile-artist", index))
                .test_support()
                .w_full()
                .justify_start()
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PageEvent::OpenArtist(target.clone()));
                }))
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .px(px(ROW_INSET))
                .py(px(7.))
                .rounded(px(10.))
                .hover(|row| row.bg(palette.tile_hover.color()))
                .child(
                    div()
                        .w(px(18.))
                        .flex_none()
                        .text_right()
                        .text_size(Rems(0.8125))
                        .text_color(faint(palette))
                        .child((index + 1).to_string()),
                )
                .child(components::artwork(
                    palette,
                    image_cache,
                    artist.artwork_url.as_deref(),
                    tokens::PROFILE_ARTIST_ARTWORK,
                    CadenceIcon::Person,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(Rems(0.906))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .truncate()
                                .child(artist.name.clone()),
                        )
                        .when(!artist.genres.is_empty(), |text| {
                            text.child(
                                div().mt(px(4.)).flex().gap(px(4.)).children(
                                    artist
                                        .genres
                                        .iter()
                                        .take(2)
                                        .map(|genre| genre_chip(palette, genre)),
                                ),
                            )
                        }),
                )
                .child(range_dots(palette, held(artist), accent))
                .child(
                    div()
                        .w(px(LIKED_WIDTH))
                        .flex_none()
                        .text_right()
                        .text_size(Rems(0.78))
                        .text_color(rgb(palette.text_muted))
                        .child(liked_by.map_or_else(String::new, |counts| {
                            format!(
                                "{} liked",
                                thousands(u64::from(counts.get(index).copied().unwrap_or(0)))
                            )
                        })),
                )
        }))
}

/// The same layout with every number and name left blank. Static: no shimmer
/// timer runs, and it disappears the moment the numbers land.
fn skeleton(palette: CadencePalette, compact: bool) -> Div {
    let block = |width: f32, height: f32| {
        div()
            .w(px(width))
            .h(px(height))
            .rounded(px(6.))
            .bg(rgb(palette.control_hover))
    };
    let hours = card(palette)
        .h_full()
        .child(card_title(palette, "When you listen"))
        .child(
            card_detail(
                palette,
                "Your last 50 plays, by hour. Spotify keeps no more than that.",
            )
            .mt_0p5()
            .mb(Rems(0.875)),
        )
        .child(
            div()
                .h(px(BARS_HEIGHT))
                .flex()
                .items_end()
                .gap(px(BAR_GAP))
                .children((0..24).map(|index| {
                    div()
                        .flex_1()
                        .h(relative(0.18 + 0.5 * (((index * 7) % 5) as f32 / 4.)))
                        .rounded(px(4.))
                        .bg(rgb(palette.control_hover))
                })),
        )
        .child(hour_marks(palette).mt_2().opacity(0.0));
    let personality = card(palette)
        .h_full()
        .flex()
        .flex_col()
        .justify_center()
        .child(caption(palette, "You are a").opacity(0.0))
        .child(block(170., 36.).mt_2())
        .child(block(260., 10.).mt(Rems(0.875)))
        .child(block(220., 10.).mt_2());
    let totals = (0..4).map(|_| {
        div()
            .flex_grow(1.)
            .flex_basis(stat_basis(compact))
            .min_w_0()
            .child(
                card(palette)
                    // Each bar sits in a box as tall as the text it stands for.
                    .child(
                        div()
                            .h(px(20.25))
                            .flex()
                            .items_center()
                            .child(block(96., 10.)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .h(px(31.5))
                            .flex()
                            .items_center()
                            .child(block(110., 26.)),
                    ),
            )
    });
    let rows = (0..ARTISTS_SHOWN).map(|_| {
        div()
            .flex()
            .items_center()
            .gap(px(ROW_GAP))
            .px(px(ROW_INSET))
            .py(px(7.))
            .child(div().w(px(18.)).flex_none())
            .child(
                div()
                    .size(Rems(2.5))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(palette.control_hover)),
            )
            .child(
                div()
                    .flex_1()
                    .child(block(120., 12.))
                    .child(block(150., 18.).mt(px(8.))),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(DOT_GAP))
                    .children((0..3).map(|_| {
                        div()
                            .size(px(DOT))
                            .rounded_full()
                            .bg(rgb(palette.control_hover))
                    })),
            )
            .child(
                div()
                    .w(px(LIKED_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(block(48., 8.)),
            )
    });
    let artists = card(palette)
        .child(card_title(palette, "Your top artists"))
        .child(
            card_detail(
                palette,
                "Who stays in your charts. Dots show 4 weeks, 6 months and a year.",
            )
            .mt_1()
            .mb(Rems(0.75)),
        )
        .children(rows);

    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .gap_4()
                .when(compact, |row| row.flex_col())
                .child(
                    div()
                        .flex_grow(1.7)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .child(hours),
                )
                .child(
                    div()
                        .flex_grow(1.)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .child(personality),
                ),
        )
        .child(div().flex().flex_wrap().gap_4().children(totals))
        .child(artists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_read_on_a_twelve_hour_clock() {
        assert_eq!(hour_text(0), "12 am");
        assert_eq!(hour_text(9), "9 am");
        assert_eq!(hour_text(12), "12 pm");
        assert_eq!(hour_text(23), "11 pm");
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn liked_music_is_shown_in_hours() {
        assert_eq!(hours_text(0), "0 min");
        assert_eq!(hours_text(59 * 60_000), "59 min");
        assert_eq!(hours_text(271 * 3_600_000), "271 h");
        assert_eq!(hours_text(90 * 60_000), "2 h");
    }

    #[test]
    fn plays_are_counted_into_their_hour() {
        let by_hour = plays_by_hour(&[23, 23, 1, 9, 99]);
        assert_eq!(by_hour[23], 2);
        assert_eq!(by_hour[1], 1);
        assert_eq!(by_hour.iter().sum::<u32>(), 4);
        assert_eq!(busiest_hour(&by_hour), 23);
    }

    #[test]
    fn the_busiest_part_of_the_day_names_the_listener() {
        let mut hours = [0u32; 24];
        hours[23] = 20;
        hours[1] = 10;
        hours[9] = 4;
        let (habit, within) = habit_of(&hours);
        assert_eq!(habit.name, "Night owl");
        assert_eq!(within, 30);

        let mut morning = [0u32; 24];
        morning[7] = 10;
        assert_eq!(habit_of(&morning).0.name, "Early bird");
    }

    #[test]
    fn a_window_past_midnight_covers_both_sides() {
        let owl = &HABITS[0];
        assert!(owl.covers(22) && owl.covers(23) && owl.covers(0) && owl.covers(3));
        assert!(!owl.covers(4) && !owl.covers(12));
    }

    #[test]
    fn only_readable_ids_are_shown_as_a_handle() {
        assert_eq!(handle_of("samarden"), Some("samarden"));
        assert_eq!(handle_of("sam.arden_1"), Some("sam.arden_1"));
        assert_eq!(handle_of("31zk3x7a9q2m4p6r8t0v1w5y3u7e"), None);
        assert_eq!(handle_of(""), None);
    }
}
