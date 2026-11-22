//! The Audio tab: an equalizer drawn as one curve with five points.
//!
//! The curve is the real response of the five filters the player runs, so
//! what the graph shows is what the speakers do. Dragging a point asks which
//! gain puts the curve through the pointer; the sound follows every move of
//! the drag, and the file is written once when the pointer lets go.

use super::*;
use crate::equalizer::{self as eq, CustomPreset};
use gpui_kit::base::ElementExt as _;

/// The graph, without its frequency labels.
const GRAPH_HEIGHT: Rems = Rems(16.25);
/// The row of frequency labels under the graph.
const LABEL_ROW_HEIGHT: Rems = Rems(1.75);
/// Gap kept above +12 dB and below -12 dB, as a fraction of the graph.
const MARGIN: f32 = 18. / 260.;
/// The decibels from the middle line to the top (and the bottom).
const SPAN_DB: f32 = 12.;
/// Points along the curve. The curve is smooth, so this looks continuous.
const SAMPLES: usize = 96;
const GRID_DB: [i32; 5] = [12, 6, 0, -6, -12];
const FREQUENCY_LABELS: [&str; eq::BAND_COUNT] = ["60 Hz", "250 Hz", "1k", "4k", "12k"];
/// A point's radius at rest and while held, and the ring around it.
const POINT_RADIUS: f32 = 8.;
const POINT_RADIUS_HELD: f32 = 10.;
const POINT_RING: f32 = 3.;
/// Up and down arrows move a point one decibel; page keys three.
const KEY_STEP_DB: f32 = 1.;
const KEY_STEP_LARGE_DB: f32 = 3.;

/// Payload for the drag that keeps pointer moves arriving once the pointer
/// leaves the graph. See [`scrubber::ScrubberDrag`].
#[derive(Clone)]
pub(super) struct EqualizerDrag;

/// Where a gain in decibels falls down a graph `height` tall.
fn y_for_db(db: f32, height: f32) -> f32 {
    height / 2. - db / SPAN_DB * (height / 2. - height * MARGIN)
}

/// The inverse of [`y_for_db`], limited to the plotted range.
fn db_for_y(y: f32, height: f32) -> f32 {
    ((height / 2. - y) / (height / 2. - height * MARGIN) * SPAN_DB).clamp(-SPAN_DB, SPAN_DB)
}

fn signed_db(db: f32) -> String {
    if db == 0. {
        "0 dB".to_owned()
    } else {
        format!("{db:+} dB")
    }
}

impl Settings {
    pub(in crate::app) fn comparing(&self) -> bool {
        self.eq_bypass
    }

    /// Compare is for listening, so it never outlives the tab it belongs to.
    pub(in crate::app) fn end_compare(&mut self, cx: &mut Context<Self>) {
        if self.eq_bypass {
            self.eq_bypass = false;
            self.publish_equalizer(cx);
        }
    }

    fn equalizer(cx: &App) -> eq::Settings {
        services::AppServices::preferences(cx).equalizer
    }

    fn publish_equalizer(&mut self, cx: &mut Context<Self>) {
        let settings = Self::equalizer(cx);
        services::AppServices::apply_equalizer(settings, self.eq_bypass, cx);
        cx.notify();
    }

    /// Changes the curve. Shaping the sound turns the equalizer on and leaves
    /// Compare, so an edit is always heard.
    fn edit_equalizer(&mut self, change: impl FnOnce(&mut eq::Settings), cx: &mut Context<Self>) {
        let mut settings = Self::equalizer(cx);
        let before = settings;
        change(&mut settings);
        settings.enabled = true;
        if settings == before && !self.eq_bypass {
            return;
        }
        self.eq_bypass = false;
        services::AppServices::apply_equalizer(settings, false, cx);
        cx.notify();
    }

