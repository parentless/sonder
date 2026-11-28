use super::*;
use gpui_kit::TestSupportExt as _;

mod audio;

/// What the settings page asks the workspace to do.
pub(super) enum SettingsEvent {
    OpenWhatsNew,
    RequestAppChange,
    SetTheme(ThemePreference),
    SetZoom(UiZoom),
    SetAutoplay(bool),
    SetNormaliseVolume(bool),
    SetKeepInTray(bool),
    SetStreamingQuality(StreamingQuality),
    SetPlayer(crate::playback::Engine),
    SetLaunchAtLogin(bool),
    SetLyricsView(LyricsView),
    SetDiscord(crate::discord::Config),
    SetLastfm(crate::lastfm::Config),
    /// Writes the equalizer to the settings file. The sound already follows
    /// every change; this is sent when a gesture ends.
    SaveEqualizer(crate::equalizer::Settings),
    SaveEqualizerPresets(Vec<crate::equalizer::CustomPreset>),
}

/// Broad groups, so the page stays short without a tab for every setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    /// Appearance and playback.
    General,
    /// Spotify, Discord and Last.fm.
    Connections,
    /// The equalizer.
    Audio,
    /// Play a song by saying it.
    Voice,
    /// Updates and cache.
    System,
}

impl SettingsTab {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Connections,
        Self::Audio,
        Self::Voice,
        Self::System,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Connections => "Connections",
            Self::Audio => "Audio",
            Self::Voice => "Voice",
            Self::System => "System",
        }
    }
}

