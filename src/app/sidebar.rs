use super::*;
use gpui_kit::TestSupportExt as _;

/// Navigation the sidebar asks the workspace to perform.
pub(super) enum SidebarEvent {
    Navigate(Route),
    Failed(SharedString),
    OpenPlaylist {
        playlist: model::Playlist,
        origin: Route,
    },
}

/// A library route the rail links to.
struct NavItem {
    id: &'static str,
    label: &'static str,
    icon: CadenceIcon,
    selected_icon: CadenceIcon,
    route: Route,
}

const NAV_ITEMS: [NavItem; 5] = [
    NavItem {
        id: "nav-home",
        label: "Home",
        icon: CadenceIcon::Home,
        selected_icon: CadenceIcon::HomeFilled,
        route: Route::Home,
    },
    NavItem {
        id: "nav-library",
        label: "Liked Songs",
        icon: CadenceIcon::Heart,
        selected_icon: CadenceIcon::HeartFilled,
        route: Route::LikedSongs,
    },
    NavItem {
        id: "nav-playlist",
        label: "Playlists",
        icon: CadenceIcon::Playlist,
        selected_icon: CadenceIcon::PlaylistFilled,
        route: Route::Playlists,
    },
    NavItem {
        id: "nav-recent",
        label: "Recently played",
        icon: CadenceIcon::Clock,
        selected_icon: CadenceIcon::ClockFilled,
        route: Route::Recent,
    },
    // Shown only once the listener has added a folder.
    NavItem {
        id: "nav-local-files",
        label: "Local Files",
        icon: CadenceIcon::Folder,
        selected_icon: CadenceIcon::FolderFilled,
        route: Route::LocalFiles,
    },
];

/// An update the rail offers, in the order a listener meets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UpdateOffer {
    Available(String),
    #[cfg(target_os = "windows")]
    Downloading(String),
    #[cfg(target_os = "windows")]
    Ready(String),
}

impl UpdateOffer {
    /// `previous` supplies the version while a download is under way, since
    /// that state does not repeat it.
    pub(super) fn from_check(
        check: &updater::UpdateCheck,
        #[cfg_attr(not(target_os = "windows"), allow(unused_variables))] previous: Option<&Self>,
    ) -> Option<Self> {
        use updater::UpdateCheck;
        match check {
            UpdateCheck::Available(version) => Some(Self::Available(version.clone())),
            #[cfg(target_os = "windows")]
            UpdateCheck::Downloading => previous.map(|offer| match offer {
                Self::Available(v) | Self::Downloading(v) | Self::Ready(v) => {
                    Self::Downloading(v.clone())
                }
            }),
            #[cfg(target_os = "windows")]
            UpdateCheck::Ready(version) => Some(Self::Ready(version.clone())),
            _ => None,
        }
    }
}

/// Where the rail stands between collapsed (0) and expanded (1). One spring
/// drives the rail's width and everything inside it, so the page edge, labels
/// and pills move together.
struct RowTransition {
    progress: f32,
    row_width: Rems,
}

/// One line of section-label text: the body size times GPUI's default line height.
const SECTION_LABEL_HEIGHT: Rems = Rems(0.8125 * 1.618_034);

/// Shrinks `block` toward nothing as `progress` falls from 1, and takes back the
/// flex gap that followed it, so the rows below slide up instead of jumping.
/// At rest (progress 1) it adds nothing.
fn collapsing(block: Div, progress: f32, natural: Rems, gap: Rems) -> Div {
    if progress >= 1. {
        return block;
    }
    block
        .h(natural * progress)
        .overflow_hidden()
        .mb(Rems(-gap.0 * (1. - progress)))
}

