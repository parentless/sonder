//! Album review mode: rate an album track by track while listening.
//!
//! The next track stays locked until the current one is rated and locked in.
//! The view exists only while a review is open, so it costs nothing at rest:
//! one small `Vec` of ratings, one comment input, and rows built only on
//! render. Progress is saved as one tiny JSON preference per album, written
//! only when a rating is locked in.

use super::track_row::{COLUMN_GUTTER, INDEX_COLUMN_WIDTH, TIME_COLUMN_WIDTH};
use super::*;
use crate::storage::AlbumReview;
use gpui_kit::{ClickEvent, TestSupportExt as _};

/// Ratings are whole stars, one to five.
pub(super) const MAX_STARS: u8 = 5;
const STAR_COLOR: u32 = 0xF5B942;
/// Height the open row grows by, which holds the stars and the comment line.
const PANEL_HEIGHT: f32 = 5.5;
const PANEL_OPEN: Duration = Duration::from_millis(220);
const PANEL_CLOSE: Duration = Duration::from_millis(180);
const COMMENT_LIMIT: usize = 280;

/// The mean star rating, doubled to a score out of ten.
fn mean_score(ratings: &[u8]) -> f32 {
    let total: u32 = ratings.iter().map(|rating| u32::from(*rating)).sum();
    total as f32 / ratings.len().max(1) as f32 * 2.
}

/// What the review needs to know about the album it is for.
pub(super) struct ReviewAlbum {
    pub id: String,
    pub title: SharedString,
    pub artist: SharedString,
    pub year: Option<String>,
    pub artwork_url: Option<String>,
}

/// What the share-image buttons are doing. Only one image is made at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareAction {
    Copy,
    Save,
}

#[derive(Clone, PartialEq, Eq)]
enum ShareStatus {
    Idle,
    Working(ShareAction),
    Done(ShareAction),
    Failed,
}

/// How long "Copied" and "Saved" stay on a button before it goes back.
const SHARE_FEEDBACK: Duration = Duration::from_millis(2500);

pub(super) enum ReviewEvent {
    /// Play the album from this track.
    Play(usize),
}

pub(super) struct ReviewList {
    album_id: String,
    title: SharedString,
    artist: SharedString,
    year: Option<String>,
    artwork_url: Option<String>,
    tracks: Arc<[model::Track]>,
    review: AlbumReview,
    share: ShareStatus,
    share_task: Option<gpui_kit::Task<()>>,
    /// First track that is not locked in yet. Equal to the track count once
    /// every track has a rating.
    cursor: usize,
    /// The rating chosen for the open track, not yet locked in.
    draft: u8,
    hover_star: Option<u8>,
    /// The row whose panel is closing, and the one whose progress segment just
    /// filled. Both only drive a short animation.
    leaving: Option<usize>,
    just_locked: Option<usize>,
    comment: Entity<InputState>,
    focus: FocusHandle,
    scroll: gpui_kit::ScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<ReviewEvent> for ReviewList {}

impl Focusable for ReviewList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ReviewList {
    pub(super) fn new(
        album: ReviewAlbum,
        tracks: Arc<[model::Track]>,
        saved: Option<AlbumReview>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ReviewAlbum {
            id: album_id,
            title,
            artist,
            year,
            artwork_url,
        } = album;
        let count = tracks.len();
        // A saved review only applies to the same track list.
        let review = saved
            .filter(|saved| saved.ratings.len() == count && saved.comments.len() == count)
            .unwrap_or_else(|| AlbumReview {
                ratings: vec![0; count],
                comments: vec![String::new(); count],
            });
        let cursor = review
            .ratings
            .iter()
            .position(|rating| *rating == 0)
            .unwrap_or(count);
        let comment =
            cx.new(|cx| InputState::new(window, cx).placeholder("Add a comment (optional)"));
        let subscription = cx.subscribe_in(
            &comment,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.lock_in(window, cx);
                }
            },
        );
        Self {
            album_id,
            title,
            artist,
            year,
            artwork_url,
            tracks,
            review,
            share: ShareStatus::Idle,
            share_task: None,
            cursor,
            draft: 0,
            hover_star: None,
            leaving: None,
            just_locked: None,
            comment,
            focus: cx.focus_handle(),
            scroll: gpui_kit::ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub(super) fn complete(&self) -> bool {
        self.cursor >= self.tracks.len()
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
        self.scroll
            .scroll_to_item(self.cursor.min(self.tracks.len().saturating_sub(1)));
    }

    /// The album's score out of ten: the mean star rating, doubled.
    fn score(&self) -> f32 {
        mean_score(&self.review.ratings)
    }

    fn extremes(&self) -> Option<(usize, usize)> {
        let ratings = &self.review.ratings;
        let best = (0..ratings.len()).max_by_key(|i| (ratings[*i], std::cmp::Reverse(*i)))?;
        let worst = (0..ratings.len()).min_by_key(|i| (ratings[*i], *i))?;
        Some((best, worst))
    }

    fn set_draft(&mut self, stars: u8, cx: &mut Context<Self>) {
        if self.complete() || !(1..=MAX_STARS).contains(&stars) || self.draft == stars {
            return;
        }
        self.draft = stars;
        cx.notify();
    }

    fn lock_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.complete() || self.draft == 0 {
            return;
        }
        let index = self.cursor;
        let text: String = self
            .comment
            .read(cx)
            .value()
            .trim()
            .chars()
            .take(COMMENT_LIMIT)
            .collect();
        self.review.ratings[index] = self.draft;
        self.review.comments[index] = text;
        services::AppServices::save_album_review(&self.album_id, &self.review, cx);
        self.leaving = Some(index);
        self.just_locked = Some(index);
        self.cursor += 1;
        self.draft = 0;
        self.hover_star = None;
        self.comment
            .update(cx, |input, cx| input.set_value("", window, cx));
        window.focus(&self.focus, cx);
        // Keyboard-initiated, so the scroll is immediate rather than animated.
        self.scroll
            .scroll_to_item(self.cursor.min(self.tracks.len().saturating_sub(1)));
        cx.notify();
    }