/// The settings page: appearance, playback, and the Spotify developer app.
pub(super) struct Settings {
    session: Entity<session::Session>,
    tab: SettingsTab,
    scroll: gpui_kit::ScrollHandle,
    /// Each switch is ringed while its focus is inside, in place of the kit's
    /// ring, which also shows after a click.
    autoplay_focus: FocusHandle,
    normalise_focus: FocusHandle,
    tray_focus: FocusHandle,
    startup_focus: FocusHandle,
    automatic_updates_focus: FocusHandle,
    automatic_install_focus: FocusHandle,
    discord_focus: FocusHandle,
    discord_application_id: Entity<InputState>,
    discord_error: Option<SharedString>,
    lastfm_focus: FocusHandle,
    lastfm_api_key: Entity<InputState>,
    lastfm_api_secret: Entity<InputState>,
    lastfm_status: Option<SharedString>,
    /// Set between asking Last.fm for a sign-in link and finishing it.
    lastfm_token: Option<String>,
    lastfm_busy: bool,
    /// Voice play lives for the whole process; this page shows and changes it.
    voice: Entity<voice_play::VoicePlay>,
    voice_focus: FocusHandle,
    voice_capture_focus: FocusHandle,
    /// Waiting for the next key press to become the shortcut.
    voice_capturing: bool,
    voice_error: Option<SharedString>,
    cache_cleared: bool,
    local_files_error: Option<SharedString>,
    /// Equalizer: the point being dragged, the graph's painted bounds (read
    /// when mapping the pointer), and the keyboard focus of each point.
    eq_drag: Option<usize>,
    eq_graph: Rc<Cell<Bounds<Pixels>>>,
    eq_focus: [FocusHandle; crate::equalizer::BAND_COUNT],
    eq_switch_focus: FocusHandle,
    eq_compare_focus: FocusHandle,
    /// Compare plays the original while the curve stays as it is. It is not
    /// saved and ends when the tab does.
    eq_bypass: bool,
    eq_presets: Vec<crate::equalizer::CustomPreset>,
    eq_naming: bool,
    eq_name: Entity<InputState>,
    eq_note: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for Settings {}

impl Settings {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let discord_id = services::AppServices::discord_config(cx)
            .application_id
            .clone();
        let discord_application_id = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Discord Application ID");
            input.set_value(discord_id, window, cx);
            input
        });
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(
            &discord_application_id,
            |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.discord_error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let enabled = services::AppServices::discord_config(cx).enabled;
                    this.save_discord(enabled, cx);
                }
                _ => {}
            },
        ));
        let lastfm_api_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Last.fm API key");
            input.set_value(
                services::AppServices::lastfm_config(cx).api_key.clone(),
                window,
                cx,
            );
            input
        });
        let lastfm_api_secret =
            cx.new(|cx| InputState::new(window, cx).placeholder("Last.fm shared secret"));
        let eq_name = cx.new(|cx| InputState::new(window, cx).placeholder("Preset name"));
        subscriptions.push(cx.subscribe(&eq_name, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save_preset(cx);
            }
        }));
        if let Some(status) = updater::status(cx) {
            subscriptions.push(cx.observe(&status, |_, _, cx| cx.notify()));
        }
        let voice = services::AppServices::voice_play(cx);
        subscriptions.push(cx.observe(&voice, |_, _, cx| cx.notify()));
        Self {
            session: services::AppServices::session(cx),
            tab: SettingsTab::General,
            scroll: gpui_kit::ScrollHandle::new(),
            autoplay_focus: cx.focus_handle(),
            normalise_focus: cx.focus_handle(),
            tray_focus: cx.focus_handle(),
            startup_focus: cx.focus_handle(),
            automatic_updates_focus: cx.focus_handle(),
            automatic_install_focus: cx.focus_handle(),
            discord_focus: cx.focus_handle(),
            lastfm_focus: cx.focus_handle(),
            lastfm_api_key,
            lastfm_api_secret,
            lastfm_status: None,
            lastfm_token: None,
            lastfm_busy: false,
            voice,
            voice_focus: cx.focus_handle(),
            voice_capture_focus: cx.focus_handle(),
            voice_capturing: false,
            voice_error: None,
            discord_application_id,
            discord_error: None,
            cache_cleared: false,
            local_files_error: None,
            eq_drag: None,
            eq_graph: Rc::new(Cell::new(Bounds::default())),
            eq_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            eq_switch_focus: cx.focus_handle(),
            eq_compare_focus: cx.focus_handle(),
            eq_bypass: false,
            eq_presets: services::AppServices::equalizer_presets(cx),
            eq_naming: false,
            eq_name,
            eq_note: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the Local Files folders, the last section of the General tab.
    pub(super) fn show_local_files(&mut self, cx: &mut Context<Self>) {
        self.end_compare(cx);
        self.tab = SettingsTab::General;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn pick_local_folders(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add to Local Files".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(folders))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.local_files_error = services::AppServices::library(cx)
                    .update(cx, |library, cx| library.add_local_folders(folders, cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Lyrics: where they appear.
    fn lyrics_section(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let preferences = services::AppServices::preferences(cx);
        let view = preferences.lyrics_view;
        let card = div()
            .mt_4()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface_raised))
            .p_5()
            .child(
                div().child(Self::playback_setting_row(
                    palette,
                    "Lyrics view",
                    "A panel beside the page, the whole page, or one line above the player.",
                    div().flex().flex_wrap().gap_2().children(
                        LyricsView::ALL
                            .into_iter()
                            .enumerate()
                            .map(|(index, option)| {
                                components::pill(
                                    palette,
                                    ("settings-lyrics-view", index),
                                    option.label(),
                                    option == view,
                                )
                                .test_support()
                                .h_9()
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.emit(SettingsEvent::SetLyricsView(option));
                                    },
                                ))
                            }),
                    ),
                )),
            );
        div()
            .mt_12()
            .child(Self::settings_section_header(
                palette,
                "Lyrics",
                "Follow the words as a song plays. Lyrics come from LRCLIB, so the song’s title, artist and length are sent there when lyrics are open.",
            ))
            .child(card)
    }

    /// Saves a change to Voice play's settings, or says why it cannot be made.
    fn save_voice(
        &mut self,
        change: impl FnOnce(&mut crate::voice::Config),
        cx: &mut Context<Self>,
    ) {
        let mut config = self.voice.read(cx).config().clone();
        change(&mut config);
        self.voice_error = services::AppServices::set_voice_config(config, cx)
            .err()
            .map(|error| format!("{error:#}").into());
        cx.notify();
    }

    /// The key press that is being waited for becomes the shortcut.
    fn capture_shortcut(&mut self, event: &gpui_kit::KeyDownEvent, cx: &mut Context<Self>) {
        if !self.voice_capturing {
            return;
        }
        let key = event.keystroke.key.as_str();
        cx.stop_propagation();
        match key {
            "escape" => {
                self.voice_capturing = false;
                self.voice_error = None;
            }
            // Holding a modifier first is how a shortcut starts.
            "control" | "shift" | "alt" | "platform" | "function" | "meta" | "super" | "win" => {}
            _ => {
                let held = &event.keystroke.modifiers;
                match crate::voice::Hotkey::from_keys(
                    held.control,
                    held.alt,
                    held.shift,
                    held.platform,
                    key,
                ) {
                    Some(hotkey) => {
                        self.voice_capturing = false;
                        self.save_voice(|config| config.hotkey = hotkey, cx);
                    }
                    None => {
                        self.voice_error = Some(
                            "That shortcut can't be used. Hold Ctrl, Alt or Windows with a key, or use an F key on its own."
                                .into(),
                        );
                    }
                }
            }
        }
        cx.notify();
    }

    fn voice_model_row(
        &self,
        palette: CadencePalette,
        model: crate::voice::Model,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::voice::model::megabytes;
        let voice = self.voice.read(cx);
        let installed = voice.installed(model);
        let downloading = voice
            .download()
            .filter(|download| download.model == model)
            .map(|download| download.received);
        let busy = voice.download().is_some();
        let key = crate::voice::Model::ALL
            .iter()
            .position(|candidate| *candidate == model)
            .unwrap_or(0);
        let action = if downloading.is_some() {
            components::settings_button(palette, ("settings-voice-cancel", key), "Cancel")
                .test_support()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.voice.update(cx, |voice, cx| voice.cancel_download(cx));
                }))
                .into_any_element()
        } else if installed {
            components::settings_button(palette, ("settings-voice-remove", key), "Remove")
                .test_support()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let selected_and_on = {
                        let config = this.voice.read(cx).config();
                        config.model == model && config.enabled
                    };
                    if selected_and_on {
                        this.save_voice(|config| config.enabled = false, cx);
                    }
                    this.voice
                        .update(cx, |voice, cx| voice.remove_model(model, cx));
                }))
                .into_any_element()
        } else {
            components::settings_button(palette, ("settings-voice-download", key), "Download")
                .test_support()
                .when(busy, |button| button.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.voice
                        .update(cx, |voice, cx| voice.start_download(model, cx));
                }))
                .into_any_element()
        };
        let detail = match downloading {
            Some(received) => format!(
                "Downloading {} of {}",
                megabytes(received),
                model.size_label()
            ),
            None if installed => format!("{} · Downloaded", model.size_label()),
            None => format!("{} · {}", model.size_label(), model.detail()),
        };
        let fraction = downloading.map(|received| received as f32 / model.bytes() as f32);
        h_flex()
            .w_full()
            .items_center()
            .gap_3()
            .child(
                components::pill(
                    palette,
                    ("settings-voice-model", key),
                    model.label(),
                    selected,
                )
                .test_support()
                .h_9()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.save_voice(|config| config.model = model, cx);
                })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(tokens::BODY_TEXT)
                            .text_color(rgb(palette.text_muted))
                            .child(if installed && selected {
                                format!("{detail} · In use")
                            } else {
                                detail
                            }),
                    )
                    .when_some(fraction, |column, fraction| {
                        column.child(
                            div()
                                .h(px(4.))
                                .w_full()
                                .rounded_full()
                                .bg(rgb(palette.control))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(fraction.clamp(0., 1.)))
                                        .rounded_full()
                                        .bg(rgb(palette.text_primary)),
                                ),
                        )
                    }),
            )
            .child(div().flex_none().child(action))
    }

    /// Hold a shortcut, say a song, and it plays: the setting, its shortcut
    /// and the speech model it needs.
    fn voice_section(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::voice::Hotkey;
        let voice = self.voice.read(cx);
        let config = voice.config().clone();
        let phase = voice.phase().clone();
        let status = match &phase {
            voice_play::Phase::Idle => voice.last().cloned(),
            voice_play::Phase::Listening => Some("Listening…".into()),
            voice_play::Phase::Working(text) => Some(format!("Working on “{text}”…").into()),
        };
        let supported = crate::voice::hook::SUPPORTED;
        let download_error = voice.download_error().cloned();
        let capturing = self.voice_capturing;
        let shortcut = if capturing {
            "Press the new shortcut…".to_owned()
        } else {
            config.hotkey.label()
        };
        let card = div()
            .mt_4()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface_raised))
            .p_5()
            .child(Self::playback_setting_row(
                palette,
                "Play by voice",
                "Hold the shortcut, say a song and its artist, then let go. Sonder searches Spotify and plays it, even while another app or a game is in front.",
                components::focus_ring_around(
                    palette,
                    &self.voice_focus,
                    window,
                    cx,
                    Switch::new("settings-voice-enabled")
                        .cursor_pointer()
                        .focus_ring(false)
                        .checked(config.enabled)
                        .on_click(cx.listener(|this, enabled: &bool, _, cx| {
                            let enabled = *enabled;
                            this.save_voice(|config| config.enabled = enabled, cx);
                        })),
                ),
            ))
            .child(
                div()
                    .mt_5()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(Self::playback_setting_row(
                        palette,
                        "Shortcut",
                        "Hold it while you speak. Hold Ctrl, Alt or Windows with a key, or use an F key on its own.",
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("settings-voice-shortcut-field")
                                    .test_support()
                                    .track_focus(&self.voice_capture_focus)
                                    .on_key_down(cx.listener(
                                        |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                                            this.capture_shortcut(event, cx)
                                        },
                                    ))
                                    // Escape is an action elsewhere; while waiting for a
                                    // shortcut it only means "never mind".
                                    .on_action(cx.listener(|this, _: &DismissOverlay, _, cx| {
                                        if this.voice_capturing {
                                            this.voice_capturing = false;
                                            this.voice_error = None;
                                            cx.notify();
                                        } else {
                                            cx.propagate();
                                        }
                                    }))
                                    .h_10()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .rounded(tokens::CONTROL_RADIUS)
                                    .bg(rgb(palette.control))
                                    .border_1()
                                    .border_color(rgb(if capturing {
                                        palette.text_primary
                                    } else {
                                        palette.border
                                    }))
                                    .text_size(tokens::BODY_TEXT)
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette.text_primary))
                                    .child(shortcut),
                            )
                            .child(
                                components::settings_button(
                                    palette,
                                    "settings-voice-shortcut",
                                    if capturing { "Cancel" } else { "Change" },
                                )
                                .test_support()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.voice_capturing = !this.voice_capturing;
                                    this.voice_error = None;
                                    if this.voice_capturing {
                                        window.focus(&this.voice_capture_focus, cx);
                                    }
                                    cx.notify();
                                })),
                            )
                            .when(config.hotkey != Hotkey::default() && !capturing, |row| {
                                row.child(
                                    components::settings_button(
                                        palette,
                                        "settings-voice-shortcut-reset",
                                        "Reset",
                                    )
                                    .test_support()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_voice(
                                            |config| config.hotkey = Hotkey::default(),
                                            cx,
                                        );
                                    })),
                                )
                            }),
                    )),
            )
            .child(
                div()
                    .mt_5()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(rgb(palette.text))
                            .child("Speech model"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(tokens::BODY_TEXT)
                            .line_height(relative(1.45))
                            .text_color(rgb(palette.text_muted))
                            .child("This is what turns your voice into words. It runs entirely on this computer, and only while you hold the shortcut: it loads as you start speaking and is gone from memory as soon as it has your words. Your voice is never sent anywhere or saved. It understands English and is downloaded once from its publisher, whisper.cpp on Hugging Face."),
                    )
                    .child(
                        v_flex().mt_4().gap_3().children(
                            crate::voice::Model::ALL
                                .into_iter()
                                .map(|model| {
                                    self.voice_model_row(palette, model, config.model == model, cx)
                                }),
                        ),
                    ),
            )
            .child(
                div()
                    .mt_5()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .id("settings-voice-status")
                            .test_support()
                            .text_size(tokens::BODY_TEXT)
                            .text_color(rgb(palette.text))
                            .child(status.unwrap_or_else(|| "Nothing played by voice yet.".into())),
                    )
                    .when_some(
                        self.voice_error.clone().or(download_error),
                        |card, error| {
                            card.child(
                                div()
                                    .id("settings-voice-error")
                                    .test_support()
                                    .mt_2()
                                    .text_size(tokens::BODY_TEXT)
                                    .text_color(rgb(palette.text))
                                    .child(error),
                            )
                        },
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(tokens::CAPTION_TEXT)
                            .text_color(rgb(palette.text_muted))
                            .child(if supported {
                                "Windows does not let Sonder hear the shortcut while a window running as administrator has focus. Start Sonder as administrator too if a game does."
                            } else {
                                "Voice play works on Windows only for now."
                            }),
                    ),
            );
        div()
            .mt_12()
            .child(Self::settings_section_header(
                palette,
                "Voice play",
                "Start a song by saying its name.",
            ))
            .child(card)
    }

    /// The folders Local Files reads, with a way to add and remove them.
    fn local_files_section(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let library = services::AppServices::library(cx);
        let (folders, status) = {
            let library = library.read(cx);
            let status = if library.local_files_scanning() {
                "Looking for music…".to_owned()
            } else {
                let tracks = library.local_tracks().len();
                format!("{tracks} {}", if tracks == 1 { "track" } else { "tracks" })
            };
            (library.local_folders().clone(), status)
        };
        let add = components::settings_button(palette, "settings-local-files-add", "Add folder")
            .test_support()
            .on_click(cx.listener(|this, _, _, cx| this.pick_local_folders(cx)));
        let card = div()
            .mt_4()
            .rounded_2xl()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface_raised))
            .p_5();
        let card = if folders.is_empty() {
            card.child(Self::playback_setting_row(
                palette,
                "Folders",
                "Music in the folders you add appears under Local Files. Sonder plays MP3, FLAC, Ogg, M4A and WAV files.",
                add,
            ))
        } else {
            card.children(folders.iter().enumerate().map(|(index, folder)| {
                let removed = folder.clone();
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .when(index > 0, |row| {
                        row.mt_5()
                            .pt_5()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                    })
                    .child(components::icon(
                        CadenceIcon::Folder,
                        tokens::CONTROL_ICON,
                        palette.text_muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(tokens::BODY_TEXT)
                            .text_color(rgb(palette.text))
                            .child(folder.display().to_string()),
                    )
                    .child(
                        components::icon_button_sized(
                            palette,
                            ("settings-local-files-remove", index),
                            CadenceIcon::Close,
                            tokens::MENU_ICON,
                        )
                        .test_support()
                        .size_8()
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Remove folder")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.local_files_error = services::AppServices::library(cx)
                                .update(cx, |library, cx| {
                                    library.remove_local_folder(&removed, cx)
                                });
                            cx.notify();
                        })),
                    )
            }))
            .child(
                h_flex()
                    .mt_5()
                    .pt_5()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(tokens::BODY_TEXT)
                            .text_color(rgb(palette.text_muted))
                            .child(status),
                    )
                    .child(add),
            )
        };
        div()
            .mt_12()
            .child(Self::settings_section_header(
                palette,
                "Local Files",
                "Play music saved on this computer.",
            ))
            .child(card)
            .when_some(self.local_files_error.clone(), |section, error| {
                section.child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(rgb(palette.text))
                        .child(error),
                )
            })
    }

    #[cfg(test)]
    pub(super) fn autoplay_focus(&self) -> &FocusHandle {
        &self.autoplay_focus
    }

    fn page(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let session = self.session.read(cx);
        let saved_configuration = session.client_id_source() == Some(ClientIdSource::Saved);
        let environment_configuration =
            session.client_id_source() == Some(ClientIdSource::Environment);
        let client_id = session.client_id().cloned().unwrap_or_default();
        let dashboard_url = format!("{}/{client_id}", onboarding::SPOTIFY_DASHBOARD_URL);
        let preferences = services::AppServices::preferences(cx);
        let autoplay = preferences.autoplay;
        let normalise_volume = preferences.normalise_volume;
        let keep_in_tray = preferences.keep_in_tray;
        let launch_at_login = preferences.launch_at_login;
        let streaming_quality = preferences.streaming_quality;
        let player = preferences.player;

        let tab = self.tab;
        // The tab list stays put while the selected group scrolls beside it.
        div()
            .size_full()
            .flex()
            .justify_center()
            .child(self.tab_list(palette, cx))
            .child(
        div()
            .id("settings-page").test_support()
            .h_full()
            .flex_1()
            .min_w_0()
            .max_w(tokens::SETTINGS_MAX_WIDTH)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .pr_10()
                    .pt_7()
                    .pb_16()
                    .when(tab == SettingsTab::General, |content| content
                    .child(
                        div()
                            .child(Self::settings_section_header(
                                palette,
                                "Appearance",
                                "Choose how Sonder looks.",
                            ))
                            .child(
                                div()
                                    .mt_4()
                                    .p_1()
                                    .rounded(tokens::CONTAINER_RADIUS)
                                    .bg(rgb(palette.control))
                                    .flex()
                                    .gap_1()
                                    .child(self.appearance_option(
                                        "settings-appearance-system",
                                        CadenceIcon::SystemAppearance,
                                        "System",
                                        ThemePreference::System,
                                        cx,
                                    ))
                                    .child(self.appearance_option(
                                        "settings-appearance-light",
                                        CadenceIcon::Sun,
                                        "Light",
                                        ThemePreference::Light,
                                        cx,
                                    ))
                                    .child(self.appearance_option(
                                        "settings-appearance-dark",
                                        CadenceIcon::Moon,
                                        "Dark",
                                        ThemePreference::Dark,
                                        cx,
                                    )),
                            ),
                    )
                    .child(div().mt_5().child(components::section_label(palette, "UI zoom"))
                        .child(Self::zoom_scale(palette, preferences.ui_zoom, cx))
                    )
                    .child(
                        div().mt_5().child(components::section_label(palette, "Selection color"))
                            .child(div().mt_2().flex().flex_wrap().gap_2().children(
                                SelectionColor::ALL.into_iter().enumerate().map(|(index, color)| {
                                    components::button(palette, ("settings-selection-color", index))
                                        .hover(|style| style.opacity(0.9))
                                        .test_support().h_9().px_3().rounded_lg()
                                        .bg(rgb(color.fill(palette.canvas == CadencePalette::DARK.canvas)))
                                        .border_1().border_color(rgb(if preferences.selection_color == color { palette.text_primary } else { palette.border }))
                                        .child(color.name())
                                        .on_click(move |_, window, cx| appearance::Appearance::set_selection_color(color, window, cx))
                                })
                            ))
                    )
                    .child(
                        div()
                            .mt_12()
                            .child(Self::settings_section_header(
                                palette,
                                "Playback",
                                "Control how playback behaves.",
                            ))
                            .child(
                                div()
                                    .mt_4()
                                    .rounded_2xl()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.surface_raised))
                                    .p_5()
                                    .child(Self::playback_setting_row(
                                        palette,
                                        "Autoplay",
                                        "Keep the music going with similar songs when the queue ends.",
                                        components::focus_ring_around(
                                            palette,
                                            &self.autoplay_focus,
                                            window,
                                            cx,
                                            Switch::new("settings-autoplay")
.cursor_pointer()
                                                .focus_ring(false)
                                                .checked(autoplay)
                                                .on_click(cx.listener(
                                                    |_, checked: &bool, _, cx| {
                                                        cx.emit(SettingsEvent::SetAutoplay(
                                                            *checked,
                                                        ));
                                                    },
                                                )),
                                        ),
                                    ))
                                    .child(
                                        div()
                                            .mt_5()
                                            .pt_5()
                                            .border_t_1()
                                            .border_color(rgb(palette.border))
                                            .child(Self::playback_setting_row(
                                                palette,
                                                "Streaming quality",
                                                "Higher quality uses more data. Takes effect the next time playback connects, such as after restarting Sonder.",
                                                div().flex().flex_wrap().gap_2().children(
                                                    StreamingQuality::ALL.into_iter().map(|quality| {
                                                        components::pill(
                                                            palette,
                                                            ("settings-streaming-quality", usize::from(quality.kbps())),
                                                            quality.label(),
                                                            quality == streaming_quality,
                                                        )
                                                        .test_support()
                                                        .h_9()
                                                        .on_click(cx.listener(move |_, _, _, cx| {
                                                            cx.emit(SettingsEvent::SetStreamingQuality(quality));
                                                        }))
                                                    }),
                                                ),
                                            )),
                                    )
                                    .when(crate::soloist::supported() || player == crate::playback::Engine::Soloist, |card| {
                                        card.child(
                                            div()
                                                .mt_5()
                                                .pt_5()
                                                .border_t_1()
                                                .border_color(rgb(palette.border))
                                                .child(Self::playback_setting_row(
                                                    palette,
                                                    "Player",
                                                    "Spotify’s own player may play on accounts the built-in one can’t. It ignores streaming quality and volume normalization. Local files still play in Sonder.",
                                                    div().flex().flex_wrap().gap_2().children(
                                                        [
                                                            (crate::playback::Engine::BuiltIn, "Built-in"),
                                                            (crate::playback::Engine::Soloist, "Spotify’s player"),
                                                        ]
                                                        .into_iter()
                                                        .enumerate()
                                                        .map(|(index, (engine, label))| {
                                                            components::pill(
                                                                palette,
                                                                ("settings-player", index),
                                                                label,
                                                                engine == player,
                                                            )
                                                            .test_support()
                                                            .h_9()
                                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                                cx.emit(SettingsEvent::SetPlayer(engine));
                                                            }))
                                                        }),
                                                    ),
                                                )),
                                        )
                                    })
                                    .child(
                                        div()
                                            .mt_5()
                                            .pt_5()
                                            .border_t_1()
                                            .border_color(rgb(palette.border))
                                            .child(Self::playback_setting_row(
                                                palette,
                                                "Normalize volume",
                                                "Keep songs at a similar loudness. Takes effect the next time playback connects, such as after restarting Sonder.",
                                                components::focus_ring_around(
                                                    palette,
                                                    &self.normalise_focus,
                                                    window,
                                                    cx,
                                                    Switch::new("settings-normalise-volume")
.cursor_pointer()
                                                        .focus_ring(false)
                                                        .checked(normalise_volume)
                                                        .on_click(cx.listener(
                                                            |_, checked: &bool, _, cx| {
                                                                cx.emit(
                                                                    SettingsEvent::SetNormaliseVolume(
                                                                        *checked,
                                                                    ),
                                                                );
                                                            },
                                                        )),
                                                ),
                                            )),
                                    )
                                    .when(cfg!(target_os = "windows"), |card| {
                                        card.child(
                                            div()
                                                .mt_5()
                                                .pt_5()
                                                .border_t_1()
                                                .border_color(rgb(palette.border))
                                                .child(Self::playback_setting_row(
                                                    palette,
                                                    "Keep running in the tray",
                                                    "Closing the window keeps the music playing. Click the tray icon to bring Sonder back.",
                                                    components::focus_ring_around(
                                                        palette,
                                                        &self.tray_focus,
                                                        window,
                                                        cx,
                                                        Switch::new("settings-keep-in-tray")
.cursor_pointer()
                                                            .focus_ring(false)
                                                            .checked(keep_in_tray)
                                                            .on_click(cx.listener(
                                                                |_, checked: &bool, _, cx| {
                                                                    cx.emit(
                                                                        SettingsEvent::SetKeepInTray(
                                                                            *checked,
                                                                        ),
                                                                    );
                                                                },
                                                            )),
                                                    ),
                                                ))
                                                .child(
                                                    div()
                                                        .mt_5()
                                                        .pt_5()
                                                        .border_t_1()
                                                        .border_color(rgb(palette.border))
                                                        .child(Self::playback_setting_row(
                                                            palette,
                                                            "Open on startup",
                                                            "Start Sonder when you sign in to Windows. With the tray option on, it starts hidden in the tray.",
                                                            components::focus_ring_around(
                                                                palette,
                                                                &self.startup_focus,
                                                                window,
                                                                cx,
                                                                Switch::new("settings-open-on-startup")
.cursor_pointer()
                                                                    .focus_ring(false)
                                                                    .checked(launch_at_login)
                                                                    .on_click(cx.listener(
                                                                        |_, checked: &bool, _, cx| {
                                                                            cx.emit(
                                                                                SettingsEvent::SetLaunchAtLogin(
                                                                                    *checked,
                                                                                ),
                                                                            );
                                                                        },
                                                                    )),
                                                            ),
                                                        )),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(self.local_files_section(palette, cx))
                    .child(self.lyrics_section(palette, cx))
                    )
                    .when(tab == SettingsTab::Audio, |content| {
                        content.child(self.audio_tab(palette, window, cx))
                    })
                    .when(tab == SettingsTab::Voice, |content| {
                        content.child(self.voice_section(palette, window, cx).mt_0())
                    })
                    .when(tab == SettingsTab::Connections, |content| content
                    .child(
                        div()
                            .child(Self::settings_section_header(
                                palette,
                                "Spotify",
                                "Manage the developer app Sonder uses to connect to Spotify.",
                            ))
                            .child(
                                div()
                                    .mt_4()
                                    .rounded_2xl()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.surface_raised))
                                    .p_5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_5()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(
                                                                gpui_kit::FontWeight::MEDIUM,
                                                            )
                                                            .text_color(rgb(palette.text))
                                                            .child(if environment_configuration {
                                                                "Configured by SPOTIFY_CLIENT_ID"
                                                            } else {
                                                                "Spotify developer app"
                                                            }),
                                                    )
                                                    .child(
                                                        components::flush_button(palette,
                                                            "settings-open-spotify-dashboard",
                                                        )
                                                            .mt_1p5()
                                                            .rounded_md()
                                                            .text_sm()
                                                            .text_color(rgb(palette.link))
                                                            .underline()
                                                            .gap(tokens::INLINE_ICON_GAP)
                                                            .justify_start()
                                                            .child(client_id)
                                                            .child(components::icon(
                                                                CadenceIcon::ArrowUpRight,
                                                                tokens::INLINE_ICON,
                                                                palette.link,
                                                            ))
                                                            .on_click(cx.listener(
                                                                move |_, _, _, cx| {
                                                                    cx.open_url(&dashboard_url);
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_none()
                                                    .gap_2()
                                                    .when(
                                                        saved_configuration,
                                                        |actions| {
                                                            actions.child(
                                                                components::settings_button(palette,
                                                                    "settings-change-spotify-app",
                                                                    "Change developer app",
                                                                )
                                                                .on_click(cx.listener(
                                                                    |_, _, _, cx| {
                                                                        cx.emit(
                                                                            SettingsEvent::RequestAppChange,
                                                                        );
                                                                    },
                                                                )),
                                                            )
                                                        },
                                                    ),
                                            ),
                                    )
                                    .when(environment_configuration, |card| {
                                        card.child(
                                            div()
                                                .mt_3()
                                                .text_size(tokens::BODY_TEXT)
                                                .line_height(relative(1.45))
                                                .text_color(rgb(palette.text))
                                                .child(
                                                    "Quit Sonder and update the environment variable to change the developer app.",
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(self.discord_section(palette, window, cx))
                    .child(self.lastfm_section(palette, window, cx))
                    )
                    .when(tab == SettingsTab::System, |content| content
                    .child(self.updates_section(palette, window, cx).mt_0())
                    .child(self.cache_section(palette, cx))),
            )
            )
    }

    fn tab_list(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .w(tokens::SETTINGS_NAV_WIDTH)
            .mr_6()
            .pl_10()
            .pt_7()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                SettingsTab::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let selected = self.tab == tab;
                        components::filled_button(palette, ("settings-tab", index))
                            .test_support()
                            .h_10()
                            .w_full()
                            .px_3()
                            .justify_start()
                            .rounded(tokens::CONTROL_RADIUS)
                            .text_sm()
                            .font_weight(if selected {
                                gpui_kit::FontWeight::SEMIBOLD
                            } else {
                                gpui_kit::FontWeight::MEDIUM
                            })
                            .text_color(rgb(palette.text_primary))
                            .when(selected, |button| button.bg(rgb(palette.selection)))
                            .hover(|style| style.bg(rgb(palette.control_hover)))
                            .child(tab.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.tab != tab {
                                    this.end_compare(cx);
                                    this.eq_naming = false;
                                    this.tab = tab;
                                    // Each group starts at its top.
                                    this.scroll.set_offset(gpui_kit::point(
                                        gpui_kit::px(0.),
                                        gpui_kit::px(0.),
                                    ));
                                    cx.notify();
                                }
                            }))
                    }),
            )
    }

    fn save_discord(&mut self, enabled: bool, cx: &mut Context<Self>) {
        // Turning sharing off must always work, even if the field contains
        // an unfinished edit. Keep the saved ID when disabling the toggle.
        let saved = services::AppServices::discord_config(cx);
        let application_id = if !enabled && saved.enabled {
            saved.application_id.clone()
        } else {
            self.discord_application_id
                .read(cx)
                .value()
                .trim()
                .to_owned()
        };
        if (enabled || !application_id.is_empty())
            && !crate::discord::valid_application_id(&application_id)
        {
            self.discord_error = Some("Enter the numeric Application ID from Discord's Developer Portal, not a bot token.".into());
            cx.notify();
            return;
        }
        self.discord_error = None;
        cx.emit(SettingsEvent::SetDiscord(crate::discord::Config {
            enabled,
            application_id,
        }));
        cx.notify();
    }

    fn set_lastfm_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let mut config = services::AppServices::lastfm_config(cx).clone();
        if enabled && !config.connected() {
            self.lastfm_status = Some("Connect your Last.fm account first.".into());
            cx.notify();
            return;
        }
        config.enabled = enabled;
        self.lastfm_status = None;
        cx.emit(SettingsEvent::SetLastfm(config));
        cx.notify();
    }

    /// Two steps: ask Last.fm for a sign-in page, then finish once approved.
    fn lastfm_connect(&mut self, cx: &mut Context<Self>) {
        if self.lastfm_busy {
            return;
        }
        let api_key = self.lastfm_api_key.read(cx).value().trim().to_owned();
        let api_secret = self.lastfm_api_secret.read(cx).value().trim().to_owned();
        self.lastfm_busy = true;
        self.lastfm_status = Some("Contacting Last.fm…".into());
        cx.notify();
        if let Some(token) = self.lastfm_token.clone() {
            let reply = crate::lastfm::finish_auth(api_key.clone(), api_secret, token);
            cx.spawn(async move |this, cx| {
                let result = reply.await;
                let _ = this.update(cx, |this, cx| {
                    this.lastfm_busy = false;
                    match result {
                        Ok(Ok(username)) => {
                            this.lastfm_token = None;
                            this.lastfm_status = None;
                            cx.emit(SettingsEvent::SetLastfm(crate::lastfm::Config {
                                enabled: true,
                                api_key,
                                username,
                            }));
                        }
                        Ok(Err(error)) => this.lastfm_status = Some(format!("{error:#}").into()),
                        Err(_) => {
                            this.lastfm_status = Some("Last.fm sign-in did not finish.".into())
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            let reply = crate::lastfm::begin_auth(api_key, api_secret);
            cx.spawn(async move |this, cx| {
                let result = reply.await;
                let _ = this.update(cx, |this, cx| {
                    this.lastfm_busy = false;
                    match result {
                        Ok(Ok(start)) => {
                            this.lastfm_token = Some(start.token);
                            this.lastfm_status = Some(
                                "Approve Sonder in your browser, then press Finish connecting."
                                    .into(),
                            );
                            cx.open_url(&start.url);
                        }
                        Ok(Err(error)) => this.lastfm_status = Some(format!("{error:#}").into()),
                        Err(_) => {
                            this.lastfm_status = Some("Last.fm sign-in did not finish.".into())
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn lastfm_disconnect(&mut self, cx: &mut Context<Self>) {
        if self.lastfm_busy {
            return;
        }
        self.lastfm_busy = true;
        let api_key = services::AppServices::lastfm_config(cx).api_key.clone();
        let reply = crate::lastfm::forget_credentials();
        cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.lastfm_busy = false;
                match result {
                    Ok(Ok(())) => {
                        this.lastfm_status = None;
                        cx.emit(SettingsEvent::SetLastfm(crate::lastfm::Config {
                            enabled: false,
                            api_key,
                            username: String::new(),
                        }));
                    }
                    Ok(Err(error)) => this.lastfm_status = Some(format!("{error:#}").into()),
                    Err(_) => {
                        this.lastfm_status = Some("Could not remove the Last.fm sign-in.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn lastfm_section(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let config = services::AppServices::lastfm_config(cx).clone();
        let connect_label = if self.lastfm_token.is_some() {
            "Finish connecting"
        } else {
            "Connect"
        };
        div().mt_12()
            .child(Self::settings_section_header(palette, "Last.fm", "Optionally scrobble what you listen to."))
            .child(div().mt_4().rounded_2xl().border_1().border_color(rgb(palette.border))
                .bg(rgb(palette.surface_raised)).p_5()
                .child(Self::playback_setting_row(palette, "Scrobble to Last.fm",
                    if config.connected() { "Plays after half the track or four minutes. Retries when offline." } else { "Connect an account below to turn this on." },
                    components::focus_ring_around(palette, &self.lastfm_focus, window, cx,
                        Switch::new("settings-lastfm-enabled")
.cursor_pointer().focus_ring(false).checked(config.enabled)
                            .on_click(cx.listener(|this, enabled: &bool, _, cx| this.set_lastfm_enabled(*enabled, cx))))))
                .child(if config.connected() {
                    div().mt_5().pt_5().border_t_1().border_color(rgb(palette.border))
                        .child(div().text_xs().text_color(rgb(palette.text)).child(format!("Connected as {}", config.username)))
                        .child(components::settings_button(palette, "settings-lastfm-disconnect", "Disconnect")
                            .mt_3().on_click(cx.listener(|this, _, _, cx| this.lastfm_disconnect(cx))))
                } else {
                    div().mt_5().pt_5().border_t_1().border_color(rgb(palette.border))
                        .child(div().text_xs().text_color(rgb(palette.text)).child("API key and shared secret"))
                        .child(div().mt_2().child(Input::new(&self.lastfm_api_key).id("settings-lastfm-api-key").w_full()))
                        .child(div().mt_2().child(Input::new(&self.lastfm_api_secret).id("settings-lastfm-api-secret").w_full()))
                        .child(div().mt_2().text_size(tokens::CAPTION_TEXT).text_color(rgb(palette.text_muted))
                            .child("Create a free Last.fm API account, then paste its key and shared secret. The secret stays in your system keychain."))
                        .child(h_flex().mt_3().gap_3()
                            .child(components::settings_button(palette, "settings-lastfm-connect", connect_label)
                                .test_support()
                                .on_click(cx.listener(|this, _, _, cx| this.lastfm_connect(cx))))
                            .child(components::settings_button(palette, "settings-lastfm-portal", "Create API account")
                                .on_click(|_, _, cx| cx.open_url("https://www.last.fm/api/account/create"))))
                })
                .when_some(self.lastfm_status.clone(), |card, status| card.child(div().mt_2().text_sm().text_color(rgb(palette.text)).child(status))))
    }

    fn discord_section(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = services::AppServices::discord_config(cx).enabled;
        div().mt_12()
            .child(Self::settings_section_header(palette, "Discord", "Optionally show what you're listening to on your Discord profile."))
            .child(div().mt_4().rounded_2xl().border_1().border_color(rgb(palette.border))
                .bg(rgb(palette.surface_raised)).p_5()
                .child(Self::playback_setting_row(palette, "Share listening activity", "Requires Discord desktop. Activity clears while paused or signed out.",
                    components::focus_ring_around(palette, &self.discord_focus, window, cx,
                        Switch::new("settings-discord-enabled")
.cursor_pointer().focus_ring(false).checked(enabled)
                            .on_click(cx.listener(|this, enabled: &bool, _, cx| this.save_discord(*enabled, cx))))))
                .child(div().mt_5().pt_5().border_t_1().border_color(rgb(palette.border))
                    .child(div().text_xs().text_color(rgb(palette.text)).child("Discord Application ID"))
                    .child(div().mt_2().flex().gap_3()
                        .child(Input::new(&self.discord_application_id).id("settings-discord-application-id").flex_1().min_w_0())
                        .child(components::settings_button(palette, "settings-discord-save", "Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let enabled = services::AppServices::discord_config(cx).enabled;
                                this.save_discord(enabled, cx);
                            }))))
                    .child(div().mt_2().text_size(tokens::CAPTION_TEXT).text_color(rgb(palette.text_muted))
                        .child("Create a Discord application named Sonder and paste its public Application ID. No bot token needed."))
                    .child(components::settings_button(palette, "settings-discord-portal", "Open Discord Developer Portal")
                        .mt_3().on_click(|_, _, cx| cx.open_url("https://discord.com/developers/applications")))
                    .when_some(self.discord_error.clone(), |card, error| card.child(div().mt_2().text_sm().text_color(rgb(palette.text)).child(error)))))
    }

    fn updates_section(
        &self,
        palette: CadencePalette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let check = updater::status(cx).map(|status| status.read(cx).check.clone());
        div()
            .mt_12()
            .child(Self::settings_section_header(
                palette,
                "Updates",
                "New versions install in place from GitHub releases.",
            ))
            .child(
                div()
                    .mt_4()
                    .rounded_2xl()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface_raised))
                    .p_5()
                    .child(Self::version_row(palette, check.as_ref()))
                    .when_some(updater::automatic_install(cx), |card, enabled| {
                        card.child(
                            div()
                                .mt_5()
                                .pt_5()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .child(Self::playback_setting_row(
                                    palette,
                                    "Install updates automatically",
                                    "Downloads new versions quietly in the background and installs them when you close Sonder. Never interrupts playback.",
                                    components::focus_ring_around(
                                        palette,
                                        &self.automatic_install_focus,
                                        window,
                                        cx,
                                        Switch::new("settings-automatic-install")
.cursor_pointer()
                                            .focus_ring(false)
                                            .checked(enabled)
                                            .on_click(cx.listener(|_, checked: &bool, _, cx| {
                                                updater::set_automatic_install(*checked, cx);
                                                cx.notify();
                                            })),
                                    ),
                                )),
                        )
                    })
                    .child(
                        div()
                            .mt_5()
                            .pt_5()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .child(Self::playback_setting_row(
                                palette,
                                "What’s new",
                                "See what changed in each version.",
                                components::settings_button(palette, "settings-whats-new", "Open")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SettingsEvent::OpenWhatsNew)
                                    })),
                            )),
                    )
                    .when_some(updater::automatic_checks(cx), |card, enabled| {
                        card.child(
                            div()
                                .mt_5()
                                .pt_5()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .child(Self::playback_setting_row(
                                    palette,
                                    "Check for updates automatically",
                                    "Once a day. You choose when to install.",
                                    components::focus_ring_around(
                                        palette,
                                        &self.automatic_updates_focus,
                                        window,
                                        cx,
                                        Switch::new("settings-automatic-updates")
.cursor_pointer()
                                            .focus_ring(false)
                                            .checked(enabled)
                                            .on_click(cx.listener(|_, checked: &bool, _, cx| {
                                                updater::set_automatic_checks(*checked, cx);
                                                cx.notify();
                                            })),
                                    ),
                                )),
                        )
                    }),
            )
    }

    /// The running version with the outcome of Sparkle's last check, and a
    /// button that opens Sparkle's window. `None` means this build never
    /// checks, so there is nothing to press.
    fn version_row(palette: CadencePalette, check: Option<&updater::UpdateCheck>) -> Div {
        use updater::UpdateCheck;
        let (detail, emphasized) = match check {
            None => ("This build does not update itself.".to_owned(), false),
            Some(UpdateCheck::Unknown) => ("Not checked yet.".to_owned(), false),
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Checking) => ("Checking for updates…".to_owned(), false),
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Downloading) => (
                "Downloading and verifying the update. Sonder will restart when ready.".to_owned(),
                false,
            ),
            Some(UpdateCheck::UpToDate) => ("Up to date.".to_owned(), false),
            Some(UpdateCheck::Available(version)) => {
                (format!("Sonder {version} is ready to install."), true)
            }
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Ready(version)) => (
                format!(
                    "Sonder {version} is downloaded. It installs when you restart or close Sonder."
                ),
                true,
            ),
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Failed(error)) => (format!("Update failed: {error}"), false),
            Some(UpdateCheck::Skipped(version)) => (format!("Version {version} skipped."), false),
        };
        #[cfg(target_os = "windows")]
        let busy = matches!(
            check,
            Some(UpdateCheck::Checking | UpdateCheck::Downloading)
        );
        #[cfg(not(target_os = "windows"))]
        let busy = false;
        let action = match check {
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Checking | UpdateCheck::Downloading) => "Please wait…",
            Some(UpdateCheck::Available(_)) => "Install update",
            #[cfg(target_os = "windows")]
            Some(UpdateCheck::Ready(_)) => "Restart to update",
            _ => "Check for updates",
        };
        h_flex()
            .w_full()
            .justify_between()
            .items_start()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    // Let the description wrap instead of pushing the control out of its card.
                    .min_w_0()
                    .max_w(relative(0.6))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(rgb(palette.text))
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .text_size(tokens::BODY_TEXT)
                            .line_height(relative(1.45))
                            .text_color(rgb(if emphasized {
                                palette.text_primary
                            } else {
                                palette.text_muted
                            }))
                            .child(detail),
                    ),
            )
            .when(check.is_some(), |row| {
                row.child(
                    components::settings_button(palette, "settings-check-updates", action)
                        .when(!busy, |button| {
                            button.on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(CheckForUpdates), cx)
                            })
                        })
                        .when(busy, |button| button.opacity(0.6)),
                )
            })
    }

    fn cache_section(&self, palette: CadencePalette, cx: &mut Context<Self>) -> Div {
        let cache = services::AppServices::image_cache(cx);
        let (bytes, limit, reclaimed) = cache.read(cx).usage();
        let detail = if self.cache_cleared {
            "Cache cleared in memory and on disk. Visible artwork reloads as needed.".to_owned()
        } else if reclaimed {
            "Older artwork has been cleared automatically to keep memory bounded.".to_owned()
        } else {
            format!(
                "Artwork uses {:.1} MiB of {} MiB in memory, plus up to {} MiB saved on disk for faster launches. Older images are removed automatically; manual cleanup is optional.",
                bytes as f64 / 1_048_576.,
                limit / 1_048_576,
                disk_cache::DEFAULT_CAP_BYTES / 1_048_576
            )
        };
        div()
            .mt_8()
            .child(Self::settings_section_header(
                palette,
                "Cache",
                "Artwork is managed automatically.",
            ))
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(rgb(palette.text_muted))
                            .child(detail),
                    )
                    .child(
                        components::settings_button(palette, "settings-clear-cache", "Clear cache")
                            .test_support()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cache.update(cx, |cache, cx| cache.clear(window, cx));
                                http::clear_disk_cache();
                                this.cache_cleared = true;
                                cx.refresh_windows();
                            })),
                    ),
            )
    }

    fn settings_section_header(
        palette: CadencePalette,
        title: &'static str,
        detail: &'static str,
    ) -> Div {
        div()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_primary))
                    .child(title),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(tokens::BODY_TEXT)
                    .text_color(rgb(palette.text_muted))
                    .child(detail),
            )
    }

    fn playback_setting_row(
        palette: CadencePalette,
        title: &'static str,
        description: &'static str,
        field: impl IntoElement,
    ) -> Div {
        h_flex()
            .w_full()
            .justify_between()
            .items_start()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    // Let the description wrap instead of pushing the control out of its card.
                    .min_w_0()
                    .max_w(relative(0.6))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(rgb(palette.text))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(tokens::BODY_TEXT)
                            .line_height(relative(1.45))
                            .text_color(rgb(palette.text_muted))
                            .child(description),
                    ),
            )
            .child(div().flex_none().child(field))
    }

    /// Seven stops from dense to spacious on one line; the chosen stop is
    /// filled, and Reset returns to 100%.
    fn zoom_scale(palette: CadencePalette, current: UiZoom, cx: &mut Context<Self>) -> Div {
        let default = UiZoom::default();
        let stops = UiZoom::ALL.len() as f32;
        let mut row = div()
            .relative()
            .flex()
            .w_full()
            .max_w(Rems(32.))
            .mt_2()
            // The line runs between the first and last dot centres.
            .child(
                div()
                    .absolute()
                    .top(Rems(2.4))
                    .left(relative(0.5 / stops))
                    .right(relative(0.5 / stops))
                    .h(px(2.))
                    .bg(rgb(palette.border)),
            );
        for (index, zoom) in UiZoom::ALL.into_iter().enumerate() {
            let selected = zoom == current;
            let caption = if index == 0 {
                "Dense"
            } else if zoom == default {
                "Default"
            } else if index == UiZoom::ALL.len() - 1 {
                "Spacious"
            } else {
                ""
            };
            let ink = if selected {
                palette.text_primary
            } else {
                palette.text_muted
            };
            row = row.child(
                components::button(palette, ("settings-ui-zoom", usize::from(zoom.percent())))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .py_1p5()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .rounded_lg()
                    .hover(|style| style.bg(rgb(palette.control)))
                    .child(
                        div()
                            .h_5()
                            .whitespace_nowrap()
                            .text_size(tokens::BODY_TEXT)
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(caption),
                    )
                    .child(
                        div()
                            .size_4()
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(if selected {
                                palette.text_primary
                            } else {
                                palette.text_muted
                            }))
                            .bg(rgb(if selected {
                                palette.text_primary
                            } else {
                                palette.surface
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(ink))
                            .child(format!("{}%", zoom.percent())),
                    )
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::SetZoom(zoom))),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(row)
            .when(current != default, |scale| {
                scale.child(
                    div().mt_2().flex().child(
                        components::pill(palette, "settings-ui-zoom-reset", "Reset", false)
                            .test_support()
                            .h_9()
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(SettingsEvent::SetZoom(UiZoom::default()))
                            })),
                    ),
                )
            })
    }

    fn appearance_option(
        &self,
        id: &'static str,
        icon: CadenceIcon,
        label: &'static str,
        preference: ThemePreference,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = appearance::Appearance::palette(cx);
        let selected = appearance::Appearance::preference(cx) == preference;
        components::filled_button(palette, id)
            .h_11()
            .flex_1()
            .gap_2()
            .rounded(tokens::CONTROL_RADIUS)
            .bg(if selected {
                rgb(palette.selection)
            } else {
                rgb(palette.control)
            })
            .text_sm()
            .font_weight(if selected {
                gpui_kit::FontWeight::SEMIBOLD
            } else {
                gpui_kit::FontWeight::MEDIUM
            })
            .text_color(rgb(palette.text_primary))
            .hover(|style| style.bg(rgb(palette.control_hover)))
            .child(components::icon(
                icon,
                tokens::MENU_ICON,
                palette.text_primary,
            ))
            .child(label)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(SettingsEvent::SetTheme(preference));
            }))
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.page(window, cx)
    }
}
