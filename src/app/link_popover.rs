//! The Local Files popover for saving songs from links.
use super::*;
use crate::link_download::{self, Format};
use gpui_kit::TestSupportExt as _;
use link_downloads::{LinkDownloads, Phase, ProbeState};
use std::{collections::HashSet, path::PathBuf};

/// How long typing pauses before the links in the field are looked up, so a
/// half-typed address is not.
const LOOKUP_DELAY: Duration = Duration::from_millis(450);
const POPOVER_WIDTH: Rems = Rems(25.);

pub(super) enum LinkPopoverEvent {
    /// There is no folder yet; take them to where one is added.
    OpenFolders,
}

impl EventEmitter<LinkPopoverEvent> for LinkPopover {}

pub(super) struct LinkPopover {
    downloads: Entity<LinkDownloads>,
    library: Entity<library::Library>,
    image_cache: Entity<image_cache::BoundedImageCache>,
    input: Entity<InputState>,
    /// The links in the field, once the pause after typing has passed.
    links: Vec<String>,
    /// Playlist links the listener chose to save only the first song of.
    first_only: HashSet<String>,
    /// The clipboard link that filled the field, so the popover can say so.
    from_clipboard: bool,
    pending_lookup: Option<gpui_kit::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LinkPopover {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Paste a link to a song"));
        let downloads = services::AppServices::link_downloads(cx);
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.text_changed(cx),
                        InputEvent::PressEnter { .. } => this.save(window, cx),
                        _ => {}
                    }
                }),
                cx.observe(&downloads, |_, _, cx| cx.notify()),
            ];
        let mut popover = Self {
            downloads,
            library: services::AppServices::library(cx),
            image_cache: services::AppServices::image_cache(cx),
            input,
            links: Vec::new(),
            first_only: HashSet::new(),
            from_clipboard: false,
            pending_lookup: None,
            _subscriptions: subscriptions,
        };
        popover.fill_from_clipboard(window, cx);
        popover
    }

    /// Opens ready to type or paste into.
    pub(super) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// A link already on the clipboard is the likeliest thing to paste, so it
    /// fills an empty field; the popover says where it came from.
    fn fill_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Tests and screenshots must not depend on what the machine copied.
        if cfg!(test) || !self.input.read(cx).value().is_empty() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if link_download::parse_links(&text).is_empty() {
            return;
        }
        self.set_text(&text, window, cx);
        self.read_links(cx);
        self.from_clipboard = true;
    }

    fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.from_clipboard = false;
    }

    fn text_changed(&mut self, cx: &mut Context<Self>) {
        self.from_clipboard = false;
        self.pending_lookup = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LOOKUP_DELAY).await;
            let _ = this.update(cx, |this, cx| this.read_links(cx));
        }));
        cx.notify();
    }

    /// Reads the field now and looks up whatever links it holds.
    fn read_links(&mut self, cx: &mut Context<Self>) {
        self.pending_lookup = None;
        let text = self.input.read(cx).value().to_string();
        self.links = link_download::parse_links(&text);
        self.first_only.retain(|link| self.links.contains(link));
        let links = self.links.clone();
        self.downloads
            .update(cx, |downloads, cx| downloads.look_up(&links, cx));
        cx.notify();
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.set_text(&text, window, cx);
            self.read_links(cx);
        }
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text("", window, cx);
        self.read_links(cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn folders(&self, cx: &App) -> Arc<[PathBuf]> {
        self.library.read(cx).local_folders().clone()
    }

    /// The links that can be saved right now.
    fn ready_links(&self, cx: &App) -> Vec<String> {
        let downloads = self.downloads.read(cx);
        self.links
            .iter()
            .filter(|link| matches!(downloads.probe(link), Some(ProbeState::Found(_))))
            .cloned()
            .collect()
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Enter may arrive before the pause after typing has passed.
        self.read_links(cx);
        let folders = self.folders(cx);
        let ready = self.ready_links(cx);
        let Some(folder) = self.downloads.read(cx).folder(&folders) else {
            return;
        };
        if ready.is_empty() {
            return;
        }
        let playlists: Vec<String> = {
            let downloads = self.downloads.read(cx);
            ready
                .iter()
                .filter(|link| {
                    !self.first_only.contains(*link)
                        && matches!(
                            downloads.probe(link),
                            Some(ProbeState::Found(probe)) if probe.playlist_count.is_some_and(|count| count > 1)
                        )
                })
                .cloned()
                .collect()
        };
        self.downloads.update(cx, |downloads, cx| {
            downloads.save(&ready, &playlists, folder, cx)
        });
        // The field is free for the next link while these run.
        self.set_text("", window, cx);
        self.links.clear();
        self.first_only.clear();
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Types `text` into the field and looks its links up at once, without the
    /// pause a person's typing gets.
    #[cfg(test)]
    pub(super) fn fixture_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text(text, window, cx);
        self.read_links(cx);
    }

    fn choose_format(&mut self, format: Format, cx: &mut Context<Self>) {
        self.downloads
            .update(cx, |downloads, cx| downloads.choose(format, None, cx));
    }

    fn choose_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let format = self.downloads.read(cx).format();
        self.downloads.update(cx, |downloads, cx| {
            downloads.choose(format, Some(folder), cx)
        });
    }
}

