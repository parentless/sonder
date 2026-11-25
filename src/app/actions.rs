use super::*;

impl Workspace {
    /// A pointer press returns keyboard focus to the workspace before the
    /// pressed element can claim it. Controls decline it, so the playback
    /// shortcut keeps working after a click. A text field takes it back.
    pub(super) fn focus_on_pointer_press(
        &mut self,
        _: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
    }

    pub(super) fn open_search(
        &mut self,
        _: &OpenSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toolbar
            .update(cx, |toolbar, cx| toolbar.focus_search(window, cx));
        self.navigate(Route::Search, cx);
    }

    pub(super) fn close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // While the sign-in window is locked on top, the pair stays together.
        if services::AppServices::onboarding_window(cx).is_some() {
            return;
        }
        window.remove_window();
    }

    pub(super) fn dismiss_overlay(
        &mut self,
        _: &DismissOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The first-launch intro cannot be skipped, so Escape does nothing
        // until it ends.
        if self.intro.is_some() {
            return;
        }
        if self.close_credits(cx) {
            return;
        }
        if self.soloist_setup.is_some() {
            self.cancel_soloist_setup(cx);
            return;
        }
        // An in-flight scrub is the most immediate thing Escape can undo, so it
        // takes precedence over closing whatever is open behind it.
        if self.player_bar.update(cx, |bar, cx| bar.cancel_scrub(cx)) {
            return;
        }
        if self
            .toolbar
            .update(cx, |toolbar, cx| toolbar.close_suggestions(cx))
        {
            return;
        }
        if self.router.route() == Route::LocalFiles
            && self
                .local_files
                .update(cx, |page, cx| page.close_link_popover(cx))
        {
            return;
        }
        if self.router.route() == Route::LocalFiles
            && self
                .local_files
                .update(cx, |page, cx| page.clear_search(window, cx))
        {
            return;
        }
        if self.session.read(cx).app_change_confirmation_open() {
            self.cancel_spotify_app_change(cx);
            return;
        }
        self.player
            .update(cx, |player, cx| player.dismiss_problem(cx));
    }

    pub(super) fn act_on_playback_help(
        &mut self,
        action: playback_help::PlaybackHelpAction,
        cx: &mut Context<Self>,
    ) {
        use playback_help::PlaybackHelpAction;
        match action {
            PlaybackHelpAction::Dismiss => {
                self.player
                    .update(cx, |player, cx| player.dismiss_problem(cx));
            }
            PlaybackHelpAction::Retry => {
                self.player
                    .update(cx, |player, cx| player.retry_playback(cx));
            }
            PlaybackHelpAction::Reset => {
                if services::AppServices::preferences(cx).player == crate::playback::Engine::Soloist
                {
                    // Spotify's own player has no browser sign-in to redo;
                    // what needs redoing is its setup.
                    self.player
                        .update(cx, |player, cx| player.dismiss_problem(cx));
                    self.open_soloist_setup(cx);
                } else {
                    self.player
                        .update(cx, |player, cx| player.reset_playback(cx));
                }
            }
            PlaybackHelpAction::SetUpSoloist => {
                self.player
                    .update(cx, |player, cx| player.dismiss_problem(cx));
                self.open_soloist_setup(cx);
            }
            PlaybackHelpAction::UseBuiltIn => {
                self.player
                    .update(cx, |player, cx| player.dismiss_problem(cx));
                self.choose_player(crate::playback::Engine::BuiltIn, cx);
            }
            PlaybackHelpAction::UseAnotherAccount => {
                self.player
                    .update(cx, |player, cx| player.dismiss_problem(cx));
                self.session.update(cx, |session, cx| session.logout(cx));
            }
        }
    }

    pub(super) fn toggle_playback(
        &mut self,
        _: &TogglePlayback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_focused_input(cx) {
            return;
        }
        self.player.update(cx, |player, cx| player.toggle(cx));
    }

    pub(super) fn authenticate(&mut self, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.authenticate(cx));
    }

    pub(super) fn request_spotify_app_change(&mut self, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.request_app_change(cx));
    }

    pub(super) fn cancel_spotify_app_change(&mut self, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.cancel_app_change(cx));
    }

    pub(super) fn confirm_spotify_app_change(&mut self, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.confirm_app_change(cx));
    }

    pub(super) fn logout(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.logout(cx));
    }

    /// Drops every in-flight catalog reply and returns to the top of the app,
    /// for when the account behind them is changing.
    pub(super) fn restart_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(Route::LikedSongs, cx);
        self.clear_pages(window, cx);
        self.pending_radio_request = None;
    }

    pub(super) fn handle_session_event(
        &mut self,
        event: &session::SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            session::SessionEvent::Restarted => self.restart_navigation(window, cx),
            session::SessionEvent::Ready => {
                self.last_error = None;
                cx.notify();
            }
            session::SessionEvent::LoggedOut => {
                self.restart_navigation(window, cx);
                self.clear_account_data(window, cx);
            }
            session::SessionEvent::Failed(error) => {
                self.last_error = Some(error.clone());
                cx.notify();
            }
            session::SessionEvent::Notice(notice) => {
                self.action_notice = Some(notice.clone());
                cx.notify();
            }
        }
    }

    /// Drops every page's contents and any request still in flight.
    fn clear_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toolbar
            .update(cx, |toolbar, cx| toolbar.clear_search(window, cx));
        self.search.update(cx, |page, cx| page.clear(cx));
        self.playlist.update(cx, |page, cx| page.clear(cx));
        self.artist.update(cx, |page, cx| page.clear(cx));
        self.album.update(cx, |page, cx| page.clear(cx));
    }

    /// Forgets everything cached for the account that just went away.
    pub(super) fn clear_account_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| library.clear(cx));
        self.clear_pages(window, cx);
        self.player.update(cx, |player, cx| player.clear(cx));
        self.last_error = None;
        self.action_notice = None;
        cx.notify();
    }

    pub(super) fn load_playlist(&mut self, playlist: model::Playlist, cx: &mut Context<Self>) {
        self.playlist.update(cx, |page, cx| page.open(playlist, cx));
    }

    pub(super) fn submit_search(&mut self, cx: &mut Context<Self>) {
        if !self.search.update(cx, |search, cx| search.submit(cx)) {
            return;
        }
        self.last_error = None;
        self.navigate(Route::Search, cx);
    }

    pub(super) fn end_volume_drag(
        &mut self,
        _: &gpui_kit::MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.player
            .update(cx, |player, cx| player.end_volume_drag(cx));
    }

    /// Hides the queue panel, which the player bar's toggle also reflects.
    pub(super) fn close_queue(&mut self, cx: &mut Context<Self>) {
        self.queue_drawer
            .update(cx, |drawer, cx| drawer.close_menus(cx));
        self.player_bar
            .update(cx, |bar, cx| bar.set_queue_open(false, cx));
        cx.notify();
    }

    pub(super) fn close_lyrics(&mut self, cx: &mut Context<Self>) {
        self.player_bar
            .update(cx, |bar, cx| bar.set_lyrics_open(false, cx));
        cx.notify();
    }

    /// Row action menus are anchored to a page that a route change takes off
    /// screen, so they have to come down with it or they reappear on return.
    pub(super) fn close_track_menus(&mut self, cx: &mut Context<Self>) {
        self.queue_drawer
            .update(cx, |drawer, cx| drawer.close_menus(cx));
        self.liked_songs.update(cx, |page, cx| page.close_menus(cx));
        self.recent.update(cx, |page, cx| page.close_menus(cx));
        self.local_files.update(cx, |page, cx| page.close_menus(cx));
        self.search.update(cx, |page, cx| page.close_menus(cx));
        self.playlist.update(cx, |page, cx| page.close_menus(cx));
        self.artist.update(cx, |page, cx| page.close_menus(cx));
        self.album.update(cx, |page, cx| page.close_menus(cx));
    }

    /// Opens who made `track`, from any page.
    pub(super) fn show_credits(&mut self, track: model::Track, cx: &mut Context<Self>) {
        self.close_track_menus(cx);
        let backend = services::AppServices::backend(cx);
        let dialog = cx.new(|cx| credits_dialog::CreditsDialog::new(track, backend, cx));
        let subscription =
            cx.subscribe(&dialog, |this, _, _: &credits_dialog::CreditsEvent, cx| {
                this.close_credits(cx);
            });
        self.credits_opened += 1;
        self.credits_leaving = None;
        self.credits = Some(workspace::CreditsOpen {
            dialog,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Opens the setup for Spotify's own player. It asks the backend first, so
    /// a saved key goes straight on to downloading and pairing.
    pub(super) fn open_soloist_setup(&mut self, cx: &mut Context<Self>) {
        if self.soloist_setup.is_some() {
            return;
        }
        let dialog = cx.new(|_| soloist_setup::SoloistSetup::new());
        let subscription = cx.subscribe(
            &dialog,
            |this, _, event: &soloist_setup::SoloistSetupEvent, cx| match event {
                soloist_setup::SoloistSetupEvent::Start(api_key) => {
                    services::AppServices::backend(cx).send(
                        crate::backend::BackendCommand::SetUpSoloist {
                            api_key: api_key.clone(),
                        },
                    );
                }
                soloist_setup::SoloistSetupEvent::Cancel => this.cancel_soloist_setup(cx),
            },
        );
        self.soloist_setup = Some(workspace::SoloistSetupOpen {
            dialog,
            _subscription: subscription,
        });
        services::AppServices::backend(cx)
            .send(crate::backend::BackendCommand::SetUpSoloist { api_key: None });
        cx.notify();
    }

    /// Closes the setup and stops whatever it was waiting for.
    pub(super) fn cancel_soloist_setup(&mut self, cx: &mut Context<Self>) {
        if self.soloist_setup.take().is_some() {
            services::AppServices::backend(cx)
                .send(crate::backend::BackendCommand::CancelSoloistSetup);
            cx.notify();
        }
    }

    /// Passes what the backend reports on to the open setup. Once Spotify's
    /// player is ready the choice is saved and the setup closes.
    pub(super) fn soloist_setup_step(
        &mut self,
        step: crate::soloist::Step,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.soloist_setup.as_ref() else {
            return;
        };
        if step == crate::soloist::Step::Done {
            self.soloist_setup = None;
            self.save_player(crate::playback::Engine::Soloist, cx);
            cx.notify();
            return;
        }
        open.dialog
            .update(cx, |dialog, cx| dialog.set_step(step, cx));
    }

    /// The listener's choice of player, from Settings or a dialog.
    pub(super) fn choose_player(
        &mut self,
        player: crate::playback::Engine,
        cx: &mut Context<Self>,
    ) {
        match player {
            crate::playback::Engine::Soloist => self.open_soloist_setup(cx),
            crate::playback::Engine::BuiltIn => {
                if services::AppServices::preferences(cx).player == crate::playback::Engine::BuiltIn
                {
                    return;
                }
                services::AppServices::backend(cx)
                    .send(crate::backend::BackendCommand::UseBuiltInPlayer);
                self.save_player(crate::playback::Engine::BuiltIn, cx);
            }
        }
    }

    fn save_player(&mut self, player: crate::playback::Engine, cx: &mut Context<Self>) {
        let result = services::AppServices::set_player(player, cx);
        let result = services::require_settings_storage(result);
        if let Some(error) = services::preference_save_error("player", result) {
            self.last_error = Some(error);
        }
    }

    /// Closes the credits, returning whether they were open.
    pub(super) fn close_credits(&mut self, cx: &mut Context<Self>) -> bool {
        let leaving = self.credits.take();
        let was_open = leaving.is_some();
        if was_open {
            self.credits_leaving = leaving;
            cx.notify();
        }
        was_open
    }

    pub(super) fn close_account_menu(&mut self, cx: &mut Context<Self>) {
        self.toolbar
            .update(cx, |toolbar, cx| toolbar.close_menu(cx));
    }

    pub(super) fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == Route::WhatsNew {
            return self.open_whats_new(cx);
        }
        let already_there = self.router.route() == route;
        self.router.navigate(route);
        if route == Route::Profile && !already_there {
            self.profile.update(cx, |page, cx| page.opened(cx));
        }
        if route == Route::LocalFiles {
            // Picks up files added or changed since the last look.
            self.library
                .update(cx, |library, cx| library.refresh_local_files(cx));
        }
        self.settle_navigation(cx);
    }

    pub(super) fn open_whats_new(&mut self, cx: &mut Context<Self>) {
        self.router.open_whats_new();
        whats_new::mark_seen();
        self.whats_new.update(cx, |page, cx| page.reopened(cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.clear_whats_new_badge(cx));
        self.settle_navigation(cx);
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.router.open_settings();
        self.settle_navigation(cx);
    }

    pub(super) fn open_playlist(&mut self, origin: Route, cx: &mut Context<Self>) {
        self.router.open_playlist(origin);
        self.settle_navigation(cx);
    }

    pub(super) fn open_artist(
        &mut self,
        artist: model::ArtistRef,
        origin: Route,
        cx: &mut Context<Self>,
    ) {
        if artist.source_id.is_none() {
            return;
        }
        let changed = self.artist.update(cx, |page, cx| page.open(artist, cx));
        self.router.open_artist(origin, changed);
        self.settle_navigation(cx);
    }

    pub(super) fn open_album(
        &mut self,
        album: model::AlbumRef,
        origin: Route,
        cx: &mut Context<Self>,
    ) {
        if album.source_id.is_none() {
            return;
        }
        let changed = self.album.update(cx, |page, cx| page.open(album, cx));
        self.router.open_album(origin, changed);
        self.settle_navigation(cx);
    }

    /// Clears everything overlaying the page and tells the sidebar where the
    /// listener ended up, whichever way the route changed.
    fn settle_navigation(&mut self, cx: &mut Context<Self>) {
        let route = self.router.route();
        self.home
            .update(cx, |page, cx| page.set_visible(route == Route::Home, cx));
        let pinned_origin = self.router.pinned_origin();
        let active_playlist = self
            .playlist
            .read(cx)
            .selected
            .as_ref()
            .map(|p| p.source_id.clone());
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.show_route(route, pinned_origin, active_playlist, cx)
        });
        self.close_queue(cx);
        self.close_account_menu(cx);
        self.close_track_menus(cx);
        self.session
            .update(cx, |session, cx| session.cancel_app_change(cx));
        cx.notify();
    }

    pub(super) fn handle_page_event(
        &mut self,
        _: Entity<impl EventEmitter<page::PageEvent> + 'static>,
        event: &page::PageEvent,
        cx: &mut Context<Self>,
    ) {
        let origin = self.router.route();
        match event {
            page::PageEvent::Loaded => self.last_error = None,
            page::PageEvent::Failed(error) => self.last_error = Some(error.clone()),
            page::PageEvent::OpenPlaylist(playlist) => {
                self.load_playlist(playlist.clone(), cx);
                self.open_playlist(origin, cx);
            }
            page::PageEvent::OpenArtist(artist) => self.open_artist(artist.clone(), origin, cx),
            page::PageEvent::Navigate(route) => self.navigate(*route, cx),
            page::PageEvent::OpenAlbum(album) => self.open_album(album.clone(), origin, cx),
            page::PageEvent::StartRadio(track) => self.start_track_radio(track.clone(), cx),
            page::PageEvent::ShowCredits(track) => self.show_credits(track.clone(), cx),
            page::PageEvent::SavePlaylist { name, tracks } => {
                self.save_playlist(name.clone(), tracks.clone(), cx)
            }
            page::PageEvent::OpenLocalFilesSettings => {
                self.settings
                    .update(cx, |settings, cx| settings.show_local_files(cx));
                self.open_settings(cx);
            }
        }
        cx.notify();
    }

    /// Saves songs as a private Spotify playlist, one at a time, telling the
    /// listener how it went in the notice bar.
    fn save_playlist(
        &mut self,
        name: SharedString,
        tracks: Arc<[model::Track]>,
        cx: &mut Context<Self>,
    ) {
        if self.playlist_request.is_some() {
            return;
        }
        let uris: Vec<String> = tracks
            .iter()
            .filter(|track| !track.is_local())
            .filter_map(|track| track.spotify_uri.clone())
            .collect();
        if uris.is_empty() {
            self.action_notice = Some("There are no Spotify songs here to save".into());
            return;
        }
        self.action_notice = Some(format!("Saving “{name}”…").into());
        let backend = services::AppServices::backend(cx);
        // Weak, so a request still running cannot keep the library alive.
        let library = services::AppServices::library(cx).downgrade();
        let reply = catalog::request(&backend, {
            let name = name.to_string();
            move |respond| BackendCommand::CreatePlaylist {
                name,
                uris,
                respond,
            }
        });
        self.playlist_request = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |this, cx| {
                this.playlist_request = None;
                this.action_notice = Some(match result {
                    Ok(()) => {
                        let _ = library.update(cx, |library, cx| library.playlist_created(cx));
                        format!("Saved “{name}” to your playlists").into()
                    }
                    Err(error) => error,
                });
                cx.notify();
            });
        }));
    }

    /// Queues a radio seeded from `track`, tracking the request so a later
    /// failure or cancellation can clear the notice it puts up.
    fn start_track_radio(&mut self, track: model::Track, cx: &mut Context<Self>) {
        self.action_notice = Some(SharedString::new_static("Starting track radio…"));
        let request_id = next_request_id(&mut self.radio_request_id);
        self.pending_radio_request = Some(request_id);
        let started = self
            .player
            .update(cx, |player, cx| player.start_radio(request_id, track, cx));
        if !started {
            self.pending_radio_request = None;
            self.action_notice = Some(SharedString::new_static("Unable to start track radio"));
        }
    }
}
