//! Spotify's personalized Home shelves, "Made for you" and "Your top mixes".
//!
//! Spotify closed its recommendation and algorithmic-playlist endpoints to new
//! Web API apps, so these two shelves are read the way Spotify's own clients
//! read them: the private Pathfinder `homeSection` query, signed with the
//! playback session's tokens plus a Web API token as `sp_t`. The contract
//! follows Psst's homeSection integration (jpochyla/psst#745) through
//! Spotifast's reader (MIT). It is not part of the published API and can
//! change without notice, so every failure here only hides the shelves.

use std::{
    collections::HashSet,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use librespot::core::session::Session;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::model::{HomeMix, HomeMixes, Playlist, Provider};

/// The playback session is not up yet, as at launch. Not a failure of the
/// read itself: the page asks again once playback has connected.
#[derive(Debug)]
pub(crate) struct PlaybackNotConnected;

impl PlaybackNotConnected {
    /// What the error says; the page, which only receives text, looks for it.
    pub(crate) const MESSAGE: &'static str = "Spotify playback is not connected";
}

impl std::fmt::Display for PlaybackNotConnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::MESSAGE)
    }
}

impl std::error::Error for PlaybackNotConnected {}

const ENDPOINT: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const MADE_FOR_YOU: &str = "spotify:section:0JQ5DAUnp4wcj0bCb3wh3S";
const TOP_MIXES: &str = "spotify:section:0JQ5DAnM3wGh0gz1MXnu89";
const QUERY_HASH: &str = "eb3fba2d388cf4fc4d696b1757a58584e9538a3b515ea742e9cc9465807340be";
/// Spotify DJ is a special context that librespot cannot play as a playlist.
const DJ_URI: &str = "spotify:playlist:37i9dQZF1EYkqdzj48dyYq";
const CACHE_AGE: Duration = Duration::from_secs(10 * 60);
const PAGE_SIZE: usize = 20;
const MAX_PAGES: usize = 10;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// What one account's last read answered, and any pause Spotify asked for.
#[derive(Default)]
struct State {
    account: String,
    cached: Option<(Instant, HomeMixes)>,
    cooldown: Option<(Instant, Duration)>,
}

/// One lock around the whole read, so concurrent Home loads share one answer.
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

/// No user agent on purpose: tried against the live service, Spotify answers
/// this request with 403 "Client/request not allowed" when it carries an
/// unfamiliar one ("Sonder/x.y.z") and with 200 when it carries none.
fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(Into::into)
}

/// Both shelves for the session's account, from memory when read lately.
/// `web_token` is a current Web API access token.
pub(crate) async fn load(session: &Session, web_token: String, force: bool) -> Result<HomeMixes> {
    wait_until_ready(session).await?;
    let mut state = state().lock().await;
    let account = session.username();
    if state.account != account {
        *state = State {
            account,
            ..State::default()
        };
    }
    if let Some((at, mixes)) = &state.cached
        && !force
        && at.elapsed() < CACHE_AGE
    {
        return Ok(mixes.clone());
    }
    if let Some((at, wait)) = state.cooldown
        && at.elapsed() < wait
    {
        bail!("Spotify asked for a pause before the next Home read");
    }
    let (mixes, wait) = tokio::time::timeout(Duration::from_secs(60), fetch(session, &web_token))
        .await
        .context("Made for you took too long to load")?;
    if let Some(wait) = wait {
        state.cooldown = Some((Instant::now(), wait));
    }
    let mixes = mixes?;
    state.cached = Some((Instant::now(), mixes.clone()));
    Ok(mixes)
}