    fn commit_equalizer(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::SaveEqualizer(Self::equalizer(cx)));
    }

    fn set_equalizer_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let mut settings = Self::equalizer(cx);
        settings.enabled = enabled;
        if !enabled {
            self.eq_bypass = false;
        }
        services::AppServices::apply_equalizer(settings, self.eq_bypass, cx);
        self.commit_equalizer(cx);
        cx.notify();
    }

    fn toggle_compare(&mut self, cx: &mut Context<Self>) {
        if !Self::equalizer(cx).enabled {
            return;
        }
        self.eq_bypass = !self.eq_bypass;
        self.publish_equalizer(cx);
    }

    fn choose_preset(&mut self, steps: [i8; eq::BAND_COUNT], cx: &mut Context<Self>) {
        self.eq_note = None;
        self.edit_equalizer(|settings| settings.steps = steps, cx);
        self.commit_equalizer(cx);
    }

    fn nudge_band(&mut self, band: usize, db: f32, cx: &mut Context<Self>) {
        let delta = (db / eq::STEP_DB).round() as i32;
        self.edit_equalizer(
            |settings| {
                let next = (i32::from(settings.steps[band]) + delta).clamp(-24, 24);
                settings.steps[band] = next as i8;
            },
            cx,
        );
        self.commit_equalizer(cx);
    }

    fn reset_band(&mut self, band: usize, cx: &mut Context<Self>) {
        self.edit_equalizer(|settings| settings.steps[band] = 0, cx);
        self.commit_equalizer(cx);
    }

    /// Moves `band` so the curve passes through the pointer's height.
    fn drag_band_to(&mut self, band: usize, pointer_y: Pixels, cx: &mut Context<Self>) {
        let bounds = self.eq_graph.get();
        let height = f32::from(bounds.size.height);
        if height <= 0. {
            return;
        }
        let target = db_for_y(f32::from(pointer_y - bounds.origin.y), height);
        let gains = Self::equalizer(cx).gains_db();
        let step = eq::Settings::step_for(eq::gain_for_response(gains, band, target));
        self.edit_equalizer(|settings| settings.steps[band] = step, cx);
    }

    /// The band whose label is nearest the pointer across the graph.
    fn band_at(&self, pointer_x: Pixels) -> Option<usize> {
        let bounds = self.eq_graph.get();
        let width = f32::from(bounds.size.width);
        if width <= 0. {
            return None;
        }
        let across = f32::from(pointer_x - bounds.origin.x) / width;
        (0..eq::BAND_COUNT).min_by(|a, b| {
            (eq::ANCHORS[*a] - across)
                .abs()
                .total_cmp(&(eq::ANCHORS[*b] - across).abs())
        })
    }

    #[cfg(test)]
    pub(in crate::app) fn name_preset_for_test(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.eq_name
            .update(cx, |input, cx| input.set_value(name.to_owned(), window, cx));
    }

    fn begin_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.eq_naming = true;
        self.eq_note = None;
        self.eq_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        window.focus(&self.eq_name.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn cancel_naming(&mut self, cx: &mut Context<Self>) {
        self.eq_naming = false;
        self.eq_note = None;
        cx.notify();
    }

    pub(super) fn save_preset(&mut self, cx: &mut Context<Self>) {
        let name: String = self
            .eq_name
            .read(cx)
            .value()
            .trim()
            .chars()
            .take(eq::MAX_PRESET_NAME_CHARS)
            .collect();
        if name.is_empty() {
            self.eq_note = Some("Give the preset a name.".into());
            cx.notify();
            return;
        }
        let taken = |other: &str| other.eq_ignore_ascii_case(&name);
        if eq::PRESETS.iter().any(|preset| taken(preset.name)) {
            self.eq_note = Some("That name belongs to a built-in preset.".into());
            cx.notify();
            return;
        }
        let steps = Self::equalizer(cx).steps;
        if let Some(existing) = self.eq_presets.iter_mut().find(|p| taken(&p.name)) {
            existing.steps = steps;
        } else if self.eq_presets.len() >= eq::MAX_CUSTOM_PRESETS {
            self.eq_note = Some(
                format!(
                    "You can keep {} presets. Delete one to save another.",
                    eq::MAX_CUSTOM_PRESETS
                )
                .into(),
            );
            cx.notify();
            return;
        } else {
            self.eq_presets.push(CustomPreset { name, steps });
        }
        self.eq_naming = false;
        self.eq_note = None;
        cx.emit(SettingsEvent::SaveEqualizerPresets(self.eq_presets.clone()));
        cx.notify();
    }

    fn delete_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.eq_presets.len() {
            self.eq_presets.remove(index);
            self.eq_note = None;
            cx.emit(SettingsEvent::SaveEqualizerPresets(self.eq_presets.clone()));
            cx.notify();
        }
    }

    pub(super) fn audio_tab(
        &mut self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let preferences = services::AppServices::preferences(cx);
        let settings = preferences.equalizer;
        let enabled = settings.enabled;
        let comparing = self.eq_bypass && enabled;
        let gains = settings.gains_db();
        let curve = eq::Curve::new(gains);
        let rem = window.rem_size();
        let graph_height = f32::from(GRAPH_HEIGHT.to_pixels(rem));
        let held = self.eq_drag;
        let focus_visible: [bool; eq::BAND_COUNT] =
            std::array::from_fn(|band| components::is_focus_visible(&self.eq_focus[band], window));
        let point_y: [f32; eq::BAND_COUNT] = std::array::from_fn(|band| {
            y_for_db(
                curve.db_at(eq::FREQUENCIES[band]).clamp(-SPAN_DB, SPAN_DB),
                graph_height,
            )
        });

        let graph_bounds = self.eq_graph.clone();
        let paint_state = GraphPaint {
            gains,
            grid: rgb(palette.text_primary),
            line: rgb(palette.text_primary),
            point: rgb(palette.text_primary),
            ring: rgb(palette.surface),
            focus: rgb(palette.focus_ring),
            held,
            focus_visible,
        };

        let hit_areas = (0..eq::BAND_COUNT).map(|band| {
            let db = settings.gains_db()[band];
            div()
                .id(("equalizer-band", band))
                .test_support()
                .track_focus(&self.eq_focus[band])
                .key_context(EQUALIZER_BAND_KEY_CONTEXT)
                .role(gpui_kit::Role::Slider)
                .aria_label(format!("{} equalizer band", FREQUENCY_LABELS[band]))
                .aria_orientation(gpui_kit::Orientation::Vertical)
                .aria_min_numeric_value(-f64::from(eq::MAX_GAIN_DB))
                .aria_max_numeric_value(f64::from(eq::MAX_GAIN_DB))
                .aria_numeric_value(f64::from(db))
                .aria_value(signed_db(db))
                .on_action(cx.listener(move |this, _: &EqualizerBandUp, _, cx| {
                    this.nudge_band(band, KEY_STEP_DB, cx);
                }))
                .on_action(cx.listener(move |this, _: &EqualizerBandDown, _, cx| {
                    this.nudge_band(band, -KEY_STEP_DB, cx);
                }))
                .on_action(cx.listener(move |this, _: &EqualizerBandUpLarge, _, cx| {
                    this.nudge_band(band, KEY_STEP_LARGE_DB, cx);
                }))
                .on_action(cx.listener(move |this, _: &EqualizerBandDownLarge, _, cx| {
                    this.nudge_band(band, -KEY_STEP_LARGE_DB, cx);
                }))
                .on_action(cx.listener(move |this, _: &EqualizerBandReset, _, cx| {
                    this.reset_band(band, cx);
                }))
                .on_a11y_action(gpui_kit::AccessibleAction::Increment, {
                    let handle = cx.entity().downgrade();
                    move |_, _, cx| {
                        handle
                            .update(cx, |this, cx| this.nudge_band(band, KEY_STEP_DB, cx))
                            .ok();
                    }
                })
                .on_a11y_action(gpui_kit::AccessibleAction::Decrement, {
                    let handle = cx.entity().downgrade();
                    move |_, _, cx| {
                        handle
                            .update(cx, |this, cx| this.nudge_band(band, -KEY_STEP_DB, cx))
                            .ok();
                    }
                })
                .absolute()
                .left(relative(eq::ANCHORS[band]))
                .top(px(point_y[band]))
                .ml(px(-14.))
                .mt(px(-14.))
                .size(px(28.))
        });

        let grid_labels = GRID_DB.into_iter().map(|db| {
            div()
                .absolute()
                .left(px(10.))
                .top(px(y_for_db(db as f32, graph_height) - 14.))
                .text_size(Rems(0.65625))
                .text_color(rgb(palette.text_muted).opacity(0.75))
                .child(if db > 0 {
                    format!("+{db}")
                } else {
                    db.to_string()
                })
        });

        let bubble = held.map(|band| {
            let db = gains[band];
            div()
                .absolute()
                .left(relative(eq::ANCHORS[band]))
                .top(px((point_y[band] - 44.).max(4.)))
                .ml(px(-30.))
                .w(px(60.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(palette.control))
                .text_size(Rems(0.6875))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text_primary))
                .child(signed_db(db))
        });

        let graph = div()
            .id("equalizer-graph")
            .test_support()
            .relative()
            .w_full()
            .h(GRAPH_HEIGHT)
            .rounded(px(12.))
            .bg(rgb(palette.surface_hover))
            .cursor(gpui_kit::CursorStyle::ResizeUpDown)
            .when(!enabled, |graph| graph.opacity(0.55))
            .on_prepaint(move |bounds, _, _| graph_bounds.set(bounds))
            .on_drag(EqualizerDrag, |_, _, _, cx| {
                cx.new(|_| scrubber::NoDragPreview)
            })
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    // The click that activates a background window should
                    // not also move a point.
                    if event.first_mouse {
                        return;
                    }
                    let Some(band) = this.band_at(event.position.x) else {
                        return;
                    };
                    window.focus(&this.eq_focus[band], cx);
                    if event.click_count >= 2 {
                        this.reset_band(band, cx);
                        return;
                    }
                    this.eq_drag = Some(band);
                    this.drag_band_to(band, event.position.y, cx);
                    cx.notify();
                }),
            )
            .on_drag_move(cx.listener(
                |this, event: &gpui_kit::DragMoveEvent<EqualizerDrag>, _, cx| {
                    if let Some(band) = this.eq_drag {
                        this.drag_band_to(band, event.event.position.y, cx);
                    }
                },
            ))
            // Outside the drag handlers: a frame that cannot start a gesture
            // must still be able to end one that an earlier frame started.
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| this.end_drag(cx)),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| this.end_drag(cx)),
            )
            .child(
                gpui_kit::canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| paint_graph(bounds, &paint_state, window),
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .children(grid_labels)
            .children(hit_areas)
            .children(bubble);

        let frequency_labels =
            div()
                .relative()
                .w_full()
                .h(LABEL_ROW_HEIGHT)
                .children((0..eq::BAND_COUNT).map(|band| {
                    div()
                        .absolute()
                        .left(relative(eq::ANCHORS[band]))
                        .top(px(6.))
                        .ml(px(-30.))
                        .w(px(60.))
                        .text_center()
                        .text_size(Rems(0.71875))
                        .text_color(rgb(palette.text_muted))
                        .child(FREQUENCY_LABELS[band])
                }));

        let on_pill =
            |steps: [i8; eq::BAND_COUNT]| enabled && !comparing && settings.steps == steps;
        let built_in = eq::PRESETS.iter().enumerate().map(|(index, preset)| {
            let steps = eq::Settings::default().with_gains(preset.gains).steps;
            components::pill(
                palette,
                ("equalizer-preset", index),
                preset.name,
                on_pill(steps) || (!enabled && index == 0 && settings.is_flat()),
            )
            .test_support()
            .h_9()
            .on_click(cx.listener(move |this, _, _, cx| this.choose_preset(steps, cx)))
            .into_any_element()
        });
        let custom = self.eq_presets.iter().enumerate().map(|(index, preset)| {
            let steps = preset.steps;
            let active = on_pill(steps);
            div()
                .relative()
                .child(
                    components::pill(
                        palette,
                        ("equalizer-custom-preset", index),
                        preset.name.clone(),
                        active,
                    )
                    .test_support()
                    .h_9()
                    .pr_9()
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_preset(steps, cx))),
                )
                .child(
                    div()
                        .id(("equalizer-custom-preset-delete", index))
                        .test_support()
                        .absolute()
                        .right(px(7.))
                        .top(px(7.))
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(palette.text_primary).opacity(0.16)))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Delete preset")
                                .build(window, cx)
                        })
                        .child(components::icon(
                            CadenceIcon::Close,
                            Rems(0.75),
                            if active {
                                palette.surface
                            } else {
                                palette.text_primary
                            },
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_preset(index, cx))),
                )
                .into_any_element()
        });

        let soloist = preferences.player == crate::playback::Engine::Soloist;

        div()
            .child(Self::settings_section_header(
                palette,
                "Equalizer",
                "Drag the points to shape the sound.",
            ))
            .child(
                div()
                    .mt_4()
                    .rounded_2xl()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface_raised))
                    .p_6()
                    .child(Self::playback_setting_row(
                        palette,
                        "Equalizer",
                        "Turn it off to hear the original sound.",
                        components::focus_ring_around(
                            palette,
                            &self.eq_switch_focus,
                            window,
                            cx,
                            Switch::new("settings-equalizer")
                                .cursor_pointer()
                                .focus_ring(false)
                                .checked(enabled)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.set_equalizer_enabled(*checked, cx);
                                })),
                        ),
                    ))
                    .when(soloist, |card| {
                        card.child(
                            div()
                                .mt_3()
                                .text_size(tokens::BODY_TEXT)
                                .text_color(rgb(palette.text_muted))
                                .child("Spotify’s own player does not use the equalizer. Switch to the built-in player in General."),
                        )
                    })
                    .child(div().mt_8().child(graph).child(frequency_labels))
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .flex_wrap()
                                    .gap_2()
                                    .children(built_in)
                                    .children(custom),
                            )
                            .child(
                                div()
                                    .h_9()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap_3()
                                    .when(!enabled, |row| row.opacity(0.45))
                                    .child(
                                        div()
                                            .text_size(tokens::BODY_TEXT)
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette.text))
                                            .child("Compare"),
                                    )
                                    .child(components::focus_ring_around(
                                        palette,
                                        &self.eq_compare_focus,
                                        window,
                                        cx,
                                        Switch::new("settings-equalizer-compare")
                                            .cursor_pointer()
                                            .focus_ring(false)
                                            .checked(comparing)
                                            .on_click(cx.listener(|this, _: &bool, _, cx| {
                                                this.toggle_compare(cx);
                                            })),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .mt_3()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .children(self.eq_note.clone().map(|note| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(tokens::BODY_TEXT)
                                    .text_color(rgb(palette.text_muted))
                                    .child(note)
                            }))
                            .when(self.eq_naming, |row| {
                                row.child(
                                    div().w(Rems(13.)).child(
                                        Input::new(&self.eq_name)
                                            .id("equalizer-preset-name")
                                            .w_full(),
                                    ),
                                )
                                .child(
                                    components::settings_button(
                                        palette,
                                        "equalizer-save-confirm",
                                        "Save",
                                    )
                                    .test_support()
                                    .on_click(cx.listener(|this, _, _, cx| this.save_preset(cx))),
                                )
                                .child(
                                    components::settings_button(
                                        palette,
                                        "equalizer-save-cancel",
                                        "Cancel",
                                    )
                                    .test_support()
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_naming(cx))),
                                )
                            })
                            .when(!self.eq_naming, |row| {
                                row.child(
                                    components::settings_button(
                                        palette,
                                        "equalizer-save-preset",
                                        "Save as preset",
                                    )
                                    .test_support()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_naming(window, cx);
                                    })),
                                )
                            }),
                    ),
            )
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.eq_drag.take().is_some() {
            self.commit_equalizer(cx);
            cx.notify();
        }
    }
}

