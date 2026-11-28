//! Song search that does not touch the Web API.
//!
//! When Spotify rate limits the Web API, playback keeps working because it
//! uses the playback session. Spotify's own clients search through the private
//! Pathfinder service, signed with the same session, so this reads search the
//! same way as Made for you does. The query hash is not part of any published
//! API and can change without notice; every failure here only means the
//! caller falls back to the (limited) Web API error.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use librespot::core::session::Session;
use serde_json::{Value, json};

use crate::model::{
    Album, AlbumRef, Artist, ArtistRef, CatalogSearchResults, Provider, SEARCH_ALBUM_LIMIT,
    SEARCH_ARTIST_LIMIT, SEARCH_TRACK_LIMIT, Track,
};

const ENDPOINT: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
/// `searchDesktop`, as shipped in the web player's search chunk.
const SEARCH_HASH: &str = "eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03";
const LIMIT: usize = 10;

fn query(term: &str) -> Value {
    json!({
        "operationName": "searchDesktop",
        "variables": {
            "searchTerm": term,
            "offset": 0,
            "limit": LIMIT,
            "numberOfTopResults": 5,
            "includeAudiobooks": false,
            "includeArtistHasConcertsField": false,
            "includeLocalConcertsField": false,
            "includeAuthors": false,
            "includePreReleases": true,
            "includeAlbumPreReleases": true,
            "includeEpisodeContentRatingsV2": false,
            "isPrefix": false,
            "sectionFilters": ["GENERIC", "VIDEO_CONTENT"],
            "clientFeatures": []
        },
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": SEARCH_HASH}}
    })
}

/// Spotify's raw answer to a search.
pub(crate) async fn raw(session: &Session, term: &str) -> Result<Value> {
    crate::made_for_you::wait_until_ready(session).await?;
    let token = session
        .login5()
        .auth_token()
        .await
        .map_err(|_| anyhow!("could not authorize the playback session"))?;
    let client_token = session
        .spclient()
        .client_token()
        .await
        .map_err(|_| anyhow!("could not get a client token"))?;
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        crate::made_for_you::client()?
            .post(ENDPOINT)
            .bearer_auth(&token.access_token)
            .header("client-token", client_token)
            .header("content-type", "application/json")
            .body(query(term).to_string())
            .send(),
    )
    .await
    .context("search took too long")?
    .map_err(|error| anyhow!("search request failed: {}", error.without_url()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| anyhow!("search answer was cut short: {}", error.without_url()))?;
    if !status.is_success() {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(300).collect();
        return Err(anyhow!("search answered {status}: {snippet}"));
    }
    serde_json::from_slice(&bytes).context("search answer was not JSON")
}

/// Searches through the playback session. Same shape as the Web API search,
/// so the page cannot tell which route answered.
pub(crate) async fn search(session: &Session, term: &str) -> Result<CatalogSearchResults> {
    parse(&raw(session, term).await?)
}

fn image(sources: Option<&Value>) -> Option<String> {
    let sources = sources?.as_array()?;
    let all: Vec<(u64, &str)> = sources
        .iter()
        .filter_map(|source| Some((source.get("width")?.as_u64()?, source.get("url")?.as_str()?)))
        .collect();
    // Close to 300 pixels wide, as the Web API path picks.
    all.iter()
        .filter(|(width, _)| *width >= 300)
        .min_by_key(|(width, _)| *width)
        .or_else(|| all.iter().max_by_key(|(width, _)| *width))
        .map(|(_, url)| (*url).to_owned())
}

fn id_of(uri: &str) -> Option<String> {
    uri.rsplit(':')
        .next()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn artist_refs(artists: Option<&Value>) -> Vec<ArtistRef> {
    artists
        .and_then(|artists| artists.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            let uri = artist.get("uri")?.as_str()?;
            Some(ArtistRef {
                name: artist.pointer("/profile/name")?.as_str()?.to_owned().into(),
                source_id: id_of(uri),
                spotify_uri: Some(uri.to_owned()),
            })
        })
        .collect()
}

