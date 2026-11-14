//! Lyrics for the playing song, in a side panel, over the whole page, or as one
//! line above the player.
//!
//! Nothing is asked of the network until this view is on screen, and a song is
//! asked for once: answers are kept for the last few songs. While a synced song
//! plays, a single timer is set for the moment the next line starts, so the view
//! redraws a few times a minute rather than every frame. A line change glides:
//! for a third of a second the lines ease up while the old line shrinks and the
//! new one grows, then the view goes quiet again.

use std::collections::VecDeque;

use super::*;
use crate::{
    backend::BackendCommand,
    lyrics::{Line, Lyrics, Query},
};
use gpui_kit::TestSupportExt as _;

/// How many songs' answers are kept, including songs with no lyrics.
const CACHE_ENTRIES: usize = 16;
/// The soonest a line change is scheduled, so a clock that runs a hair early
/// cannot spin the timer.
const MIN_DELAY_MS: u32 = 30;
/// The most lines drawn for one song.
const MAX_LINES: usize = 300;
/// How long moving on to the next line takes.
const TRANSITION: Duration = Duration::from_millis(380);
/// Where the current line sits, as a share of the height.
const CURRENT_AT: f32 = 0.34;

enum State {
    Idle,
    Loading,
    Ready(Arc<Lyrics>),
    Missing,
    Failed,
}

/// Raised when the listener closes the lyrics.
pub(super) struct CloseLyrics;

impl EventEmitter<CloseLyrics> for LyricsPane {}

pub(super) struct LyricsPane {
    backend: BackendHandle,
    player: Entity<player::Player>,
    view: LyricsView,
    state: State,
    /// The song the state answers for.
    key: Option<String>,
    cache: VecDeque<(String, Arc<Lyrics>)>,
    request: Option<gpui_kit::Task<()>>,
    /// Wakes the view when the next line starts. Replaced on every draw.
    tick: Option<gpui_kit::Task<()>>,
    scroll: gpui_kit::ScrollHandle,
    motion: Motion,
    _player_events: Subscription,
}

/// Where the glide from one line to the next has got to.
#[derive(Default)]
struct Motion {
    /// The line (by style anchor, -1 before the first) the view is laid out for.
    target: Option<isize>,
    /// The line the sizes are easing away from.
    from: isize,
    /// Where the new line was on screen when the glide began.
    from_y: Pixels,
    started: Option<std::time::Instant>,
    /// Frames still to re-place the view after a jump, so it follows the layout
    /// of the lines it has just been given rather than the previous song's.
    settling: u8,
}

/// Type sizes for one way of showing the lines.
#[derive(Clone, Copy)]
struct Scale {
    current: Rems,
    near: Rems,
    padding: Rems,
}

/// How one line is drawn.
#[derive(Clone, Copy)]
struct Look {
    scale: Scale,
    palette: CadencePalette,
    /// Whether a line can be clicked to seek.
    synced: bool,
}

impl Scale {
    const PANEL: Self = Self {
        current: Rems(1.9),
        near: Rems(1.25),
        padding: Rems(0.35),
    };
    const PAGE: Self = Self {
        current: Rems(2.75),
        near: Rems(2.),
        padding: Rems(0.5),
    };
}

impl LyricsPane {
    pub(super) fn new(backend: BackendHandle, cx: &mut Context<Self>) -> Self {
        let player = services::AppServices::player(cx);
        Self {
            backend,
            _player_events: cx.observe(&player, |_, _, cx| cx.notify()),
            player,
            view: LyricsView::default(),
            state: State::Idle,
            key: None,
            cache: VecDeque::new(),
            request: None,
            tick: None,
            scroll: gpui_kit::ScrollHandle::new(),
            motion: Motion::default(),
        }
    }

    /// Forgets where the lines were, so the next draw places them afresh.
    fn reset_motion(&mut self) {
        self.motion = Motion::default();
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    #[cfg(test)]
    pub(super) fn scroll_offset_y(&self) -> f32 {
        f32::from(self.scroll.offset().y)
    }

    #[cfg(test)]
    pub(super) fn gliding(&self) -> bool {
        self.motion.started.is_some() || self.motion.settling > 0
    }

    /// The pane was just opened: place the lines without a glide.
    pub(super) fn reopened(&mut self, cx: &mut Context<Self>) {
        self.reset_motion();
        cx.notify();
    }

    pub(super) fn set_view(&mut self, view: LyricsView, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.reset_motion();
            cx.notify();
        }
    }