/// The session announces the account's country a moment after it connects,
/// and Spotify refuses the query without it. Waits for that, then gives up
/// as "not connected" so the page asks again later.
async fn wait_until_ready(session: &Session) -> Result<()> {
    for _ in 0..40 {
        if !session.is_invalid() && !session.country().is_empty() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(PlaybackNotConnected.into())
}

async fn fetch(session: &Session, web_token: &str) -> (Result<HomeMixes>, Option<Duration>) {
    let tokens = async {
        let client = client()?;
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
        Ok::<_, anyhow::Error>((client, token.access_token, client_token))
    }
    .await;
    let (client, token, client_token) = match tokens {
        Ok(tokens) => tokens,
        Err(error) => return (Err(error), None),
    };
    let country = session.country();
    let timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned());
    // Spotify answers 403 "Client/request not allowed" to some pairings of the
    // `sp_t` context and the session's tokens. Try the Web API token first,
    // as Spotifast does, then the session's own token, then none, and keep
    // whichever Spotify accepts, logging what each one got.
    let contexts = [
        ("web token", web_token),
        ("session token", &token),
        ("none", ""),
    ];
    let mut request = Request {
        client: &client,
        token: &token,
        client_token: &client_token,
        country: &country,
        timezone: &timezone,
        web_token,
    };
    let mut made_for_you = Err(SectionError::Failed(anyhow!("no attempt was made")));
    for (label, context) in contexts {
        request.web_token = context;
        made_for_you = read_section(&request, MADE_FOR_YOU).await;
        match &made_for_you {
            Ok(_) => {
                log::info!("made for you: Spotify accepted the {label} as context");
                break;
            }
            Err(SectionError::RateLimited(_)) => break,
            Err(SectionError::Failed(error)) => {
                log::warn!("made for you: context {label} failed: {error:#}");
            }
        }
    }
    let top_mixes = if made_for_you.is_ok() {
        read_section(&request, TOP_MIXES).await
    } else {
        Err(SectionError::Failed(anyhow!("skipped")))
    };
    let wait = [&made_for_you, &top_mixes]
        .into_iter()
        .filter_map(|result| match result {
            Err(SectionError::RateLimited(wait)) => Some(*wait),
            _ => None,
        })
        .max();
    // Made for you is the page's reason to ask; Your top mixes may be missing
    // on its own.
    let mixes = made_for_you
        .map_err(SectionError::into_error)
        .map(|made_for_you| HomeMixes {
            made_for_you,
            top_mixes: top_mixes.unwrap_or_default(),
        });
    (mixes, wait)
}

struct Request<'a> {
    client: &'a reqwest::Client,
    token: &'a str,
    client_token: &'a str,
    country: &'a str,
    timezone: &'a str,
    web_token: &'a str,
}

enum SectionError {
    RateLimited(Duration),
    Failed(anyhow::Error),
}

impl SectionError {
    fn into_error(self) -> anyhow::Error {
        match self {
            Self::RateLimited(_) => anyhow!("Spotify is rate limiting the Home read"),
            Self::Failed(error) => error,
        }
    }
}

impl From<anyhow::Error> for SectionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

/// Reads every page of one section, in Spotify's order.
async fn read_section(request: &Request<'_>, section: &str) -> Result<Vec<HomeMix>, SectionError> {
    let mut mixes = Vec::new();
    let mut seen = HashSet::new();
    for page in 0..MAX_PAGES {
        let offset = page * PAGE_SIZE;
        let response = request
            .client
            .post(ENDPOINT)
            .bearer_auth(request.token)
            .header("client-token", request.client_token)
            .header("content-type", "application/json")
            .body(query(request, section, offset).to_string())
            .send()
            .await
            .map_err(|error| anyhow!("Home request failed: {}", error.without_url()))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map_or(DEFAULT_COOLDOWN, Duration::from_secs);
            return Err(SectionError::RateLimited(wait));
        }
        if !status.is_success() {
            let body = response.bytes().await.unwrap_or_default();
            let snippet: String = String::from_utf8_lossy(&body).chars().take(300).collect();
            return Err(anyhow!("Home request answered {status}: {snippet}").into());
        }
        let body: Value = serde_json::from_slice(
            &response
                .bytes()
                .await
                .map_err(|error| anyhow!("Home answer was cut short: {}", error.without_url()))?,
        )
        .map_err(|_| anyhow!("Home answer was not JSON"))?;
        let page = parse_page(&body)?;
        let more = page.more(offset);
        let had_mixes = !page.mixes.is_empty();
        let mut added = 0;
        for mix in page.mixes {
            if seen.insert(mix.playlist.source_id.clone()) {
                mixes.push(mix);
                added += 1;
            }
        }
        if !more {
            return Ok(mixes);
        }
        // A repeated page must not spin forever or pass a prefix off as whole.
        if added == 0 && had_mixes {
            return Err(anyhow!("Home repeated a page").into());
        }
    }
    Err(anyhow!("Home had too many pages").into())
}