fn chip(
    palette: CadencePalette,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> impl IntoElement + StatefulInteractiveElement + Styled {
    components::filled_button(palette, id)
        .h_8()
        .px_3()
        .flex_none()
        .rounded_full()
        .text_size(tokens::BODY_TEXT)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .when(selected, |chip| {
            chip.bg(rgb(palette.text_primary))
                .text_color(rgb(palette.surface))
        })
        .when(!selected, |chip| {
            chip.bg(rgb(palette.control))
                .text_color(rgb(palette.text_primary))
                .hover(|style| style.bg(rgb(palette.control_hover)))
        })
        .child(label.into())
        .test_support()
}

fn caption(palette: CadencePalette, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(tokens::CAPTION_TEXT)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_muted))
        .child(text.into())
}

fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn clock(seconds: f64) -> String {
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// "Ana Lind · 3:41 · example.com", leaving out what is not known.
fn probe_detail(probe: &link_download::Probe) -> String {
    if let Some(count) = probe.playlist_count {
        return format!(
            "Playlist · {count} {}",
            if count == 1 { "song" } else { "songs" }
        );
    }
    [
        probe.artist.clone(),
        probe.duration_secs.map(clock),
        (!probe.site.is_empty()).then(|| probe.site.clone()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

impl LinkPopover {
    fn link_row(
        &self,
        palette: CadencePalette,
        index: usize,
        link: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.downloads.read(cx).probe(link).cloned();
        let row = div()
            .flex()
            .items_center()
            .gap_3()
            .p_2()
            .rounded_xl()
            .bg(rgb(palette.control));
        match state {
            Some(ProbeState::Found(probe)) => {
                let playlist = probe.playlist_count.filter(|count| *count > 1);
                let first_only = self.first_only.contains(link);
                let owned = link.to_owned();
                row.child(components::artwork(
                    palette,
                    &self.image_cache,
                    probe.thumbnail.as_deref(),
                    tokens::TRACK_ARTWORK,
                    CadenceIcon::MusicNote,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.text_primary))
                                .child(probe.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(tokens::CAPTION_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(probe_detail(&probe)),
                        ),
                )
                .when_some(playlist, |row, count| {
                    row.child(
                        chip(
                            palette,
                            ("link-playlist", index),
                            if first_only {
                                "First song".to_owned()
                            } else {
                                format!("All {count}")
                            },
                            !first_only,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.first_only.remove(&owned) {
                                this.first_only.insert(owned.clone());
                            }
                            cx.notify();
                        })),
                    )
                })
                .into_any_element()
            }
            Some(ProbeState::Failed(message)) => {
                let owned = link.to_owned();
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(tokens::CAPTION_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(link.to_owned()),
                        )
                        .child(div().text_color(rgb(palette.danger)).child(message)),
                )
                .when(link_download::refusal(link).is_none(), |row| {
                    row.child(
                        chip(palette, ("link-retry", index), "Retry", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let owned = owned.clone();
                                this.downloads
                                    .update(cx, |downloads, cx| downloads.retry_probe(&owned, cx));
                            },
                        )),
                    )
                })
                .into_any_element()
            }
            Some(ProbeState::Looking { setup }) => row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(tokens::CAPTION_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(link.to_owned()),
                        )
                        .child(
                            div()
                                .text_color(rgb(palette.text_muted))
                                .child(match setup {
                                    Some((what, Some(fraction))) => format!(
                                        "Getting {what} (one time) · {}%",
                                        (fraction * 100.).round() as u32
                                    ),
                                    Some((what, None)) => format!("Getting {what} (one time)"),
                                    None => "Looking…".to_owned(),
                                }),
                        ),
                )
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    fn download_row(&self, palette: CadencePalette, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some((title, phase)) = self
            .downloads
            .read(cx)
            .items()
            .iter()
            .find(|item| item.id == id)
            .map(|item| (item.title().to_owned(), item.phase.clone()))
        else {
            return div().into_any_element();
        };
        let fraction = match &phase {
            Phase::Working { fraction, .. } | Phase::Setup { fraction, .. } => *fraction,
            _ => None,
        };
        let failed = matches!(phase, Phase::Failed(_));
        let done = matches!(phase, Phase::Done(_));
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(Rems(2.75))
            .child(
                div()
                    .flex_none()
                    .size_8()
                    .rounded_lg()
                    .bg(rgb(palette.control))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(components::icon(
                        if done {
                            CadenceIcon::Check
                        } else {
                            CadenceIcon::MusicNote
                        },
                        tokens::CONTROL_ICON,
                        palette.text_primary,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(tokens::CAPTION_TEXT)
                            .text_color(rgb(if failed {
                                palette.danger
                            } else {
                                palette.text_muted
                            }))
                            .child(link_downloads::phase_label(&phase)),
                    )
                    .when_some(fraction, |col, fraction| {
                        col.child(
                            div()
                                .mt_1()
                                .h(px(3.))
                                .w_full()
                                .rounded_full()
                                .bg(rgb(palette.control))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(fraction.clamp(0.02, 1.)))
                                        .rounded_full()
                                        .bg(rgb(palette.text_primary)),
                                ),
                        )
                    }),
            )
            .when(failed || matches!(phase, Phase::Cancelled), |row| {
                row.child(
                    chip(palette, ("download-retry", id as usize), "Retry", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.downloads
                                .update(cx, |downloads, cx| downloads.retry(id, cx));
                        }),
                    ),
                )
            })
            .child(
                components::icon_button_sized(
                    palette,
                    ("download-dismiss", id as usize),
                    CadenceIcon::Close,
                    tokens::INLINE_ICON,
                )
                .size_8()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.downloads
                        .update(cx, |downloads, cx| downloads.dismiss(id, cx));
                })),
            )
            .into_any_element()
    }
}