    fn handle_key(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing into the comment box must never change the rating.
        if !self.focus.is_focused(window) {
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.lock_in(window, cx),
            key => {
                if let Some(stars) = key.parse::<u8>().ok().filter(|_| key.len() == 1) {
                    self.set_draft(stars, cx);
                }
            }
        }
    }

    /// The finished review as Markdown, for pasting into chat.
    pub(super) fn summary_text(&self) -> String {
        let mut text = format!(
            "**{}** · {}\n**{:.1} / 10**\n",
            self.title,
            self.artist,
            self.score()
        );
        for (index, track) in self.tracks.iter().enumerate() {
            let stars = usize::from(self.review.ratings[index]);
            text.push_str(&format!(
                "\n{}. {} {}{}",
                index + 1,
                track.title,
                "★".repeat(stars),
                "☆".repeat(usize::from(MAX_STARS).saturating_sub(stars)),
            ));
            let comment = &self.review.comments[index];
            if !comment.is_empty() {
                text.push_str(&format!(" — {comment}"));
            }
        }
        text.push_str("\n\nReviewed in Sonder");
        text
    }

    /// Makes the share image off the UI thread, then copies it to the
    /// clipboard or saves it to Downloads. Nothing is kept: the pixels are
    /// dropped when this returns, and the image is made again next time.
    fn make_image(&mut self, action: ShareAction, cx: &mut Context<Self>) {
        if !self.complete() || matches!(self.share, ShareStatus::Working(_)) {
            return;
        }
        self.share = ShareStatus::Working(action);
        let card = review_image::ShareCard {
            title: self.title.to_string(),
            artist: self.artist.to_string(),
            year: self.year.clone(),
            ratings: self.review.ratings.clone(),
            titles: self.tracks.iter().map(|t| t.title.to_string()).collect(),
            comments: self.review.comments.clone(),
            cover: None,
        };
        let url = self.artwork_url.clone();
        let client = cx.http_client();
        let file_name = share_file_name(&self.title);
        self.share_task = Some(cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let mut card = card;
                    if let Some(url) = url {
                        card.cover = fetch_cover(client, &url).await;
                    }
                    let png = review_image::render(&card)?;
                    let saved = match action {
                        ShareAction::Copy => None,
                        ShareAction::Save => Some(save_png(&file_name, &png)?),
                    };
                    anyhow::Ok((png, saved))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.share = match made {
                    Ok((png, _)) => {
                        if action == ShareAction::Copy {
                            let image =
                                gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png);
                            cx.write_to_clipboard(ClipboardItem::new_image(&image));
                        }
                        ShareStatus::Done(action)
                    }
                    Err(error) => {
                        log::warn!("could not make the share image: {error:#}");
                        ShareStatus::Failed
                    }
                };
                cx.notify();
                // Put the button back after a moment.
                this.share_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SHARE_FEEDBACK).await;
                    let _ = this.update(cx, |this, cx| {
                        this.share = ShareStatus::Idle;
                        this.share_task = None;
                        cx.notify();
                    });
                }));
            });
        }));
        cx.notify();
    }

    fn progress(&self, palette: CadencePalette, reduce_motion: bool) -> impl IntoElement {
        let mut bar = div()
            .id("review-progress")
            .test_support()
            .w_full()
            .px_3()
            .pb_3()
            .flex()
            .gap(px(3.));
        for index in 0..self.tracks.len() {
            let mut segment = div()
                .id(("review-segment", index))
                .flex_1()
                .h(px(4.))
                .rounded_full()
                .overflow_hidden()
                .bg(rgb(palette.control));
            if index < self.cursor {
                let fill = div().h_full().rounded_full().bg(rgb(palette.link));
                segment = segment.child(if self.just_locked == Some(index) && !reduce_motion {
                    fill.with_animation(
                        ("review-fill", index),
                        Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                        |fill, delta| fill.w(relative(delta)),
                    )
                    .into_any_element()
                } else {
                    fill.w_full().into_any_element()
                });
            }
            bar = bar.child(segment);
        }
        bar
    }

    fn header(&self, palette: CadencePalette) -> impl IntoElement {
        div()
            .id("review-header")
            .h_10()
            .flex_none()
            .pl_3()
            .pr(px(16. + 12.))
            .flex()
            .items_center()
            .text_size(tokens::BODY_TEXT)
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(rgb(palette.text_muted))
            .child(
                div()
                    .w(INDEX_COLUMN_WIDTH)
                    .flex_none()
                    .text_center()
                    .child("#"),
            )
            .child(div().flex_1().min_w_0().pr(COLUMN_GUTTER).child("Title"))
            .child(div().flex_1().min_w_0().child("Rating"))
            .child(
                div()
                    .w(TIME_COLUMN_WIDTH)
                    .flex_none()
                    .flex()
                    .justify_end()
                    .pr_2()
                    .child("Time"),
            )
    }

    fn stars_row(&self, palette: CadencePalette, count: u8, size: Rems) -> Div {
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .children((1..=MAX_STARS).map(move |n| {
                components::icon(
                    if n <= count {
                        CadenceIcon::StarFilled
                    } else {
                        CadenceIcon::Star
                    },
                    size,
                    if n <= count {
                        STAR_COLOR
                    } else {
                        palette.text_muted
                    },
                )
            }))
    }

    fn panel(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shown = self.hover_star.unwrap_or(self.draft);
        let ready = self.draft > 0;
        let stars = (1..=MAX_STARS).map(|n| {
            components::button(palette, ("review-star", usize::from(n)))
                .test_support()
                .aria_label(format!("{n} stars"))
                .size_8()
                .rounded_md()
                .child(components::icon(
                    if n <= shown {
                        CadenceIcon::StarFilled
                    } else {
                        CadenceIcon::Star
                    },
                    Rems(1.5),
                    if n <= shown {
                        STAR_COLOR
                    } else {
                        palette.text_muted
                    },
                ))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let next = hovered.then_some(n);
                    if this.hover_star != next {
                        this.hover_star = next;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.set_draft(n, cx)))
        });
        div()
            .id("review-panel")
            .test_support()
            .w_full()
            .pl(INDEX_COLUMN_WIDTH + gpui_kit::Rems(0.75))
            .pr_3()
            .pb_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex().items_center().children(stars))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child("Press 1–5, then Enter to lock in"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        components::text_field_frame(palette, &self.comment, window, cx)
                            .flex_1()
                            .min_w_0()
                            .h_9()
                            .child(
                                Input::new(&self.comment)
                                    .id("review-comment")
                                    .appearance(false)
                                    .text_color(rgb(palette.text_primary))
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .flex_1()
                                    .min_w_0()
                                    .h_full(),
                            ),
                    )
                    .child({
                        let lock = components::pill(palette, "review-lock-in", "Lock in", true)
                            .h_9()
                            .test_support();
                        if ready {
                            lock.on_click(
                                cx.listener(|this, _, window, cx| this.lock_in(window, cx)),
                            )
                        } else {
                            lock.opacity(0.4)
                        }
                    }),
            )
    }
}