    /// Asks for the playing song's lyrics when it is not the one already
    /// shown or on its way.
    fn sync(&mut self, track: Option<&model::Track>, cx: &mut Context<Self>) {
        let key = track.map(|track| {
            track
                .spotify_uri
                .clone()
                .unwrap_or_else(|| track.source_id.clone())
        });
        if self.key == key {
            return;
        }
        self.key = key.clone();
        self.request = None;
        self.reset_motion();
        let (Some(track), Some(key)) = (track, key) else {
            self.state = State::Idle;
            return;
        };
        if let Some((_, hit)) = self.cache.iter().find(|(known, _)| *known == key) {
            self.state = if hit.is_empty() {
                State::Missing
            } else {
                State::Ready(hit.clone())
            };
            return;
        }
        self.state = State::Loading;
        let query = Query {
            title: track.title.to_string(),
            artist: track.artists.first().map_or_else(
                || track.artist.to_string(),
                |artist| artist.name.to_string(),
            ),
            album: track.album.to_string(),
            duration_ms: track.duration_ms,
        };
        // Dropping this task on the next song drops the reply, which ends the
        // request.
        let reply = catalog::request(&self.backend, |respond| BackendCommand::LoadLyrics {
            query,
            respond,
        });
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |this, cx| {
                if this.key.as_deref() != Some(key.as_str()) {
                    return;
                }
                match result {
                    Ok(lyrics) => {
                        let lyrics = Arc::new(lyrics);
                        this.cache.push_back((key, lyrics.clone()));
                        while this.cache.len() > CACHE_ENTRIES {
                            this.cache.pop_front();
                        }
                        this.state = if lyrics.is_empty() {
                            State::Missing
                        } else {
                            State::Ready(lyrics)
                        };
                    }
                    Err(error) => {
                        log::debug!("lyrics: {error}");
                        this.state = State::Failed;
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Sets one timer for when the next line begins, while the song plays.
    fn arm_tick(
        &mut self,
        lyrics: &Lyrics,
        current: Option<usize>,
        position_ms: u32,
        playing: bool,
        cx: &mut Context<Self>,
    ) {
        self.tick = playing
            .then(|| lyrics.next_start_ms(current))
            .flatten()
            .map(|start| {
                let delay = start.saturating_sub(position_ms).max(MIN_DELAY_MS);
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(u64::from(delay)))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
            });
    }

    fn seek(&mut self, position_ms: u32, cx: &mut Context<Self>) {
        self.player
            .update(cx, |player, cx| player.seek(position_ms, cx));
    }

    fn note(palette: CadencePalette, text: &'static str) -> Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(tokens::BODY_TEXT)
            .text_color(rgb(palette.text_muted))
            .child(text)
    }

    /// One line. Blank lines (instrumental breaks) show as a note.
    fn line(
        &self,
        index: usize,
        line: &Line,
        size: Rems,
        strength: f32,
        look: Look,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Look {
            scale,
            palette,
            synced,
        } = look;
        let text = if line.text.is_empty() {
            "♪".to_owned()
        } else {
            line.text.clone()
        };
        let start_ms = line.start_ms;
        div()
            .id(("lyric-line", index))
            .test_support()
            .flex_none()
            .w_full()
            .py(scale.padding)
            .font_weight(gpui_kit::FontWeight::BOLD)
            .line_height(relative(1.2))
            .text_size(size)
            .text_color(rgb(palette.text_primary))
            .opacity(strength)
            .when(synced, |row| {
                row.cursor_pointer()
                    .hover(|style| style.opacity((strength + 0.3).min(1.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.seek(start_ms, cx)))
            })
            .child(text)
            .into_any_element()
    }

    fn body(
        &mut self,
        lyrics: &Arc<Lyrics>,
        current: Option<usize>,
        scale: Scale,
        palette: CadencePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !lyrics.synced {
            // Without timestamps there is nothing to follow; just let it scroll.
            let look = Look {
                scale,
                palette,
                synced: false,
            };
            return div()
                .id("lyrics-scroll")
                .size_full()
                .overflow_y_scroll()
                .children(
                    lyrics
                        .lines
                        .iter()
                        .take(MAX_LINES)
                        .enumerate()
                        .map(|(index, line)| self.line(index, line, scale.near, 0.85, look, cx)),
                )
                .into_any_element();
        }
        self.synced_body(lyrics, current, scale, palette, window, cx)
    }

    /// The lines in one scrolling column, with the current one a third of the
    /// way down. Moving to the next line eases the column up while the old line
    /// shrinks and the new one grows; both happen in the same frames.
    fn synced_body(
        &mut self,
        lyrics: &Arc<Lyrics>,
        current: Option<usize>,
        scale: Scale,
        palette: CadencePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = cx.background_executor().now();
        let reduce_motion = cx.reduce_motion();
        let count = lyrics.lines.len().min(MAX_LINES);
        // Before the first line the whole song is still ahead.
        let style_anchor = current.map_or(-1, |index| index as isize);
        let layout_anchor = (style_anchor.max(0) as usize).min(count - 1);

        // Where the line to centre was last laid out, from the top of the view.
        // The lines are children 1..; child 0 is the space above them.
        let area = self.scroll.bounds();
        let anchor_y = area.size.height * CURRENT_AT;
        let line_y = self
            .scroll
            .bounds_for_item(layout_anchor + 1)
            .map(|bounds| bounds.top() - area.top());
        let place = |scroll: &gpui_kit::ScrollHandle, line_y: Pixels, y: Pixels| {
            scroll.set_offset(point(px(0.), y - line_y));
        };
        let mut progress = 1.;
        match line_y {
            None => window.request_animation_frame(),
            Some(line_y) => {
                if self.motion.target != Some(style_anchor) {
                    match self.motion.target {
                        Some(previous) if !reduce_motion && self.motion.settling == 0 => {
                            self.motion.from = previous;
                            self.motion.from_y = line_y + self.scroll.offset().y;
                            self.motion.started = Some(now);
                        }
                        _ => {
                            self.motion.started = None;
                            self.motion.settling = 2;
                        }
                    }
                    self.motion.target = Some(style_anchor);
                }
                if let Some(started) = self.motion.started {
                    let elapsed = now.saturating_duration_since(started);
                    let t = elapsed.as_secs_f32() / TRANSITION.as_secs_f32();
                    if t >= 1. {
                        self.motion.started = None;
                        place(&self.scroll, line_y, anchor_y);
                    } else {
                        progress = ease_out_quint()(t);
                        let y = self.motion.from_y + (anchor_y - self.motion.from_y) * progress;
                        place(&self.scroll, line_y, y);
                        window.request_animation_frame();
                    }
                } else {
                    place(&self.scroll, line_y, anchor_y);
                    if self.motion.settling > 0 {
                        self.motion.settling -= 1;
                        window.request_animation_frame();
                    }
                }
            }
        }
        let shown = self.motion.target.is_some() && self.motion.settling == 0;

        // Bigger and stronger the nearer a line is to the current one.
        let look_at = |distance: isize| match distance.unsigned_abs() {
            0 => (scale.current.0, 1.),
            1 => (scale.near.0, 0.55),
            2 => (scale.near.0, 0.38),
            _ => (scale.near.0, 0.26),
        };
        let look = Look {
            scale,
            palette,
            synced: true,
        };
        let from = self.motion.from;
        let mut children = Vec::with_capacity(count + 2);
        children.push(div().h(relative(CURRENT_AT)).flex_none().into_any_element());
        for index in 0..count {
            let (mut size, mut strength) = look_at(index as isize - style_anchor);
            if progress < 1. {
                let (old_size, old_strength) = look_at(index as isize - from);
                size = old_size + (size - old_size) * progress;
                strength = old_strength + (strength - old_strength) * progress;
            }
            children.push(self.line(index, &lyrics.lines[index], Rems(size), strength, look, cx));
        }
        // Room below, so the last lines can still rise to the centre.
        children.push(div().h(relative(0.7)).flex_none().into_any_element());
        div()
            .id("lyrics-scroll")
            .size_full()
            .flex()
            .flex_col()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .opacity(if shown { 1. } else { 0. })
            .children(children)
            .into_any_element()
    }

    fn heading(track: Option<&model::Track>) -> SharedString {
        track.map_or_else(SharedString::default, |track| {
            format!("{} · {}", track.title, track.artist).into()
        })
    }
}

impl Render for LyricsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let (track, position_ms, playing) = {
            let player = self.player.read(cx);
            (
                player.shared_now_playing(),
                player.position_ms(cx.background_executor().now()),
                player.playing(),
            )
        };
        self.sync(track.as_deref(), cx);
        let (lyrics, current) = match &self.state {
            State::Ready(lyrics) => (Some(lyrics.clone()), lyrics.current(position_ms)),
            _ => (None, None),
        };
        match &lyrics {
            Some(lyrics) => self.arm_tick(lyrics, current, position_ms, playing, cx),
            None => self.tick = None,
        }
        let status = |state: &State| match state {
            State::Idle => "Nothing is playing",
            State::Loading => "Finding lyrics…",
            State::Missing => "No lyrics for this song",
            State::Failed => "Couldn't load lyrics",
            State::Ready(_) => "",
        };
        let heading = Self::heading(track.as_deref());

        if self.view == LyricsView::Strip {
            let line = lyrics
                .as_ref()
                .filter(|lyrics| lyrics.synced)
                .and_then(|lyrics| current.and_then(|index| lyrics.lines.get(index)))
                .map(|line| {
                    if line.text.is_empty() {
                        "♪".to_owned()
                    } else {
                        line.text.clone()
                    }
                });
            let text: SharedString = match (&lyrics, line) {
                (_, Some(line)) => line.into(),
                (Some(lyrics), None) if lyrics.synced => SharedString::from("♪"),
                (Some(_), None) => {
                    SharedString::from("Lyrics without timing can't be followed here")
                }
                (None, _) => status(&self.state).into(),
            };
            let live = current.is_some();
            return div()
                .id("lyrics-strip")
                .test_support()
                .w_full()
                .h_12()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .px_6()
                .bg(rgb(palette.surface))
                .border_t_1()
                .border_color(rgb(palette.border))
                .child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(Rems(1.25))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(rgb(if live {
                            palette.text_primary
                        } else {
                            palette.text_muted
                        }))
                        .child(text),
                )
                .into_any_element();
        }

        let page = self.view == LyricsView::Page;
        let scale = if page { Scale::PAGE } else { Scale::PANEL };
        let body = match &lyrics {
            Some(lyrics) => self.body(lyrics, current, scale, palette, window, cx),
            None => Self::note(palette, status(&self.state)).into_any_element(),
        };
        let close = components::icon_button(palette, "close-lyrics", CadenceIcon::Close)
            .test_support()
            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseLyrics)));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(if page {
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(tokens::BODY_TEXT)
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(rgb(palette.text_muted))
                    .child(heading.clone())
            } else {
                div()
                    .text_size(tokens::DRAWER_TITLE_TEXT)
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(rgb(palette.text_primary))
                    .child("Lyrics")
            })
            .child(close);
        let container = |id: &'static str| {
            div()
                .id(id)
                .test_support()
                .occlude()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .bg(rgb(palette.surface))
                .flex()
                .flex_col()
        };
        if page {
            container("lyrics-page")
                .left_0()
                .px_12()
                .pt_6()
                .pb_8()
                .child(header)
                .child(div().flex_1().min_h_0().mt_4().child(body))
                .into_any_element()
        } else {
            container("lyrics-panel")
                .w(tokens::QUEUE_DRAWER_WIDTH)
                .p_6()
                .border_l_1()
                .border_color(rgb(palette.border))
                .shadow_xl()
                .child(header)
                .child(
                    div()
                        .mt_2()
                        .truncate()
                        .text_size(tokens::BODY_TEXT)
                        .text_color(rgb(palette.text_muted))
                        .child(heading),
                )
                .child(div().flex_1().min_h_0().mt_4().child(body))
                .into_any_element()
        }
    }
}