fn query(request: &Request<'_>, section: &str, offset: usize) -> Value {
    json!({
        "operationName": "homeSection",
        "variables": {
            "uri": section,
            "country": request.country,
            "timeZone": request.timezone,
            "sp_t": request.web_token,
            "sectionItemsLimit": PAGE_SIZE,
            "sectionItemsOffset": offset
        },
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": QUERY_HASH}}
    })
}

struct ShelfPage {
    mixes: Vec<HomeMix>,
    count: usize,
    total: Option<usize>,
}

impl ShelfPage {
    /// Counts raw cards, so skipped card types do not cut paging short.
    fn more(&self, offset: usize) -> bool {
        self.count > 0
            && self.total.map_or(self.count == PAGE_SIZE, |total| {
                offset.saturating_add(self.count) < total
            })
    }
}

/// Keeps Spotify's order and its localized titles. Cards that are not
/// playlists are skipped.
fn parse_page(body: &Value) -> Result<ShelfPage> {
    if body
        .get("errors")
        .is_some_and(|errors| !errors.is_null() && errors.as_array().is_none_or(|e| !e.is_empty()))
    {
        bail!("Home answered with errors");
    }
    let sections = body
        .pointer("/data/homeSections/sections")
        .and_then(Value::as_array)
        .filter(|sections| !sections.is_empty())
        .context("Home answered without sections")?;
    let mut mixes = Vec::new();
    let mut count = 0;
    let mut total = Some(0usize);
    for section in sections {
        let items = section
            .pointer("/sectionItems/items")
            .and_then(Value::as_array)
            .context("Home section has no items")?;
        count += items.len();
        total = total.and_then(|sum| {
            let section_total = section
                .pointer("/sectionItems/totalCount")
                .and_then(Value::as_u64)?;
            Some(sum + usize::try_from(section_total).ok()?)
        });
        mixes.extend(
            items
                .iter()
                .filter_map(|item| mix(item.pointer("/content/data")?)),
        );
    }
    Ok(ShelfPage {
        mixes,
        count,
        total,
    })
}

fn mix(data: &Value) -> Option<HomeMix> {
    if data.get("__typename")?.as_str()? != "Playlist" {
        return None;
    }
    let uri = data.get("uri")?.as_str()?;
    if uri == DJ_URI {
        return None;
    }
    let id = uri
        .strip_prefix("spotify:playlist:")
        .filter(|id| id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))?;
    let name = data.get("name")?.as_str()?;
    // The widest cover Spotify offers over https.
    let artwork_url = data
        .pointer("/images/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| image.get("sources")?.as_array())
        .flatten()
        .filter_map(|source| {
            let url = source.get("url")?.as_str()?;
            let width = source.get("width").and_then(Value::as_u64).unwrap_or(0);
            url.starts_with("https://").then(|| (width, url.to_owned()))
        })
        .max_by_key(|(width, _)| *width)
        .map(|(_, url)| url);
    let owner = data
        .pointer("/ownerV2/data/name")
        .and_then(Value::as_str)
        .unwrap_or("Spotify");
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .map(strip_markup)
        .filter(|text| !text.is_empty());
    Some(HomeMix {
        playlist: Playlist {
            provider: Provider::Spotify,
            source_id: id.to_owned(),
            name: name.to_owned().into(),
            owner: owner.to_owned().into(),
            track_count: 0,
            artwork_url,
        },
        description,
    })
}

/// Spotify's descriptions carry links as HTML, and entities for punctuation.
fn strip_markup(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut in_tag = false;
    for character in text.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    plain
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .trim()
        .to_owned()
}

/// How many songs one session-read page holds. A made-for-you playlist is
/// about this long, so it opens in one request.
const SESSION_PAGE: usize = 50;