/// The library navigation rail.
pub(super) struct Sidebar {
    library: Entity<library::Library>,
    brand_mark: Arc<gpui_kit::Image>,
    image_cache: Entity<image_cache::BoundedImageCache>,
    tooltip_anchor: Rc<Cell<Bounds<Pixels>>>,
    playlist_hover: components::HoverHighlight,
    playlist_source: Arc<[model::Playlist]>,
    playlist_order: Arc<[usize]>,
    pinned_source: Arc<[model::Playlist]>,
    pinned_count: usize,
    playlist_scroll: gpui_kit::UniformListScrollHandle,
    active_playlist: Option<String>,
    reorder_generation: usize,
    reorder_position: usize,
    reorder_destination: usize,
    reorder_started: Option<std::time::Instant>,
    /// The route to highlight, pushed by the workspace when it navigates.
    route: Route,
    /// Where a pinned playlist should return to when the listener backs out.
    pinned_origin: Route,
    compact_layout: bool,
    hover: components::HoverHighlight,
    collapsed: bool,
    /// 0 collapsed to 1 expanded, sprung toward the chosen state.
    progress: gpui_motion_kit::ValueState,
    visual_width: Rems,
    /// The toggle and navigation rows draw keyboard focus on the fill inside
    /// them, so they own their focus handles.
    toggle_focus: FocusHandle,
    /// One per entry of [`NAV_ITEMS`], in the same order.
    nav_focus: [FocusHandle; NAV_ITEMS.len()],
    whats_new_focus: FocusHandle,
    /// This version's notes have not been opened yet.
    whats_new_unseen: bool,
    update: Option<UpdateOffer>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

fn expanded_sidebar_width(compact_layout: bool) -> Rems {
    if compact_layout {
        COMPACT_EXPANDED_SIDEBAR_WIDTH
    } else {
        EXPANDED_SIDEBAR_WIDTH
    }
}

fn item_label(index: usize) -> &'static str {
    NAV_ITEMS[index].label
}

impl Sidebar {
    pub(super) fn new(collapsed: bool, cx: &mut App) -> Self {
        let width = if collapsed {
            COLLAPSED_SIDEBAR_WIDTH
        } else {
            expanded_sidebar_width(false)
        };
        Self {
            library: services::AppServices::library(cx),
            image_cache: services::AppServices::image_cache(cx),
            tooltip_anchor: Rc::new(Cell::new(Bounds::default())),
            playlist_hover: components::HoverHighlight::default(),
            playlist_source: Arc::from([]),
            playlist_order: Arc::from([]),
            pinned_source: Arc::from([]),
            pinned_count: 0,
            playlist_scroll: gpui_kit::UniformListScrollHandle::new(),
            active_playlist: None,
            reorder_generation: 0,
            reorder_position: 0,
            reorder_destination: 0,
            reorder_started: None,
            brand_mark: services::AppServices::brand_mark(cx),
            route: Route::LikedSongs,
            pinned_origin: Route::LikedSongs,
            compact_layout: false,
            hover: components::HoverHighlight::default(),
            collapsed,
            progress: gpui_motion_kit::ValueState::new(if collapsed { 0. } else { 1. }),
            visual_width: width,
            toggle_focus: cx.focus_handle().tab_stop(true),
            nav_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            whats_new_focus: cx.focus_handle().tab_stop(true),
            whats_new_unseen: whats_new::unseen(),
            update: None,
        }
    }

    /// Called once the notes are opened.
    pub(super) fn clear_whats_new_badge(&mut self, cx: &mut Context<Self>) {
        if self.whats_new_unseen {
            self.whats_new_unseen = false;
            cx.notify();
        }
    }

    pub(super) fn observe_update(&mut self, check: &updater::UpdateCheck, cx: &mut Context<Self>) {
        let next = UpdateOffer::from_check(check, self.update.as_ref());
        self.set_update(next, cx);
    }

    pub(super) fn set_update(&mut self, update: Option<UpdateOffer>, cx: &mut Context<Self>) {
        if self.update != update {
            self.update = update;
            cx.notify();
        }
    }

