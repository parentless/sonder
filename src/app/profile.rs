//! The Profile page: who is signed in, and how they listen.
//!
//! Spotify no longer shares much about an account (no country, plan or
//! followers), so the page leans on what Sonder counts itself: every song
//! started is logged, and the page turns that log into a weekday-by-hour heat
//! map, a listening personality, a few totals and when each favourite artist
//! gets played. The log starts on the day this feature shipped, so the page
//! says the stats count from when Sonder began tracking.
//!
//! The numbers are three small queries on a background thread, run each time
//! the page opens. Nothing runs while it is closed. Until they arrive the page
//! shows a skeleton with the same layout, so nothing moves when they land.
use super::top_listening::TopListening;
use super::*;
use crate::storage::{ArtistHours, ListeningStats, Store};
use gpui_kit::TestSupportExt as _;
use std::sync::Arc;

/// The artists shown, the most played first.
const ARTISTS_SHOWN: usize = 5;
/// The colour a busy hour is drawn in, and how faint the quietest one is.
const HEAT_DARK: u32 = 0x6F95F0;
const HEAT_LIGHT: u32 = 0x3F6FE0;
const HEAT_FLOOR: f32 = 0.08;
const HEAT_RANGE: f32 = 0.9;
/// The tint the personality card fades in from.
const PERSONALITY_DARK: u32 = 0x27305A;
const PERSONALITY_LIGHT: u32 = 0xDCE4FF;
/// How long each section takes to settle in, and how far apart they start.
const RISE: Duration = Duration::from_millis(320);
const RISE_STEP: Duration = Duration::from_millis(40);
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const HOUR_LABELS: [&str; 5] = ["12 am", "6 am", "12 pm", "6 pm", "11 pm"];

enum Load {
    Loading,
    Ready(Arc<ListeningStats>),
    Failed,
}

pub(super) struct ProfilePage {
    session: Entity<session::Session>,
    library: Entity<library::Library>,
    top: Entity<TopListening>,
    state: Load,
    task: Option<gpui_kit::Task<()>>,
    scroll: gpui_kit::ScrollHandle,
    /// Changes whenever the numbers arrive, so the sections settle in again.
    arrival: u64,
    _subscriptions: Vec<Subscription>,
}