impl Render for ReviewList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let reduce_motion = cx.reduce_motion();
        let mut rows = Vec::with_capacity(self.tracks.len());
        for index in 0..self.tracks.len() {
            let track = &self.tracks[index];
            let locked_in = index < self.cursor;
            let open = index == self.cursor;
            let future = index > self.cursor;
            let line = div()
                .id(("review-row", index))
                .test_support()
                .w_full()
                .h(Rems(3.25))
                .px_3()
                .flex()
                .items_center()
                .when(future, |row| row.opacity(0.45))
                .child(
                    div()
                        .w(INDEX_COLUMN_WIDTH)
                        .flex_none()
                        .flex()
                        .justify_center()
                        .text_size(tokens::BODY_TEXT)
                        .text_color(rgb(palette.text_muted))
                        .child(if future {
                            components::icon(CadenceIcon::Lock, Rems(0.875), palette.text_muted)
                                .into_any_element()
                        } else {
                            (index + 1).to_string().into_any_element()
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pr(COLUMN_GUTTER)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_ellipsis()
                                .line_clamp(1)
                                .text_size(tokens::BODY_TEXT)
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(track.title.clone()),
                        )
                        .child(
                            div()
                                .text_ellipsis()
                                .line_clamp(1)
                                .text_xs()
                                .text_color(rgb(palette.text_muted))
                                .child(track.artist.clone()),
                        ),
                )
                .child(
                    div()
                        .id(("review-rating", index))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .when(locked_in, |cell| {
                            cell.child(self.stars_row(
                                palette,
                                self.review.ratings[index],
                                Rems(0.875),
                            ))
                        }),
                )
                .child(
                    div()
                        .w(TIME_COLUMN_WIDTH)
                        .flex_none()
                        .flex()
                        .justify_end()
                        .pr_2()
                        .text_size(tokens::BODY_TEXT)
                        .text_color(rgb(palette.text_muted))
                        .child(format_duration(track.duration_ms)),
                )
                .when(!future, |row| {
                    row.on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                        if event.click_count() == 2 {
                            cx.emit(ReviewEvent::Play(index));
                        }
                    }))
                });
            let mut item = div()
                .id(("review-item", index))
                .w_full()
                .rounded_lg()
                .overflow_hidden()
                .when(open, |item| item.bg(rgb(palette.selection)))
                .child(line);
            if open {
                let panel = self.panel(palette, window, cx);
                item = item.child(if reduce_motion {
                    panel.into_any_element()
                } else {
                    div()
                        .overflow_hidden()
                        .child(panel)
                        .with_animation(
                            ("review-open", index),
                            Animation::new(PANEL_OPEN).with_easing(ease_out_quint()),
                            |panel, delta| panel.h(Rems(PANEL_HEIGHT * delta)).opacity(delta),
                        )
                        .into_any_element()
                });
            } else if self.leaving == Some(index) && !reduce_motion {
                // The panel's content is gone; only its tint shrinks away.
                item = item.child(div().w_full().bg(rgb(palette.selection)).with_animation(
                    ("review-close", index),
                    Animation::new(PANEL_CLOSE).with_easing(ease_out_quint()),
                    |strip, delta| {
                        strip
                            .h(Rems(PANEL_HEIGHT * (1. - delta)))
                            .opacity(1. - delta)
                    },
                ));
            }
            // Permanent spacing, as in the album's track list.
            rows.push(
                div()
                    .w_full()
                    .flex_none()
                    .py(Rems(0.125))
                    .child(item)
                    .into_any_element(),
            );
        }
        let finish = self
            .complete()
            .then(|| self.finish(palette, reduce_motion, cx));
        div()
            .id("review-list")
            .test_support()
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    this.handle_key(event, window, cx)
                }),
            )
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.progress(palette, reduce_motion))
            .child(self.header(palette))
            .child(
                div()
                    .id("review-scroll")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .pr(px(16.))
                    .children(rows)
                    .children(finish),
            )
    }
}

