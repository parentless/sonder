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
    pub(super) tracks: Arc<[model::Track]>,
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

/// One artist's place in the listener's charts.
pub(super) struct Yours {
    /// Their songs in the listener's charts, best rank first, then liked songs.
    pub(super) tracks: Arc<[model::Track]>,
    /// Where the artist ranks in the best chart that has them.
    pub(super) artist_rank: Option<(usize, model::HomeRange)>,
}

/// Songs shown on an artist page's "You" tab.
pub(super) const YOURS_LIMIT: usize = 10;

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
        tracks: Vec<model::Track>,
        cx: &mut Context<Self>,
    ) {
        self.data[range.index()] = Some(RangeData {
            artists: artists.into(),
            tracks: tracks.into(),
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
                        home.tracks.truncate(model::HOME_TRACK_LIMIT);
                        top.data[range.index()] = Some(RangeData {
                            artists: home.artists.into(),
                            tracks: home.tracks.into(),
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

    /// The artist's songs and rank in whatever charts are loaded, plus their
    /// liked songs. Reads memory only.
    pub(super) fn yours(&self, artist_id: &str, liked: &[model::Track]) -> Yours {
        yours_from(&self.data, artist_id, liked)
    }
}

fn yours_from(data: &[Option<RangeData>; 3], artist_id: &str, liked: &[model::Track]) -> Yours {
    let by_artist = |track: &model::Track| {
        track
            .artists
            .iter()
            .any(|artist| artist.source_id.as_deref() == Some(artist_id))
    };
    // The 6-month chart first: it is the one Home opens on.
    const ORDER: [model::HomeRange; 3] = [
        model::HomeRange::Months,
        model::HomeRange::Weeks,
        model::HomeRange::AllTime,
    ];
    let mut ranked: Vec<(usize, usize, &model::Track)> = Vec::new();
    let mut artist_rank = None;
    for (order, range) in ORDER.into_iter().enumerate() {
        let Some(chart) = data[range.index()].as_ref() else {
            continue;
        };
        if artist_rank.is_none()
            && let Some(position) = chart
                .artists
                .iter()
                .position(|artist| artist.source_id == artist_id)
        {
            artist_rank = Some((position + 1, range));
        }
        for (position, track) in chart.tracks.iter().enumerate() {
            if by_artist(track)
                && !ranked
                    .iter()
                    .any(|(_, _, seen)| seen.source_id == track.source_id)
            {
                ranked.push((position, order, track));
            }
        }
    }
    ranked.sort_by_key(|(position, order, _)| (*position, *order));
    let mut tracks: Vec<model::Track> = ranked
        .into_iter()
        .map(|(_, _, track)| track.clone())
        .collect();
    for track in liked.iter().filter(|track| by_artist(track)) {
        if tracks.len() >= YOURS_LIMIT {
            break;
        }
        if !tracks.iter().any(|seen| seen.source_id == track.source_id) {
            tracks.push(track.clone());
        }
    }
    tracks.truncate(YOURS_LIMIT);
    Yours {
        tracks: tracks.into(),
        artist_rank,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, artist: &str) -> model::Track {
        model::Track {
            provider: model::Provider::Spotify,
            source_id: id.into(),
            spotify_uri: None,
            isrc: None,
            title: id.to_owned().into(),
            artist: artist.to_owned().into(),
            artists: vec![model::ArtistRef {
                name: artist.to_owned().into(),
                source_id: Some(artist.into()),
                spotify_uri: None,
            }],
            album: "Album".into(),
            album_ref: None,
            duration_ms: 1,
            artwork_url: None,
        }
    }

    fn artist(id: &str) -> model::Artist {
        model::Artist {
            provider: model::Provider::Spotify,
            source_id: id.into(),
            spotify_uri: None,
            name: id.to_owned().into(),
            artwork_url: None,
        }
    }

    fn data(artists: Vec<model::Artist>, tracks: Vec<model::Track>) -> Option<RangeData> {
        Some(RangeData {
            artists: artists.into(),
            tracks: tracks.into(),
            loaded_at: SystemTime::now(),
        })
    }

    #[test]
    fn songs_rank_by_chart_position_then_liked_and_unknown_artists_are_not_known() {
        let months = vec![track("a1", "x"), track("b1", "y"), track("a2", "x")];
        let all = vec![track("a3", "x"), track("a1", "x")];
        let charts = [
            None,
            data(vec![artist("y"), artist("x")], months),
            data(vec![artist("x")], all),
        ];
        let liked = vec![track("a9", "x"), track("z1", "z")];
        let mine = yours_from(&charts, "x", &liked);
        let ids: Vec<_> = mine.tracks.iter().map(|t| t.source_id.as_str()).collect();
        // a1 (6mo #1), a3 (all-time #1), a2 (6mo #3), then the liked a9.
        assert_eq!(ids, ["a1", "a3", "a2", "a9"]);
        assert_eq!(mine.artist_rank, Some((2, model::HomeRange::Months)));
        assert!(!mine.tracks.is_empty() && mine.artist_rank.is_some());
    }

    #[test]
    fn an_artist_the_listener_never_plays_is_not_known() {
        let months = vec![track("a1", "x")];
        let charts = [None, data(vec![artist("x")], months), None];
        let mine = yours_from(&charts, "nobody", &[]);
        assert!(mine.tracks.is_empty() && mine.artist_rank.is_none());
    }
}