impl ProfilePage {
    pub(super) fn new(backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let session = services::AppServices::session(cx);
        let library = services::AppServices::library(cx);
        let top = TopListening::shared(&backend, cx);
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.observe(&library, |_, _, cx| cx.notify()),
            // Artist pictures come from the charts Home also reads.
            cx.observe(&top, |_, _, cx| cx.notify()),
        ];
        Self {
            session,
            library,
            top,
            state: Load::Loading,
            task: None,
            scroll: gpui_kit::ScrollHandle::new(),
            arrival: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Called whenever the page is opened: back to the top, and the counts
    /// are read afresh.
    pub(super) fn opened(&mut self, cx: &mut Context<Self>) {
        self.scroll
            .set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
        self.top
            .update(cx, |top, cx| top.ensure(model::HomeRange::AllTime, cx));
        // Tests and screenshots seed the numbers; they must not read the real
        // database.
        if cfg!(test) {
            cx.notify();
            return;
        }
        if !matches!(self.state, Load::Ready(_)) {
            self.state = Load::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { Store::open_default().and_then(|store| store.listening_stats()) })
                .await;
            let _ = this.update(cx, |page, cx| {
                page.task = None;
                match result {
                    Ok(stats) => page.show(stats, cx),
                    Err(error) => {
                        log::warn!("could not read the listening stats: {error:#}");
                        page.state = Load::Failed;
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    fn show(&mut self, stats: ListeningStats, cx: &mut Context<Self>) {
        self.state = Load::Ready(Arc::new(stats));
        self.arrival += 1;
        cx.notify();
    }

    /// Puts numbers in place without reading the database, for screenshots
    /// and tests.
    #[cfg(test)]
    pub(super) fn seed(&mut self, stats: ListeningStats, cx: &mut Context<Self>) {
        self.show(stats, cx);
    }

    /// Keeps the skeleton on screen, for screenshots and tests.
    #[cfg(test)]
    pub(super) fn seed_loading(&mut self, cx: &mut Context<Self>) {
        self.state = Load::Loading;
        cx.notify();
    }

    /// An artist's picture, when one of the charts has it.
    fn artist_art(&self, name: &str, cx: &App) -> Option<String> {
        let top = self.top.read(cx);
        model::HomeRange::ALL.into_iter().find_map(|range| {
            top.data(range)?
                .artists
                .iter()
                .find(|artist| artist.name.eq_ignore_ascii_case(name))
                .and_then(|artist| artist.artwork_url.clone())
        })
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

/// The busiest hour by minutes played; the earliest when several tie.
fn busiest_hour(by_hour: &[u32; 24]) -> usize {
    let mut best = 0;
    for (hour, minutes) in by_hour.iter().enumerate() {
        if *minutes > by_hour[best] {
            best = hour;
        }
    }
    best
}

fn minutes_by_hour(heat: &[[u32; 24]; 7]) -> [u32; 24] {
    let mut hours = [0u32; 24];
    for day in heat {
        for (hour, minutes) in day.iter().enumerate() {
            hours[hour] = hours[hour].saturating_add(*minutes);
        }
    }
    hours
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

/// The part of the day with the most listening, and its share in percent.
fn habit_of(by_hour: &[u32; 24]) -> (&'static Habit, u32) {
    let total: u64 = by_hour.iter().map(|minutes| u64::from(*minutes)).sum();
    let share = |habit: &Habit| -> u64 {
        by_hour
            .iter()
            .enumerate()
            .filter(|(hour, _)| habit.covers(*hour))
            .map(|(_, minutes)| u64::from(*minutes))
            .sum()
    };
    let mut best = &HABITS[0];
    for habit in &HABITS[1..] {
        if share(habit) > share(best) {
            best = habit;
        }
    }
    let percent = (share(best) * 100).checked_div(total).unwrap_or(0) as u32;
    (best, percent)
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

/// "74 min", or "2 h 10 min" from two hours up.
fn duration_text(minutes: u64) -> String {
    if minutes >= 120 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

fn days_text(days: u32) -> String {
    format!("{days} {}", if days == 1 { "day" } else { "days" })
}

/// How strongly a cell is drawn, from the faintest to the full colour.
fn heat_alpha(value: u32, busiest: u32) -> f32 {
    let share = if busiest == 0 {
        0.
    } else {
        value as f32 / busiest as f32
    };
    HEAT_FLOOR + share * HEAT_RANGE
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

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let compact = is_compact_content_layout(window);
        let light = palette.canvas == CadencePalette::LIGHT.canvas;
        let heat_color = if light { HEAT_LIGHT } else { HEAT_DARK };

        let (name, artwork) = self.session.read(cx).profile().map_or_else(
            || (SharedString::new_static("Spotify account"), None),
            |profile| (profile.display_name.clone(), profile.artwork_url.clone()),
        );
        let library = self.library.read(cx);
        let detail = format!(
            "{} liked {} · {} {}",
            thousands(library.liked_tracks().len() as u64),
            if library.liked_tracks().len() == 1 {
                "song"
            } else {
                "songs"
            },
            thousands(library.playlists().len() as u64),
            if library.playlists().len() == 1 {
                "playlist"
            } else {
                "playlists"
            },
        );
        let arrival = self.arrival;

        let header = {
            let avatar_size = Rems(5.).to_pixels(window.rem_size());
            let avatar = Avatar::new()
                .with_size(avatar_size)
                .border_0()
                .name(components::initials(&name));
            let avatar = match artwork {
                Some(url) => avatar.src(url),
                None => avatar,
            };
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
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .mt_1p5()
                                .text_sm()
                                .text_color(rgb(palette.text_muted))
                                .child(detail),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .self_end()
                        .pb_1()
                        .text_size(Rems(0.78))
                        .text_color(rgb(palette.text_muted))
                        .child("* All-time data"),
                )
        };

        let body = match &self.state {
            Load::Loading => skeleton(palette, compact).into_any_element(),
            Load::Failed => components::empty_state(
                palette,
                CadenceIcon::Clock,
                "Couldn’t read your listening",
                Some("Sonder couldn’t open its listening log. Reopen the page to try again.".into()),
            )
            .into_any_element(),
            Load::Ready(stats) if stats.plays == 0 => components::empty_state(
                palette,
                CadenceIcon::Clock,
                "Your habits will show up here",
                Some(
                    "Play some music in Sonder and this page fills in with when you listen and which artists you play at what hour."
                        .into(),
                ),
            )
            .into_any_element(),
            Load::Ready(stats) => {
                let stats = stats.clone();
                self.loaded(&stats, palette, heat_color, light, compact, arrival, cx)
                    .into_any_element()
            }
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
    #[allow(clippy::too_many_arguments)]
    fn loaded(
        &self,
        stats: &ListeningStats,
        palette: CadencePalette,
        heat_color: u32,
        light: bool,
        compact: bool,
        arrival: u64,
        cx: &mut Context<Self>,
    ) -> Div {
        let by_hour = minutes_by_hour(&stats.heat);
        let (habit, percent) = habit_of(&by_hour);
        let peak = hour_text(busiest_hour(&by_hour));
        let per_day = stats.minutes / u64::from(stats.tracked_days.max(1));
        let totals = [
            ("Minutes listened", thousands(stats.minutes)),
            ("Songs played", thousands(stats.plays)),
            ("Avg per day", duration_text(per_day)),
            ("Day streak", days_text(stats.streak_days)),
        ];

        let heat = heat_card(palette, &stats.heat, heat_color);
        let personality = personality_card(
            palette,
            habit,
            percent,
            &peak,
            if light {
                PERSONALITY_LIGHT
            } else {
                PERSONALITY_DARK
            },
        );
        let image_cache = services::AppServices::image_cache(cx);
        let artists = artists_card(
            palette,
            &image_cache,
            &stats.top_artists,
            |name| self.artist_art(name, cx),
            heat_color,
        );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(compact, |row| row.flex_col())
                    .child(rise(
                        div().flex_grow(1.7).flex_basis(px(0.)).min_w_0().child(heat),
                        "profile-heat",
                        arrival,
                        1,
                        cx,
                    ))
                    .child(rise(
                        div().flex_grow(1.).flex_basis(px(0.)).min_w_0().child(personality),
                        "profile-habit",
                        arrival,
                        2,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .children(totals.into_iter().enumerate().map(|(index, (label, value))| {
                        rise(
                            div()
                                .flex_grow(1.)
                                .flex_basis(stat_basis(compact))
                                .min_w_0()
                                .child(stat_card(palette, label, value)),
                            ["profile-total-0", "profile-total-1", "profile-total-2", "profile-total-3"]
                                [index],
                            arrival,
                            3 + index as u32,
                            cx,
                        )
                    })),
            )
            .child(rise(div().child(artists), "profile-artists", arrival, 7, cx))
            .child(
                div()
                    .mt_2p5()
                    .px_0p5()
                    .text_size(Rems(0.75))
                    .text_color(rgb(palette.text_muted))
                    .opacity(0.8)
                    .child(
                        "* All-time data. Listening stats count from the day Sonder began tracking on this PC.",
                    ),
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
        .bg(rgb(palette.card))
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

fn stat_card(palette: CadencePalette, label: &'static str, value: String) -> Div {
    card(palette).child(caption(palette, label)).child(
        div()
            .mt_2()
            .text_size(Rems(1.875))
            .line_height(relative(1.05))
            .font_weight(gpui_kit::FontWeight::BOLD)
            .text_color(rgb(palette.text_primary))
            .child(value),
    )
}

fn faint(palette: CadencePalette) -> gpui_kit::Hsla {
    gpui_kit::Hsla::from(rgb(palette.text_muted)).opacity(0.7)
}

/// The five hour marks under a strip of 24 cells.
fn hour_marks(palette: CadencePalette) -> Div {
    div()
        .flex()
        .justify_between()
        .text_size(tokens::CAPTION_TEXT)
        .text_color(faint(palette))
        .children(HOUR_LABELS.map(|label| div().child(label)))
}

const CELL_GAP: f32 = 3.;

fn heat_card(palette: CadencePalette, heat: &[[u32; 24]; 7], color: u32) -> Div {
    let busiest = heat.iter().flatten().copied().max().unwrap_or_default();
    card(palette)
        .h_full()
        .child(card_title(palette, "When you listen").mb(Rems(0.875)))
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(34.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(CELL_GAP))
                        .text_size(tokens::CAPTION_TEXT)
                        .text_color(faint(palette))
                        .children(
                            WEEKDAYS
                                .map(|day| div().h(Rems(1.125)).flex().items_center().child(day)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(CELL_GAP))
                        .children(heat.iter().enumerate().map(|(day, hours)| {
                            div()
                                .flex()
                                .gap(px(CELL_GAP))
                                .children(hours.iter().enumerate().map(move |(hour, minutes)| {
                                    div()
                                        .id(("heat", day * 24 + hour))
                                        .flex_1()
                                        .h(Rems(1.125))
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(gpui_kit::transparent_black())
                                        .bg(rgb(color).opacity(heat_alpha(*minutes, busiest)))
                                        .hover(|cell| cell.border_color(rgb(palette.text_primary)))
                                }))
                        })),
                ),
        )
        .child(hour_marks(palette).mt_2().ml(px(42.)))
}

/// "…between 10 pm and 4 am. Peak hour: 11 pm." with the hour in bold.
fn personality_text(habit: &Habit, percent: u32, peak: &str) -> gpui_kit::StyledText {
    let lead = format!(
        "{percent}% of your listening happens between {} and {}. Peak hour: ",
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

fn personality_card(
    palette: CadencePalette,
    habit: &Habit,
    percent: u32,
    peak: &str,
    tint: u32,
) -> Div {
    card(palette)
        .h_full()
        .flex()
        .flex_col()
        .justify_center()
        .bg(gpui_kit::linear_gradient(
            160.,
            gpui_kit::linear_color_stop(rgb(tint), 0.),
            gpui_kit::linear_color_stop(rgb(palette.card), 0.8),
        ))
        .child(caption(palette, "You are a"))
        .child(
            div()
                .mt_2()
                .text_size(Rems(2.5))
                .line_height(relative(1.05))
                .font_weight(gpui_kit::FontWeight::EXTRA_BOLD)
                .text_color(rgb(palette.text_primary))
                .child(habit.name),
        )
        .child(
            div()
                .mt(Rems(0.625))
                .text_size(Rems(0.84))
                .line_height(relative(1.5))
                .text_color(rgb(palette.text_muted))
                .child(personality_text(habit, percent, peak)),
        )
}

const ARTIST_NAME_WIDTH: f32 = 150.;
const ARTIST_PEAK_WIDTH: f32 = 92.;
const ARTIST_ROW_GAP: f32 = 14.;
const ARTIST_ROW_INSET: f32 = 8.;

fn artists_card(
    palette: CadencePalette,
    image_cache: &Entity<image_cache::BoundedImageCache>,
    artists: &[ArtistHours],
    art: impl Fn(&str) -> Option<String>,
    color: u32,
) -> Div {
    card(palette)
        .child(card_title(palette, "When you play your artists"))
        .child(
            div()
                .mt_1()
                .mb(Rems(0.875))
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child("Each row is an artist’s 24 hours. Brighter means more plays."),
        )
        .children(
            artists
                .iter()
                .take(ARTISTS_SHOWN)
                .enumerate()
                .map(|(index, artist)| {
                    let busiest = artist.hours.iter().copied().max().unwrap_or_default();
                    let peak = hour_text(busiest_hour(&artist.hours));
                    div()
                        .id(("profile-artist", index))
                        .test_support()
                        .flex()
                        .items_center()
                        .gap(px(ARTIST_ROW_GAP))
                        .px(px(ARTIST_ROW_INSET))
                        .py(px(7.))
                        .rounded(px(10.))
                        .hover(|row| row.bg(rgb(palette.card_hover)))
                        .child(
                            div()
                                .w(px(ARTIST_NAME_WIDTH))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2p5()
                                .child(components::artwork(
                                    palette,
                                    image_cache,
                                    art(&artist.name).as_deref(),
                                    tokens::SUGGESTION_ARTIST_ARTWORK,
                                    CadenceIcon::Person,
                                ))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(Rems(0.906))
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette.text_primary))
                                        .truncate()
                                        .child(artist.name.clone()),
                                ),
                        )
                        .child(div().flex_1().min_w_0().flex().gap(px(CELL_GAP)).children(
                            artist.hours.iter().map(|plays| {
                                div()
                                    .flex_1()
                                    .h(Rems(1.375))
                                    .rounded(px(3.))
                                    .bg(rgb(color).opacity(heat_alpha(*plays, busiest)))
                            }),
                        ))
                        .child(
                            div()
                                .w(px(ARTIST_PEAK_WIDTH))
                                .flex_none()
                                .text_right()
                                .text_size(Rems(0.78))
                                .text_color(rgb(palette.text_muted))
                                .child(format!("peaks {peak}")),
                        )
                }),
        )
        .child(
            hour_marks(palette)
                .mt_1p5()
                .ml(px(ARTIST_ROW_INSET + ARTIST_NAME_WIDTH + ARTIST_ROW_GAP))
                .mr(px(ARTIST_ROW_INSET + ARTIST_PEAK_WIDTH + ARTIST_ROW_GAP)),
        )
}

/// The same layout with every number and name left blank. Static: no shimmer
/// timer runs, and it disappears the moment the numbers land.
fn skeleton(palette: CadencePalette, compact: bool) -> Div {
    let block = |width: f32, height: f32| {
        div()
            .w(px(width))
            .h(px(height))
            .rounded(px(6.))
            .bg(rgb(palette.control))
    };
    let cell = || {
        div()
            .flex_1()
            .h(Rems(1.125))
            .rounded(px(3.))
            .bg(rgb(palette.control_hover))
    };
    let heat =
        card(palette)
            .h_full()
            .child(card_title(palette, "When you listen").mb(Rems(0.875)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(34.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(CELL_GAP))
                            .children(WEEKDAYS.map(|_| {
                                div()
                                    .h(Rems(1.125))
                                    .flex()
                                    .items_center()
                                    .child(block(22., 8.))
                            })),
                    )
                    .child(div().flex_1().flex().flex_col().gap(px(CELL_GAP)).children(
                        (0..7).map(|_| {
                            div()
                                .flex()
                                .gap(px(CELL_GAP))
                                .children((0..24).map(|_| cell()))
                        }),
                    )),
            )
            .child(hour_marks(palette).mt_2().ml(px(42.)).opacity(0.0));
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
            .gap(px(ARTIST_ROW_GAP))
            .px(px(ARTIST_ROW_INSET))
            .py(px(7.))
            .child(
                div()
                    .w(px(ARTIST_NAME_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        div()
                            .size(Rems(2.125))
                            .rounded_full()
                            .bg(rgb(palette.control)),
                    )
                    .child(block(80., 10.)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap(px(CELL_GAP))
                    .children((0..24).map(|_| {
                        div()
                            .flex_1()
                            .h(Rems(1.375))
                            .rounded(px(3.))
                            .bg(rgb(palette.control_hover))
                    })),
            )
            .child(
                div()
                    .w(px(ARTIST_PEAK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(block(56., 8.)),
            )
    });
    let artists = card(palette)
        .child(card_title(palette, "When you play your artists"))
        .child(
            div()
                .mt_1()
                .mb(Rems(0.875))
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .child("Each row is an artist’s 24 hours. Brighter means more plays."),
        )
        .children(rows)
        .child(
            hour_marks(palette)
                .mt_1p5()
                .ml(px(ARTIST_ROW_INSET + ARTIST_NAME_WIDTH + ARTIST_ROW_GAP))
                .mr(px(ARTIST_ROW_INSET + ARTIST_PEAK_WIDTH + ARTIST_ROW_GAP))
                .opacity(0.0),
        );

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
                        .child(heat),
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
        assert_eq!(thousands(58_940), "58,940");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn long_listening_is_shown_in_hours() {
        assert_eq!(duration_text(74), "74 min");
        assert_eq!(duration_text(130), "2 h 10 min");
        assert_eq!(days_text(1), "1 day");
        assert_eq!(days_text(41), "41 days");
    }

    #[test]
    fn the_busiest_part_of_the_day_names_the_listener() {
        let mut hours = [0u32; 24];
        hours[23] = 50;
        hours[1] = 30;
        hours[9] = 20;
        let (habit, percent) = habit_of(&hours);
        assert_eq!(habit.name, "Night owl");
        assert_eq!(percent, 80);
        assert_eq!(busiest_hour(&hours), 23);

        let mut morning = [0u32; 24];
        morning[7] = 10;
        assert_eq!(habit_of(&morning).0.name, "Early bird");
        assert_eq!(habit_of(&[0u32; 24]).1, 0);
    }

    #[test]
    fn a_window_past_midnight_covers_both_sides() {
        let owl = &HABITS[0];
        assert!(owl.covers(22) && owl.covers(23) && owl.covers(0) && owl.covers(3));
        assert!(!owl.covers(4) && !owl.covers(12));
    }

    #[test]
    fn the_quietest_hour_is_faint_and_the_busiest_is_full() {
        assert!((heat_alpha(0, 10) - HEAT_FLOOR).abs() < 1e-6);
        assert!((heat_alpha(10, 10) - (HEAT_FLOOR + HEAT_RANGE)).abs() < 1e-6);
        assert!((heat_alpha(0, 0) - HEAT_FLOOR).abs() < 1e-6);
    }
}