/// One page of a playlist read through the playback session. The Web API
/// refuses Spotify's own algorithmic playlists to apps like this one, but the
/// session lists their songs, and those songs resolve by id like any other.
pub(crate) async fn session_playlist_page(
    session: &Session,
    spotify: &crate::spotify::Spotify,
    source_id: &str,
    offset: u32,
) -> Result<crate::model::PlaylistTrackPage> {
    use librespot::{
        core::SpotifyUri,
        metadata::{Metadata as _, Playlist as SessionPlaylist},
    };

    let uri = SpotifyUri::from_uri(&format!("spotify:playlist:{source_id}"))
        .context("invalid playlist id")?;
    let list = SessionPlaylist::get(session, &uri)
        .await
        .map_err(|error| anyhow!("the playback session could not read this playlist: {error}"))?;
    let uris: Vec<String> = list
        .tracks()
        .filter_map(|uri| uri.to_uri().ok())
        .filter(|uri| uri.starts_with("spotify:track:"))
        .collect();
    let total = u32::try_from(uris.len()).unwrap_or(u32::MAX);
    let start = (offset as usize).min(uris.len());
    let end = (start + SESSION_PAGE).min(uris.len());
    let window = &uris[start..end];
    let tracks = spotify.resolve_playable_track_uris(window).await?;
    let source_offsets = tracks
        .iter()
        .map(|track| {
            let position = window
                .iter()
                .position(|uri| uri.rsplit(':').next() == Some(track.source_id.as_str()))
                .unwrap_or(0);
            u32::try_from(start + position).unwrap_or(u32::MAX)
        })
        .collect();
    Ok(crate::model::PlaylistTrackPage {
        tracks,
        source_offsets,
        next_offset: (end < uris.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX)),
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(uri: &str, name: &str) -> Value {
        json!({"content": {"data": {
            "__typename": "Playlist",
            "uri": uri,
            "name": name,
            "description": "<a href=\"x\">glaive</a>, Ericdoa &amp; more",
            "images": {"items": [{"sources": [
                {"url": "https://i.scdn.co/small", "width": 64},
                {"url": "https://i.scdn.co/large", "width": 640},
                {"url": "http://insecure", "width": 9999}
            ]}]},
            "ownerV2": {"data": {"name": "Spotify"}}
        }}})
    }

    fn page(items: Vec<Value>, total: usize) -> Value {
        json!({"data": {"homeSections": {"sections": [
            {"sectionItems": {"items": items, "totalCount": total}}
        ]}}})
    }

    const ONE: &str = "spotify:playlist:37i9dQZF1E39Q6ggt8iyvY";
    const TWO: &str = "spotify:playlist:37i9dQZF1E3aaaaaaaaaaa";

    #[test]
    fn a_card_becomes_a_mix_with_the_widest_secure_cover() {
        let parsed = parse_page(&page(vec![card(ONE, "Daily Mix 1")], 1)).unwrap();
        let mix = &parsed.mixes[0];
        assert_eq!(mix.playlist.source_id, "37i9dQZF1E39Q6ggt8iyvY");
        assert_eq!(mix.playlist.name.as_ref(), "Daily Mix 1");
        assert_eq!(
            mix.playlist.artwork_url.as_deref(),
            Some("https://i.scdn.co/large")
        );
        assert_eq!(mix.description.as_deref(), Some("glaive, Ericdoa & more"));
    }

    #[test]
    fn other_card_types_the_dj_and_bad_ids_are_skipped_but_counted() {
        let mut album = card(ONE, "An album");
        album["content"]["data"]["__typename"] = json!("Album");
        let items = vec![
            album,
            card(DJ_URI, "DJ"),
            card("spotify:playlist:short", "Bad"),
            card(TWO, "Kept"),
        ];
        let parsed = parse_page(&page(items, 4)).unwrap();
        assert_eq!(parsed.mixes.len(), 1);
        assert_eq!(parsed.count, 4);
        assert!(!parsed.more(0));
    }

    #[test]
    fn paging_follows_the_total_not_the_cards_kept() {
        let parsed = parse_page(&page(vec![card(ONE, "One")], 45)).unwrap();
        assert!(parsed.more(0));
        assert!(!parsed.more(44));
    }

    #[test]
    fn an_empty_or_errored_answer_is_a_failure() {
        assert!(parse_page(&json!({"data": {"homeSections": {"sections": []}}})).is_err());
        assert!(parse_page(&json!({"errors": [{"message": "no"}]})).is_err());
        assert!(parse_page(&json!({})).is_err());
    }
}