    /// The update prompt at the foot of the rail: what is waiting and one
    /// button. It is static, with no timer or animation.
    fn update_card(
        &self,
        palette: CadencePalette,
        collapsed: bool,
        animate: bool,
    ) -> Option<AnyElement> {
        let offer = self.update.as_ref()?;
        let (title, detail, action) = match offer {
            UpdateOffer::Available(version) => (
                "Update available",
                format!("Sonder {version}"),
                Some("Update now"),
            ),
            #[cfg(target_os = "windows")]
            UpdateOffer::Downloading(version) => (
                "Downloading update",
                format!("Sonder {version}. You can keep listening."),
                None,
            ),
            #[cfg(target_os = "windows")]
            UpdateOffer::Ready(version) => (
                "Update ready",
                format!("Sonder {version}"),
                Some("Restart to update"),
            ),
        };
        let install: fn(&mut App) = match offer {
            #[cfg(target_os = "windows")]
            UpdateOffer::Ready(_) => updater::restart_to_update,
            _ => updater::check_for_updates,
        };
        if collapsed {
            return action.map(|label| {
                components::icon_button(palette, "sidebar-update", CadenceIcon::Download)
                    .test_support()
                    .aria_label(label)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(title).build(window, cx)
                    })
                    .on_click(move |_, _, cx| install(cx))
                    .into_any_element()
            });
        }
        let key = title.len();
        let card = div()
            .flex_none()
            .p_3p5()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface_raised))
            .child(
                div()
                    .text_size(tokens::BODY_TEXT)
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_primary))
                    .child(title),
            )
            .child(
                div()
                    .mt_0p5()
                    .text_size(tokens::BODY_TEXT)
                    .line_height(relative(1.4))
                    .text_color(rgb(palette.text_muted))
                    .child(detail),
            )
            .when_some(action, |card, label| {
                card.child(
                    components::filled_button(palette, "sidebar-update")
                        .test_support()
                        .mt_2p5()
                        .w_full()
                        .h_9()
                        .rounded_full()
                        .bg(rgb(palette.text_primary))
                        .text_color(rgb(palette.canvas))
                        .hover(|style| style.opacity(0.9))
                        .text_size(tokens::BODY_TEXT)
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(label)
                        .on_click(move |_, _, cx| install(cx)),
                )
            });
        // It shows up once, a moment after startup settles, so it eases in. Test
        // renders skip the fade so screenshots never catch it half-drawn.
        if animate && !cfg!(test) {
            Some(
                card.with_animation(
                    ("update-card", key),
                    Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                    |card, progress| card.opacity(progress),
                )
                .into_any_element(),
            )
        } else {
            Some(card.into_any_element())
        }
    }

    pub(super) fn show_route(
        &mut self,
        route: Route,
        pinned_origin: Route,
        active_playlist: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.route != route
            || self.pinned_origin != pinned_origin
            || self.active_playlist != active_playlist
        {
            if route == Route::Playlist
                && self.active_playlist != active_playlist
                && let Some(position) = self.playlist_order.iter().position(|index| {
                    Some(self.playlist_source[*index].source_id.as_str())
                        == active_playlist.as_deref()
                })
                && position > 0
            {
                self.promote_playlist(position, cx);
            }
            self.active_playlist = active_playlist;
            self.route = route;
            self.pinned_origin = pinned_origin;
            cx.notify();
        }
    }

    fn promote_playlist(&mut self, position: usize, cx: &mut Context<Self>) {
        let destination = self.pinned_count;
        // Pins retain their order and cannot be displaced by recency.
        if position <= destination {
            return;
        }
        let mut order = self.playlist_order.to_vec();
        self.reorder_position = position;
        self.reorder_destination = destination;
        self.reorder_started = Some(cx.background_executor().now());
        let index = order.remove(position);
        order.insert(destination, index);
        self.playlist_order = order.into();
        self.playlist_scroll
            .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        self.reorder_generation = self.reorder_generation.wrapping_add(1);
        cx.notify();
    }

    pub(super) fn set_compact_layout(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact_layout != compact {
            self.compact_layout = compact;
            cx.notify();
        }
    }

    pub(super) fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.collapsed == collapsed {
            return;
        }
        self.collapsed = collapsed;
        if let Some(result) = services::AppServices::set_sidebar_collapsed(collapsed, cx)
            && let Some(error) = services::preference_save_error("sidebar", result)
        {
            cx.emit(SidebarEvent::Failed(error));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn rendered_width(&self) -> Rems {
        self.visual_width
    }

    fn nav_item(
        &self,
        item: &NavItem,
        focus_handle: &FocusHandle,
        transition: &RowTransition,
        palette: CadencePalette,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let index = NAV_ITEMS
            .iter()
            .position(|nav| nav.id == item.id)
            .unwrap_or(0);
        let route = self.route;
        let target = item.route;
        let selected = route == target;
        let glyph = if selected {
            item.selected_icon
        } else {
            item.icon
        };
        let base = if selected {
            palette.selection
        } else {
            palette.canvas
        };
        let fill_color = self
            .hover
            .transition(index, cx.background_executor().now())
            .map_or_else(
                || rgb(base),
                |hover| hover.color(window, base, palette.control_hover, cx.reduce_motion()),
            );
        let focus_visible = components::is_focus_visible(focus_handle, window);
        let progress = transition.progress;
        let row_width = transition.row_width;
        // The pill carries selection and hover, sized to what it visually
        // covers: the icon when collapsed, the whole row when expanded.
        let fill = div()
            .h(tokens::NAV_ROW_HEIGHT)
            .rounded_xl()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_3()
            .pr(NAV_ROW_PAD)
            .bg(fill_color)
            .when(focus_visible, |fill| {
                fill.shadow(components::inset_focus_ring(palette))
            })
            .child(
                div()
                    .relative()
                    .w_5()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(components::icon(
                        glyph,
                        tokens::CONTROL_ICON,
                        palette.text_primary,
                    )),
            )
            .child(
                div()
                    .whitespace_nowrap()
                    .child(item.label)
                    .map(move |label| label.opacity(progress)),
            )
            .map(move |fill| {
                let (width, left, pad) =
                    sidebar_fill_geometry(NAV_ROW_PAD, NAV_GLYPH_WIDTH, row_width, progress);
                fill.w(width).ml(left).pl(pad)
            });
        components::bare_button(item.id)
            .test_support()
            .track_focus(focus_handle)
            .aria_selected(selected)
            .w_full()
            .h(tokens::NAV_ROW_HEIGHT)
            .justify_start()
            .text_color(rgb(if selected {
                palette.text_primary
            } else {
                palette.text
            }))
            .text_sm()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .child(fill)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover
                    .set(index, *hovered, cx.background_executor().now());
                cx.notify();
            }))
            .when(self.collapsed, |row| {
                rail_tooltip(row, item_label(index).into(), self.tooltip_anchor.clone())
            })
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SidebarEvent::Navigate(target))))
            .into_any_element()
    }

    /// The foot of the rail: What's new, with a pill while this version's
    /// notes are unopened (a dot when the rail is collapsed), and the version.
    fn whats_new_footer(
        &self,
        transition: &RowTransition,
        palette: CadencePalette,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let index = NAV_ITEMS.len();
        let selected = self.route == Route::WhatsNew;
        let collapsed = self.collapsed;
        let unseen = self.whats_new_unseen;
        let base = if selected {
            palette.selection
        } else {
            palette.canvas
        };
        let fill_color = self
            .hover
            .transition(index, cx.background_executor().now())
            .map_or_else(
                || rgb(base),
                |hover| hover.color(window, base, palette.control_hover, cx.reduce_motion()),
            );
        let focus_visible = components::is_focus_visible(&self.whats_new_focus, window);
        let progress = transition.progress;
        let row_width = transition.row_width;
        let fade = move |element: Div| element.opacity(progress);
        let fill = div()
            .h(tokens::NAV_ROW_HEIGHT)
            .rounded_xl()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_3()
            .pr(NAV_ROW_PAD)
            .bg(fill_color)
            .when(focus_visible, |fill| {
                fill.shadow(components::inset_focus_ring(palette))
            })
            .child(
                div()
                    .relative()
                    .w_5()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(components::icon(
                        CadenceIcon::Sparkle,
                        tokens::CONTROL_ICON,
                        palette.text_primary,
                    ))
                    .when(unseen && collapsed, |icon| {
                        icon.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .size_2()
                                .rounded_full()
                                .bg(rgb(palette.text_primary)),
                        )
                    }),
            )
            .child(div().whitespace_nowrap().child("What’s new").map(fade))
            .when(unseen, |row| {
                row.child(
                    div()
                        .ml_auto()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(rgb(palette.text_primary))
                        .text_color(rgb(palette.canvas))
                        .text_size(tokens::CAPTION_TEXT)
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("New")
                        .map(fade),
                )
            })
            .map(move |fill| {
                let (width, left, pad) =
                    sidebar_fill_geometry(NAV_ROW_PAD, NAV_GLYPH_WIDTH, row_width, progress);
                fill.w(width).ml(left).pl(pad)
            });
        let row = components::bare_button("sidebar-whats-new")
            .test_support()
            .track_focus(&self.whats_new_focus)
            .aria_selected(selected)
            .w_full()
            .h(tokens::NAV_ROW_HEIGHT)
            .justify_start()
            .text_color(rgb(if selected {
                palette.text_primary
            } else {
                palette.text
            }))
            .text_sm()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .child(fill)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover
                    .set(index, *hovered, cx.background_executor().now());
                cx.notify();
            }))
            .when(collapsed, |row| {
                rail_tooltip(row, "What’s new".into(), self.tooltip_anchor.clone())
            })
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(SidebarEvent::Navigate(Route::WhatsNew));
            }));
        div()
            .flex_none()
            .flex()
            .flex_col()
            .pt_2()
            .border_t_1()
            .border_color(rgb(palette.border))
            .child(row)
            .when(!collapsed, |footer| {
                footer.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_size(tokens::CAPTION_TEXT)
                        .text_color(rgb(palette.text_muted))
                        .child(concat!("Sonder ", env!("CARGO_PKG_VERSION"))),
                )
            })
            .into_any_element()
    }

    fn panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let collapsed = self.collapsed;
        let pinned_origin = self.pinned_origin;
        let expanded_width = expanded_sidebar_width(self.compact_layout);
        let frame = self.progress.frame(
            if collapsed { 0. } else { 1. },
            super::motion::sidebar_spring(),
            window,
            cx,
        );
        let progress = (frame.value() as f32).clamp(0., 1.);
        let width_range = expanded_width - COLLAPSED_SIDEBAR_WIDTH;
        // The rail's real width follows the spring, so the page edge slides with it.
        let shown_width = COLLAPSED_SIDEBAR_WIDTH + width_range * progress;
        self.visual_width = shown_width;
        let row_width = expanded_width - SIDEBAR_CONTENT_PAD * 2.;
        let transition = RowTransition {
            progress,
            row_width,
        };
        let show_local_files = self.library.read(cx).has_local_files();
        let nav_rows: Vec<_> = NAV_ITEMS
            .iter()
            .zip(&self.nav_focus)
            .filter(|(item, _)| item.route != Route::LocalFiles || show_local_files)
            .map(|(item, focus_handle)| {
                self.nav_item(item, focus_handle, &transition, palette, window, cx)
            })
            .collect();
        let whats_new_footer = self.whats_new_footer(&transition, palette, window, cx);
        let library = self.library.read(cx);
        let playlists = if library.playlists().is_empty() {
            library.pinned_playlists().clone()
        } else {
            library.playlists().clone()
        };
        let pins = library.pinned_playlists().clone();
        let catalog_changed = !Arc::ptr_eq(&self.playlist_source, &playlists);
        let pins_changed = !Arc::ptr_eq(&self.pinned_source, &pins)
            && self.pinned_source.as_ref() != pins.as_ref();
        if catalog_changed || pins_changed {
            // Rebuild only when either Arc changes, never per hover/frame.
            let pinned_keys: HashSet<_> = pins
                .iter()
                .map(|p| (p.provider, p.source_id.as_str()))
                .collect();
            let index_at = |position| {
                if catalog_changed {
                    position
                } else {
                    self.playlist_order[position]
                }
            };
            let is_pinned = |index: &usize| {
                pinned_keys.contains(&(
                    playlists[*index].provider,
                    playlists[*index].source_id.as_str(),
                ))
            };
            let mut order = Vec::with_capacity(playlists.len());
            order.extend((0..playlists.len()).map(index_at).filter(is_pinned));
            self.pinned_count = order.len();
            order.extend(
                (0..playlists.len())
                    .map(index_at)
                    .filter(|index| !is_pinned(index)),
            );
            self.playlist_source = playlists.clone();
            self.pinned_source = pins;
            self.playlist_order = order.into();
            self.playlist_hover = components::HoverHighlight::default();
            self.reorder_started = None;
            self.reorder_generation = 0;
        } else {
            // Local favorite/history writes can return an equal new pin snapshot.
            // Adopt its Arc without rebuilding the playlist order.
            self.pinned_source = pins;
        }
        let pinned_count = self.pinned_count;
        let order = self.playlist_order.clone();
        let active_playlist = if self.route == Route::Playlist {
            self.active_playlist.clone()
        } else {
            None
        };
        let reorder_generation = self.reorder_generation;
        let reorder_position = self.reorder_position;
        let reorder_destination = self.reorder_destination;
        let show_pinned = !playlists.is_empty();
        let reduce_motion = cx.reduce_motion();
        let reorder_progress = if reduce_motion {
            1.
        } else {
            self.reorder_started.map_or(1., |start| {
                (cx.background_executor()
                    .now()
                    .saturating_duration_since(start)
                    .as_secs_f32()
                    / 0.14)
                    .clamp(0., 1.)
            })
        };
        if reorder_progress < 1. {
            window.request_animation_frame();
        }
        let reorder_progress = ease_out_quint()(reorder_progress);
        // Labels stay mounted until the rail has fully closed, fading and
        // shrinking with the spring, so nothing pops in or out mid-slide.
        let labels_shown = !collapsed || progress > 0.001;
        // A row's side padding eases from centring the cover on the rail to
        // the expanded inset, so the cover glides instead of snapping.
        let collapsed_pad = (COLLAPSED_SIDEBAR_WIDTH
            - SIDEBAR_CONTENT_PAD * 2.
            - tokens::SIDEBAR_PLAYLIST_ARTWORK.size)
            / 2.;
        let row_pad = collapsed_pad + (Rems(0.75) - collapsed_pad) * progress;
        let pinned_section = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .when(labels_shown, |section| {
                section.child(
                    collapsing(
                        div().child(components::section_label(palette, "Playlists")),
                        progress,
                        SECTION_LABEL_HEIGHT,
                        Rems(0.5),
                    )
                    .opacity(progress),
                )
            })
            .child(
                uniform_list(
                    "sidebar-playlists",
                    playlists.len(),
                    cx.processor(move |this, range: Range<usize>, window, cx| {
                        range
                            .filter_map(|position| {
                                let index = order[position];
                                playlists.get(index).cloned().map(|playlist| {
                                    let selected_playlist = playlist.clone();
                                    let label = playlist.name.clone();
                                    let selected = active_playlist.as_deref()
                                        == Some(playlist.source_id.as_str());
                                    let base = if selected {
                                        palette.selection
                                    } else {
                                        palette.canvas
                                    };
                                    let pinned = position < pinned_count;
                                    let row = components::row_button(
                                        palette,
                                        (
                                            if pinned {
                                                "pinned-playlist"
                                            } else {
                                                "sidebar-playlist"
                                            },
                                            index,
                                        ),
                                    )
                                    .test_support()
                                    .h_11()
                                    .w_full()
                                    .min_w_0()
                                    .bg(rgb(base))
                                    .aria_selected(selected)
                                    .rounded_lg()
                                    // Settled and collapsed, the cover is centred as before; while the
                                    // rail moves its padding eases between the two positions.
                                    .map(move |row| {
                                        if progress <= 0. {
                                            row.justify_center()
                                        } else {
                                            row.justify_start().px(row_pad)
                                        }
                                    })
                                    .gap_2()
                                    .text_sm()
                                    .text_color(rgb(palette.text))
                                    .child(components::artwork(
                                        palette,
                                        &this.image_cache,
                                        playlist.artwork_url.as_deref(),
                                        tokens::SIDEBAR_PLAYLIST_ARTWORK,
                                        CadenceIcon::Playlist,
                                    ))
                                    .when(pinned && labels_shown, |row| {
                                        row.child(div().flex_none().opacity(progress).child(
                                            components::icon(
                                                CadenceIcon::PinFilled,
                                                Rems(0.625),
                                                palette.text_muted,
                                            ),
                                        ))
                                    })
                                    .when(labels_shown, |row| {
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .line_clamp(1)
                                                .opacity(progress)
                                                .child(playlist.name),
                                        )
                                    })
                                    .when(collapsed, |row| {
                                        rail_tooltip(
                                            row,
                                            label.clone(),
                                            this.tooltip_anchor.clone(),
                                        )
                                    })
                                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                        this.playlist_hover.set(
                                            index,
                                            *hovered,
                                            cx.background_executor().now(),
                                        );
                                        cx.notify();
                                    }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(position) =
                                            this.playlist_order.iter().position(|id| *id == index)
                                        {
                                            this.promote_playlist(position, cx);
                                        }
                                        cx.emit(SidebarEvent::OpenPlaylist {
                                            playlist: selected_playlist.clone(),
                                            origin: pinned_origin,
                                        })
                                    }));
                                    let row = if let Some(hover) = this
                                        .playlist_hover
                                        .transition(index, cx.background_executor().now())
                                    {
                                        hover.apply(
                                            row,
                                            window,
                                            base,
                                            palette.control_hover,
                                            reduce_motion,
                                        )
                                    } else {
                                        row.into_any_element()
                                    };
                                    let offset = if reorder_generation == 0 {
                                        0.
                                    } else if position == reorder_destination {
                                        (reorder_position - reorder_destination) as f32
                                    } else if position > reorder_destination
                                        && position <= reorder_position
                                    {
                                        -1.
                                    } else {
                                        0.
                                    };
                                    div()
                                        .relative()
                                        .w_full()
                                        .top(Rems(offset * 2.75 * (1. - reorder_progress)))
                                        .child(row)
                                        .into_any_element()
                                })
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.playlist_scroll)
                .flex_1()
                .min_h_0(),
            );
        let toggle_focus_visible = components::is_focus_visible(&self.toggle_focus, window);
        let brand_fill = div()
            .h_12()
            .rounded_xl()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(tokens::BRAND_LABEL_GAP)
            .pr(BRAND_ROW_PAD)
            .when(toggle_focus_visible, |fill| {
                fill.shadow(components::inset_focus_ring(palette))
            })
            .hover(|style| style.bg(rgb(palette.control)))
            .child(
                img(self.brand_mark.clone())
                    .size(BRAND_LOGO_SIZE)
                    .flex_none(),
            )
            .child(
                div()
                    .whitespace_nowrap()
                    .child("Sonder")
                    .map(move |label| label.opacity(progress)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(tokens::CONTROL_ICON)
                    .h_12()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(components::icon(
                        CadenceIcon::ChevronLeft,
                        tokens::CONTROL_ICON,
                        palette.text_primary,
                    ))
                    .map(move |button| button.opacity(progress)),
            )
            .map(move |fill| {
                let (width, left, pad) =
                    sidebar_fill_geometry(BRAND_ROW_PAD, BRAND_LOGO_SIZE, row_width, progress);
                fill.w(width).ml(left).pl(pad)
            });
        let brand = components::bare_button("sidebar-toggle")
            .test_support()
            .track_focus(&self.toggle_focus)
            .h_12()
            .w_full()
            .flex_none()
            .justify_start()
            .items_center()
            .text_color(rgb(palette.text_primary))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .child(brand_fill)
            .tooltip_show_delay(Duration::ZERO)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(if collapsed {
                    "Expand sidebar"
                } else {
                    "Collapse sidebar"
                })
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                let collapsed = !this.collapsed;
                this.set_collapsed(collapsed, cx);
            }));

        div()
            .w(shown_width)
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(rgb(palette.canvas))
            .border_r_1()
            .border_color(rgb(palette.border))
            .child(
                div()
                    .w(shown_width)
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p(SIDEBAR_CONTENT_PAD)
                    .pt(tokens::SIDEBAR_TOP_INSET)
                    .child(brand)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .when(labels_shown, |nav| {
                                // The label sits outside the rows' gapped column, and the
                                // space under it is its own padding on an inner div, so one
                                // height shrinks to nothing: a negative margin or padding on
                                // the shrinking block itself leaves a few pixels behind that
                                // jump when it unmounts.
                                nav.child(
                                    collapsing(
                                        div().child(
                                            div().px_3().pb_2().child(components::section_label(
                                                palette, "Library",
                                            )),
                                        ),
                                        progress,
                                        SECTION_LABEL_HEIGHT + Rems(0.5),
                                        Rems(0.),
                                    )
                                    .opacity(progress),
                                )
                            })
                            .child(div().flex().flex_col().gap_1().children(nav_rows)),
                    )
                    .when(show_pinned, |sidebar| {
                        sidebar.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .child(pinned_section)
                                .map(move |pinned| {
                                    pinned.opacity(if collapsed { 1. } else { progress })
                                }),
                        )
                    })
                    .when(!show_pinned, |panel| panel.child(div().flex_1()))
                    .children(self.update_card(palette, collapsed, !cx.reduce_motion()))
                    .child(whats_new_footer),
            )
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel(window, cx)
    }
}