impl ReviewList {
    fn share_button(
        &self,
        palette: CadencePalette,
        action: ShareAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, idle, done) = match action {
            ShareAction::Copy => ("review-copy-image", "Copy image", "Copied ✓"),
            ShareAction::Save => ("review-save-image", "Save image", "Saved to Downloads ✓"),
        };
        let label = match &self.share {
            ShareStatus::Working(working) if *working == action => "Making image…",
            ShareStatus::Done(finished) if *finished == action => done,
            ShareStatus::Failed => "Couldn't make the image",
            _ => idle,
        };
        let busy = matches!(self.share, ShareStatus::Working(_));
        let button =
            components::pill(palette, id, label, action == ShareAction::Copy).test_support();
        if busy {
            button.opacity(0.6)
        } else {
            button.on_click(cx.listener(move |this, _, _, cx| this.make_image(action, cx)))
        }
    }

    fn finish(
        &self,
        palette: CadencePalette,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = |index: usize| self.tracks[index].title.clone();
        let (best, worst) = self.extremes().unwrap_or((0, 0));
        let card = div()
            .id("review-finish")
            .test_support()
            .w_full()
            .my_2()
            .px_5()
            .py_4()
            .rounded_xl()
            .bg(rgb(palette.selection))
            .flex()
            .items_center()
            .gap_5()
            .child(
                div()
                    .child(
                        div()
                            .text_size(Rems(2.25))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(format!("{:.1}", self.score())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child("out of 10"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(tokens::BODY_TEXT)
                    .text_color(rgb(palette.text_muted))
                    .child(format!("Best: {}", name(best)))
                    .child(div().child(format!("Worst: {}", name(worst)))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .child(self.share_button(palette, ShareAction::Copy, cx))
                    .child(self.share_button(palette, ShareAction::Save, cx))
                    .child(
                        components::pill(palette, "review-copy-text", "Copy text", false)
                            .test_support()
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.summary_text(),
                                ));
                            })),
                    ),
            );
        if reduce_motion {
            return card.into_any_element();
        }
        card.with_animation(
            "review-finish-in",
            Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
            |card, delta| card.opacity(delta).mt(Rems(0.5 + 0.75 * (1. - delta))),
        )
        .into_any_element()
    }
}

