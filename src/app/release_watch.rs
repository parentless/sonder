//! Notices new releases from the artists the listener plays most.
//!
//! Spotify has no "upcoming" feed, so this polls the same list Home shows and
//! compares it with the albums seen before. Anything unseen is "new": it gets a
//! badge on Home until the listener has looked, and a tray notification.
use super::*;
use crate::storage::ReleaseWatchState;

/// How often to look. The list is 15 small requests, so this is cheap.
const POLL: Duration = Duration::from_secs(6 * 60 * 60);
/// Waiting lets sign-in and the first Home load finish first.
const FIRST_POLL: Duration = Duration::from_secs(45);
const RETRY: Duration = Duration::from_secs(10 * 60);
const KEEP_SEEN: usize = 300;
const KEEP_NEW: usize = 40;

struct ReleaseWatch {
    state: Option<ReleaseWatchState>,
    _poller: Option<gpui_kit::Task<()>>,
}

impl gpui_kit::Global for ReleaseWatch {}

/// Folds a fresh list into the state and returns the ids that just turned up.
/// The first pass only learns what exists, so installing never floods.
fn observe(state: &mut Option<ReleaseWatchState>, albums: &[model::Album]) -> Vec<String> {
    let Some(state) = state.as_mut() else {
        *state = Some(ReleaseWatchState {
            seen: albums.iter().map(|album| album.source_id.clone()).collect(),
            new: Vec::new(),
        });
        return Vec::new();
    };
    let mut found = Vec::new();
    for album in albums {
        let id = &album.source_id;
        if !state.seen.contains(id) && !state.new.contains(id) {
            state.new.push(id.clone());
            found.push(id.clone());
        }
    }
    trim_front(&mut state.new, KEEP_NEW);
    found
}

/// Listener has looked: what was new is now just seen.
fn acknowledge(state: &mut ReleaseWatchState) -> bool {
    if state.new.is_empty() {
        return false;
    }
    let seen = std::mem::take(&mut state.new);
    state.seen.extend(seen);
    trim_front(&mut state.seen, KEEP_SEEN);
    true
}

fn trim_front(list: &mut Vec<String>, keep: usize) {
    if list.len() > keep {
        list.drain(..list.len() - keep);
    }
}

fn notification(albums: &[&model::Album]) -> Option<(String, String)> {
    let first = albums.first()?;
    let artist = first
        .artists
        .first()
        .map_or("an artist you play", |artist| artist.name.as_str());
    Some(match albums.len() {
        1 => (format!("New from {artist}"), first.name.to_string()),
        count => (
            "New releases".to_owned(),
            format!(
                "{count} new releases from artists you play, including {}",
                first.name
            ),
        ),
    })
}

/// Loads the saved state and starts polling. Call once the services exist.
pub(super) fn start(cx: &mut App) {
    let state = services::AppServices::load_release_watch(cx);
    let backend = services::AppServices::backend(cx);
    let poller = cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_POLL).await;
        loop {
            let reply = catalog::request(&backend, |respond| BackendCommand::LoadNewReleases {
                respond,
            });
            let wait = match reply.await {
                Ok(albums) => {
                    cx.update(|cx| apply(&albums, cx));
                    POLL
                }
                Err(error) => {
                    log::debug!("release watch: {error}");
                    RETRY
                }
            };
            cx.background_executor().timer(wait).await;
        }
    });
    cx.set_global(ReleaseWatch {
        state,
        _poller: Some(poller),
    });
}

fn apply(albums: &[model::Album], cx: &mut App) {
    let mut watch = cx.remove_global::<ReleaseWatch>();
    let found = observe(&mut watch.state, albums);
    let snapshot = watch.state.clone();
    cx.set_global(watch);
    if let Some(snapshot) = snapshot {
        services::AppServices::save_release_watch(&snapshot, cx);
    }
    if found.is_empty() {
        return;
    }
    // A window in front already shows the badge; only reach out when it is not.
    if cx.active_window().is_some() {
        return;
    }
    let fresh: Vec<&model::Album> = albums
        .iter()
        .filter(|album| found.contains(&album.source_id))
        .collect();
    if let Some((title, body)) = notification(&fresh) {
        #[cfg(target_os = "windows")]
        super::tray_host::notify(&title, &body, cx);
        #[cfg(not(target_os = "windows"))]
        let _ = (title, body);
    }
}

/// Whether this release arrived since the listener last looked.
pub(super) fn is_new(id: &str, cx: &App) -> bool {
    cx.try_global::<ReleaseWatch>()
        .and_then(|watch| watch.state.as_ref())
        .is_some_and(|state| state.new.iter().any(|new| new == id))
}

/// Marks everything shown as looked at. Call when Home is left.
pub(super) fn acknowledge_all(cx: &mut App) {
    if !cx.has_global::<ReleaseWatch>() {
        return;
    }
    let mut watch = cx.remove_global::<ReleaseWatch>();
    let changed = watch.state.as_mut().is_some_and(acknowledge);
    let snapshot = watch.state.clone();
    cx.set_global(watch);
    if changed && let Some(snapshot) = snapshot {
        services::AppServices::save_release_watch(&snapshot, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str) -> model::Album {
        model::Album {
            provider: model::Provider::Spotify,
            source_id: id.into(),
            spotify_uri: None,
            name: id.into(),
            artists: Vec::new(),
            release_date: None,
            artwork_url: None,
            track_count: None,
        }
    }

    #[test]
    fn the_first_pass_learns_without_announcing() {
        let mut state = None;
        assert!(observe(&mut state, &[album("a"), album("b")]).is_empty());
        let state = state.unwrap();
        assert_eq!(state.seen, ["a", "b"]);
        assert!(state.new.is_empty());
    }

    #[test]
    fn only_unseen_albums_are_announced_once() {
        let mut state = Some(ReleaseWatchState {
            seen: vec!["a".into()],
            new: Vec::new(),
        });
        let list = [album("a"), album("b")];
        assert_eq!(observe(&mut state, &list), ["b"]);
        assert!(observe(&mut state, &list).is_empty());
        assert_eq!(state.as_ref().unwrap().new, ["b"]);
    }

    #[test]
    fn looking_moves_new_to_seen_and_bounds_both() {
        let mut state = ReleaseWatchState {
            seen: Vec::new(),
            new: vec!["b".into()],
        };
        assert!(acknowledge(&mut state));
        assert!(!acknowledge(&mut state));
        assert_eq!(state.seen, ["b"]);
        let mut many: Vec<String> = (0..KEEP_SEEN + 5).map(|n| n.to_string()).collect();
        trim_front(&mut many, KEEP_SEEN);
        assert_eq!(many.len(), KEEP_SEEN);
        assert_eq!(many[0], "5");
    }
}