fn playable(data: &Value) -> bool {
    data.pointer("/playability/playable")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn items<'a>(body: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    body.pointer(&format!("/data/searchV2/{key}/items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn parse(body: &Value) -> Result<CatalogSearchResults> {
    if body
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        bail!("search answered with errors");
    }
    if body.pointer("/data/searchV2").is_none() {
        bail!("search answered without results");
    }
    let tracks = items(body, "tracksV2")
        .filter_map(|item| {
            let data = item.pointer("/item/data")?;
            if !playable(data) || data.get("__typename")?.as_str()? != "Track" {
                return None;
            }
            let uri = data.get("uri")?.as_str()?;
            let artists = artist_refs(data.get("artists"));
            let album = data.get("albumOfTrack")?;
            let artwork_url = image(album.pointer("/coverArt/sources"));
            let album_ref = AlbumRef {
                name: album.get("name")?.as_str()?.to_owned().into(),
                spotify_uri: album.get("uri")?.as_str().map(str::to_owned),
                source_id: album.get("id")?.as_str().map(str::to_owned),
                artwork_url: artwork_url.clone(),
            };
            Some(Track {
                provider: Provider::Spotify,
                source_id: id_of(uri)?,
                spotify_uri: Some(uri.to_owned()),
                isrc: None,
                title: data.get("name")?.as_str()?.to_owned().into(),
                artist: artists
                    .iter()
                    .map(|artist| artist.name.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                artists,
                album: album_ref.name.clone(),
                album_ref: Some(album_ref),
                duration_ms: u32::try_from(data.pointer("/duration/totalMilliseconds")?.as_u64()?)
                    .ok()?,
                artwork_url,
            })
        })
        .take(SEARCH_TRACK_LIMIT)
        .collect();
    let albums = items(body, "albumsV2")
        .filter_map(|item| {
            let data = item.get("data")?;
            if !playable(data) {
                return None;
            }
            let uri = data.get("uri")?.as_str()?;
            Some(Album {
                provider: Provider::Spotify,
                source_id: id_of(uri)?,
                spotify_uri: Some(uri.to_owned()),
                name: data.get("name")?.as_str()?.to_owned().into(),
                artists: artist_refs(data.get("artists")),
                release_date: data
                    .pointer("/date/year")
                    .and_then(Value::as_u64)
                    .map(|year| year.to_string()),
                artwork_url: image(data.pointer("/coverArt/sources")),
                track_count: None,
            })
        })
        .take(SEARCH_ALBUM_LIMIT)
        .collect();
    let artists = items(body, "artists")
        .filter_map(|item| {
            let data = item.get("data")?;
            let uri = data.get("uri")?.as_str()?;
            Some(Artist {
                provider: Provider::Spotify,
                source_id: id_of(uri)?,
                spotify_uri: Some(uri.to_owned()),
                name: data.pointer("/profile/name")?.as_str()?.to_owned().into(),
                artwork_url: image(data.pointer("/visuals/avatarImage/sources")),
                genres: Vec::new(),
            })
        })
        .take(SEARCH_ARTIST_LIMIT)
        .collect();
    Ok(CatalogSearchResults {
        artists,
        albums,
        tracks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Value {
        json!({"data": {"searchV2": {
            "tracksV2": {"items": [
                {"item": {"__typename": "TrackResponseWrapper", "data": {
                    "__typename": "Track",
                    "uri": "spotify:track:2JiDi0qAXsPwhPqA2qaKGt",
                    "name": "Bohemian Rhapsody",
                    "duration": {"totalMilliseconds": 354321},
                    "playability": {"playable": true},
                    "artists": {"items": [{"uri": "spotify:artist:1dfeR4HaWDbWqFHLkxsg1d",
                        "profile": {"name": "Queen"}}]},
                    "albumOfTrack": {"id": "1Tk", "name": "A Night At The Opera",
                        "uri": "spotify:album:1Tk",
                        "coverArt": {"sources": [
                            {"width": 64, "height": 64, "url": "https://i/64"},
                            {"width": 640, "height": 640, "url": "https://i/640"},
                            {"width": 300, "height": 300, "url": "https://i/300"}]}}}}},
                {"item": {"__typename": "TrackResponseWrapper", "data": {
                    "__typename": "Track", "uri": "spotify:track:gone", "name": "Gone",
                    "duration": {"totalMilliseconds": 1000},
                    "playability": {"playable": false},
                    "artists": {"items": []},
                    "albumOfTrack": {"id": "x", "name": "x", "uri": "spotify:album:x"}}}}
            ]},
            "albumsV2": {"items": [{"data": {
                "uri": "spotify:album:3DO", "name": "Greatest Hits",
                "date": {"year": 1981}, "playability": {"playable": true},
                "artists": {"items": [{"uri": "spotify:artist:1df", "profile": {"name": "Queen"}}]},
                "coverArt": {"sources": [{"width": 300, "url": "https://a/300"}]}}}]},
            "artists": {"items": [{"data": {
                "uri": "spotify:artist:1dfeR4HaWDbWqFHLkxsg1d",
                "profile": {"name": "Queen"},
                "visuals": {"avatarImage": {"sources": [{"width": 160, "url": "https://p/160"}]}}}}]}
        }}})
    }

    #[test]
    fn a_search_answer_becomes_tracks_albums_and_artists() {
        let results = parse(&answer()).unwrap();
        assert_eq!(results.tracks.len(), 1, "the unplayable song is dropped");
        let track = &results.tracks[0];
        assert_eq!(track.title.as_ref(), "Bohemian Rhapsody");
        assert_eq!(track.artist.as_ref(), "Queen");
        assert_eq!(track.source_id, "2JiDi0qAXsPwhPqA2qaKGt");
        assert_eq!(
            track.spotify_uri.as_deref(),
            Some("spotify:track:2JiDi0qAXsPwhPqA2qaKGt")
        );
        assert_eq!(track.duration_ms, 354_321);
        assert_eq!(track.artwork_url.as_deref(), Some("https://i/300"));
        assert_eq!(results.albums[0].release_date.as_deref(), Some("1981"));
        assert_eq!(results.artists[0].name.as_ref(), "Queen");
        assert_eq!(
            results.artists[0].artwork_url.as_deref(),
            Some("https://p/160")
        );
    }

    #[test]
    fn an_answer_without_results_or_with_errors_is_a_failure() {
        assert!(parse(&json!({"data": {}})).is_err());
        assert!(parse(&json!({"errors": [{"message": "no"}], "data": null})).is_err());
    }

    /// Logs in with the credentials Sonder stored on this machine and prints
    /// what Spotify answers. Run by hand:
    /// `cargo test private_search::tests::probe -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a signed-in Sonder on this machine and the network"]
    fn probe() {
        let term = std::env::var("SONDER_PROBE_TERM").unwrap_or_else(|_| "queen".into());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let playback = crate::playback::BuiltIn::reconnect().await.unwrap();
            let session = playback.session_for_probe();
            let body = raw(session, &term).await.unwrap();
            let text = serde_json::to_string_pretty(&body).unwrap();
            let path = std::env::var("SONDER_PROBE_OUT").unwrap_or_else(|_| "probe.json".into());
            std::fs::write(&path, &text).unwrap();
            println!("wrote {} bytes to {path}", text.len());
            let found = parse(&body).unwrap();
            for track in &found.tracks {
                println!(
                    "track {} - {} {:?}",
                    track.title, track.artist, track.spotify_uri
                );
            }
            println!(
                "{} albums, {} artists",
                found.albums.len(),
                found.artists.len()
            );
        });
    }
}