/// Downloads the cover (usually already on disk from the album page) and
/// decodes it. A missing or broken cover just means a plain background.
async fn fetch_cover(
    client: Arc<dyn gpui_kit::http_client::HttpClient>,
    url: &str,
) -> Option<image::RgbaImage> {
    use futures::AsyncReadExt as _;
    let mut response = client.get(url, ().into(), true).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .take(super::http::MAX_IMAGE_DOWNLOAD_BYTES as u64)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    image::load_from_memory(&bytes)
        .ok()
        .map(|image| image.to_rgba8())
}

/// `Sonder review - <album>.png`, with anything a file name cannot hold removed.
fn share_file_name(title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()
        })
        .take(80)
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    if clean.is_empty() {
        "Sonder review.png".to_owned()
    } else {
        format!("Sonder review - {clean}.png")
    }
}

/// Writes the image into the Downloads folder, or the temp folder without one.
fn save_png(file_name: &str, png: &[u8]) -> anyhow::Result<std::path::PathBuf> {
    let folder = directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(file_name);
    std::fs::write(&path, png)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{mean_score, share_file_name};

    #[test]
    fn share_file_names_drop_what_files_cannot_hold() {
        assert_eq!(
            share_file_name("Blue hours"),
            "Sonder review - Blue hours.png"
        );
        assert_eq!(
            share_file_name("AC/DC: Back in Black?"),
            "Sonder review - ACDC Back in Black.png"
        );
        assert_eq!(share_file_name("???"), "Sonder review.png");
    }

    #[test]
    fn score_is_the_mean_star_rating_out_of_ten() {
        assert_eq!(mean_score(&[5, 5]), 10.);
        assert_eq!(mean_score(&[4, 3, 5]), 8.);
        assert_eq!(mean_score(&[]), 0.);
    }
}