impl Render for LinkPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let folders = self.folders(cx);
        let (format, chosen, items) = {
            let downloads = self.downloads.read(cx);
            (
                downloads.format(),
                downloads.folder(&folders),
                downloads.items().len(),
            )
        };
        let ready = self.ready_links(cx);
        let typed = !self.input.read(cx).value().is_empty();
        let any_found = !ready.is_empty();
        let can_save = any_found && chosen.is_some();

        let field = components::text_field_frame(palette, &self.input, window, cx)
            .id("link-field")
            .test_support()
            .h_12()
            .gap_2p5()
            .child(components::icon(
                CadenceIcon::Link,
                tokens::FIELD_ICON,
                palette.text_muted,
            ))
            .child(
                Input::new(&self.input)
                    .id("link-input")
                    .appearance(false)
                    .text_color(rgb(palette.text_primary))
                    .bordered(false)
                    .focus_bordered(false)
                    .flex_1()
                    .min_w_0()
                    .h_full(),
            )
            .child(if typed {
                components::icon_button_sized(
                    palette,
                    "link-clear",
                    CadenceIcon::Close,
                    tokens::INLINE_ICON,
                )
                .size_8()
                .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx)))
                .into_any_element()
            } else {
                chip(palette, "link-paste", "Paste", false)
                    .on_click(cx.listener(|this, _, window, cx| this.paste(window, cx)))
                    .into_any_element()
            });

        let links: Vec<AnyElement> = self
            .links
            .clone()
            .iter()
            .enumerate()
            .map(|(index, link)| self.link_row(palette, index, link, cx))
            .collect();

        let options = any_found.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_2p5()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(caption(palette, "FORMAT"))
                        .child(div().flex().flex_wrap().gap_1p5().children(
                            Format::ALL.into_iter().map(|candidate| {
                                chip(
                                    palette,
                                    ("link-format", candidate as usize),
                                    candidate.label(),
                                    candidate == format,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.choose_format(candidate, cx),
                                ))
                            }),
                        ))
                        .child(
                            div()
                                .text_size(tokens::CAPTION_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child(format.detail()),
                        ),
                )
                .when(folders.len() > 1, |column| {
                    column.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .child(caption(palette, "SAVE TO"))
                            .child(div().flex().flex_wrap().gap_1p5().children(
                                folders.iter().enumerate().map(|(index, folder)| {
                                    let folder = folder.clone();
                                    chip(
                                        palette,
                                        ("link-folder", index),
                                        folder_name(&folder),
                                        chosen.as_ref() == Some(&folder),
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.choose_folder(folder.clone(), cx)
                                        },
                                    ))
                                }),
                            )),
                    )
                })
        });

        let ids: Vec<u64> = self
            .downloads
            .read(cx)
            .items()
            .iter()
            .rev()
            .map(|item| item.id)
            .collect();
        let list: Vec<AnyElement> = ids
            .into_iter()
            .map(|id| self.download_row(palette, id, cx))
            .collect();

        let footer = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .text_size(tokens::CAPTION_TEXT)
                    .text_color(rgb(palette.text_muted))
                    .child(if self.from_clipboard {
                        "Pasted from your clipboard. Only save audio you have the right to use."
                    } else {
                        "Only save audio you have the right to use."
                    }),
            )
            .child(if can_save {
                components::pill(
                    palette,
                    "link-save",
                    if ready.len() > 1 {
                        format!("Download {}", ready.len())
                    } else {
                        "Download".to_owned()
                    },
                    true,
                )
                .test_support()
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))
                .into_any_element()
            } else {
                components::pending_pill(palette, "link-save-disabled", "Download")
                    .test_support()
                    .opacity(0.35)
                    .into_any_element()
            });

        let body: AnyElement = if folders.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .py_4()
                .text_center()
                .child(components::icon(
                    CadenceIcon::Folder,
                    Rems(1.5),
                    palette.text_muted,
                ))
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.text_primary))
                        .child("Add a folder first"),
                )
                .child(
                    div()
                        .text_color(rgb(palette.text_muted))
                        .child("Songs from links are saved into a Local Files folder."),
                )
                .child(
                    components::settings_button(palette, "link-add-folder", "Choose a folder")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(LinkPopoverEvent::OpenFolders)),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(field)
                .when(!links.is_empty(), |column| {
                    column.child(
                        div()
                            .id("link-rows")
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .max_h(Rems(14.))
                            .overflow_y_scroll()
                            .children(links),
                    )
                })
                .children(options)
                .child(footer)
                .when(items > 0, |column| {
                    column.child(
                        div()
                            .flex()
                            .flex_col()
                            .pt_2p5()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .child(caption(palette, "DOWNLOADS"))
                            .child(
                                div()
                                    .id("link-downloads")
                                    .flex()
                                    .flex_col()
                                    .max_h(Rems(12.))
                                    .overflow_y_scroll()
                                    .children(list),
                            ),
                    )
                })
                .into_any_element()
        };

        div()
            .id("link-popover")
            .test_support()
            .occlude()
            .w(POPOVER_WIDTH)
            .p_4()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface_raised))
            .shadow_lg()
            .text_color(rgb(palette.text))
            .text_size(tokens::BODY_TEXT)
            .child(body)
    }
}