/// Native pointer tooltips use the pointer's bottom-right corner. Rail labels
/// instead follow the trigger's current layout, including scrolling/reordering.
/// One view exists only while hovered; no animation or timer is added.
fn rail_tooltip<E: StatefulInteractiveElement + gpui_kit::component::ElementExt>(
    row: E,
    label: SharedString,
    bounds: Rc<Cell<Bounds<Pixels>>>,
) -> E {
    let writer = bounds.clone();
    row.on_prepaint(move |value, window, _| {
        if value.contains(&window.mouse_position()) {
            writer.set(value);
        }
    })
    .tooltip_show_delay(Duration::ZERO)
    .tooltip(move |_, cx| {
        cx.new(|_| RailTooltip {
            bounds: bounds.clone(),
            label: label.clone(),
        })
        .into()
    })
}

struct RailTooltip {
    bounds: Rc<Cell<Bounds<Pixels>>>,
    label: SharedString,
}

impl Render for RailTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let bounds = self.bounds.get();
        gpui_kit::base::Positioner::side(bounds)
            .placement(gpui_kit::base::Placement::Right)
            .offset(px(8.))
            .child(
                div()
                    .id("sidebar-tooltip")
                    .test_support()
                    .max_w(Rems(22.))
                    .py(Rems(0.375))
                    .px_3()
                    .line_height(Rems(1.25))
                    .rounded_md()
                    .bg(rgb(palette.surface_raised))
                    .text_color(rgb(palette.text_primary))
                    .text_size(Rems(0.875))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(div().max_w(Rems(20.5)).child(self.label.clone())),
            )
    }
}
