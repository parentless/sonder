//! What the listener plays most, shared by Home and artist pages.
//!
//! Spotify answers one time range per request pair. Home loads the range on
//! screen and the longer one it is compared with; artist pages only read what
//! is already here, so opening a profile never costs a request.
use super::*;
use std::time::SystemTime;

/// Top items drift over weeks, so there is no reason to fetch them often.
const STALE_TIME: Duration = Duration::from_secs(30 * 60);

/// One time range's fetched lists, kept so switching back needs no request.
pub(super) struct RangeData {
    pub(super) artists: Arc<[model::Artist]>,
    loaded_at: SystemTime,
}

impl RangeData {
    fn is_fresh(&self) -> bool {
        self.loaded_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed < STALE_TIME)
    }
}

pub(super) enum TopEvent {
    Loaded(model::HomeRange),
    Failed(model::HomeRange, SharedString),
}

pub(super) struct TopListening {
    backend: BackendHandle,
    data: [Option<RangeData>; 3],
    requests: [Option<gpui_kit::Task<()>>; 3],
    /// The saved sign-in lacks the permission to read top items.
    permission_missing: bool,
    _session: Subscription,
}

impl EventEmitter<TopEvent> for TopListening {}

struct Shared(Entity<TopListening>);
impl gpui_kit::Global for Shared {}

impl TopListening {
    pub(super) fn shared(backend: &BackendHandle, cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<Shared>() {
            return shared.0.clone();
        }
        let backend = backend.clone();
        let entity = cx.new(|cx: &mut Context<Self>| {
            let session = services::AppServices::session(cx);
            Self {
                backend,
                data: [None, None, None],
                requests: [None, None, None],
                permission_missing: false,
                _session: cx.subscribe(
                    &session,
                    |this: &mut Self, _, event: &session::SessionEvent, cx| {
                        // A different account, or a fresh sign-in that may now carry
                        // the permission: start over.
                        if matches!(
                            event,
                            session::SessionEvent::LoggedOut | session::SessionEvent::Restarted
                        ) {
                            this.reset(cx);
                        }
                    },
                ),
            }
        });
        cx.set_global(Shared(entity.clone()));
        entity
    }

    /// The shared entity of an app that already has one.
    #[cfg(test)]
    pub(super) fn existing(cx: &App) -> Entity<Self> {
        cx.global::<Shared>().0.clone()
    }

    /// Puts a chart in place without a request, for screenshots and tests.
    #[cfg(test)]
    pub(super) fn seed(
        &mut self,
        range: model::HomeRange,
        artists: Vec<model::Artist>,
        cx: &mut Context<Self>,
    ) {
        self.data[range.index()] = Some(RangeData {
            artists: artists.into(),
            loaded_at: SystemTime::now(),
        });
        cx.notify();
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.data = [None, None, None];
        self.requests = [None, None, None];
        self.permission_missing = false;
        cx.notify();
    }

    pub(super) fn data(&self, range: model::HomeRange) -> Option<&RangeData> {
        self.data[range.index()].as_ref()
    }

    pub(super) fn loading(&self, range: model::HomeRange) -> bool {
        self.requests[range.index()].is_some()
    }

    pub(super) fn permission_missing(&self) -> bool {
        self.permission_missing
    }

    /// Fetches `range` unless it is fresh, already on its way, or not allowed.
    pub(super) fn ensure(&mut self, range: model::HomeRange, cx: &mut Context<Self>) {
        let fresh = self.data(range).is_some_and(RangeData::is_fresh);
        if fresh || self.loading(range) || self.permission_missing {
            return;
        }
        let reply = catalog::request(&self.backend, move |respond| BackendCommand::LoadHome {
            range,
            respond,
        });
        self.requests[range.index()] = Some(cx.spawn(async move |this, cx| {
            let result = reply.await;
            let _ = this.update(cx, |top, cx| {
                top.requests[range.index()] = None;
                match result {
                    Ok(mut home) => {
                        home.artists.truncate(model::HOME_ARTIST_LIMIT);
                        top.data[range.index()] = Some(RangeData {
                            artists: home.artists.into(),
                            loaded_at: SystemTime::now(),
                        });
                        cx.emit(TopEvent::Loaded(range));
                    }
                    Err(error) => {
                        top.permission_missing =
                            error.as_str() == model::HomePermissionMissing.to_string();
                        if !top.permission_missing {
                            cx.emit(TopEvent::Failed(range, error));
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