/// Everything the graph's paint pass needs, copied out of the page so the
/// pass owns its data.
struct GraphPaint {
    gains: [f32; eq::BAND_COUNT],
    grid: gpui_kit::Rgba,
    line: gpui_kit::Rgba,
    point: gpui_kit::Rgba,
    ring: gpui_kit::Rgba,
    focus: gpui_kit::Rgba,
    held: Option<usize>,
    focus_visible: [bool; eq::BAND_COUNT],
}

fn paint_graph(bounds: Bounds<Pixels>, state: &GraphPaint, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0. || height <= 0. {
        return;
    }
    let at = |x: f32, y: f32| gpui_kit::point(bounds.origin.x + px(x), bounds.origin.y + px(y));

    // The grid: a dashed line every 6 dB, a firm one at 0 dB, and a faint
    // upright through every label.
    for db in GRID_DB {
        let y = y_for_db(db as f32, height);
        if db == 0 {
            window.paint_quad(gpui_kit::fill(
                Bounds::new(at(0., y - 0.5), gpui_kit::size(px(width), px(1.))),
                state.grid.opacity(0.2),
            ));
        } else {
            let mut path = gpui_kit::PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(4.)]);
            path.move_to(at(0., y));
            path.line_to(at(width, y));
            if let Ok(path) = path.build() {
                window.paint_path(path, state.grid.opacity(0.09));
            }
        }
    }
    for anchor in eq::ANCHORS {
        window.paint_quad(gpui_kit::fill(
            Bounds::new(
                at(anchor * width - 0.5, 0.),
                gpui_kit::size(px(1.), px(height)),
            ),
            state.grid.opacity(0.07),
        ));
    }

    // The curve, read off the real filters, with a wash toward the 0 dB line.
    let curve = eq::Curve::new(state.gains);
    let middle = y_for_db(0., height);
    let samples: Vec<(f32, f32)> = (0..SAMPLES)
        .map(|index| {
            let across = index as f32 / (SAMPLES - 1) as f32;
            let db = curve
                .db_at(eq::frequency_at(across))
                .clamp(-SPAN_DB - 1., SPAN_DB + 1.);
            (across * width, y_for_db(db, height))
        })
        .collect();
    if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let mut wash = gpui_kit::PathBuilder::fill();
        wash.move_to(at(first.0, middle));
        for (x, y) in &samples {
            wash.line_to(at(*x, *y));
        }
        wash.line_to(at(last.0, middle));
        wash.close();
        if let Ok(path) = wash.build() {
            window.paint_path(path, state.line.opacity(0.10));
        }
        let mut stroke = gpui_kit::PathBuilder::stroke(px(2.5));
        stroke.move_to(at(first.0, first.1));
        for (x, y) in samples.iter().skip(1) {
            stroke.line_to(at(*x, *y));
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, state.line);
        }
    }

    // The points sit on the curve, each in a ring of the graph's own colour.
    for band in 0..eq::BAND_COUNT {
        let x = eq::ANCHORS[band] * width;
        let db = curve.db_at(eq::FREQUENCIES[band]).clamp(-SPAN_DB, SPAN_DB);
        let y = y_for_db(db, height);
        let radius = if state.held == Some(band) {
            POINT_RADIUS_HELD
        } else {
            POINT_RADIUS
        };
        let disc = |radius: f32, color: gpui_kit::Rgba| {
            gpui_kit::fill(
                Bounds::new(
                    at(x - radius, y - radius),
                    gpui_kit::size(px(radius * 2.), px(radius * 2.)),
                ),
                color,
            )
            .corner_radii(gpui_kit::Corners::all(px(radius)))
        };
        if state.focus_visible[band] {
            window.paint_quad(disc(radius + POINT_RING + 3., state.focus));
        }
        window.paint_quad(disc(radius + POINT_RING / 2., state.ring));
        window.paint_quad(disc(radius - POINT_RING / 2., state.point));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_maps_decibels_to_height_and_back() {
        let height = 260.;
        assert_eq!(y_for_db(0., height), 130.);
        assert!(y_for_db(12., height) < y_for_db(6., height));
        // 18 px of margin above +12 and below -12.
        assert!((y_for_db(12., height) - 18.).abs() < 1e-3);
        assert!((y_for_db(-12., height) - 242.).abs() < 1e-3);
        for db in [-12., -4.5, 0., 3., 12.] {
            assert!((db_for_y(y_for_db(db, height), height) - db).abs() < 1e-3);
        }
        // A pointer past the ends asks for no more than the range.
        assert_eq!(db_for_y(-50., height), 12.);
        assert_eq!(db_for_y(400., height), -12.);
    }

    #[test]
    fn decibels_read_with_a_sign() {
        assert_eq!(signed_db(0.), "0 dB");
        assert_eq!(signed_db(4.5), "+4.5 dB");
        assert_eq!(signed_db(-3.), "-3 dB");
    }
}
