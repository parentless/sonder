//! An artist's monthly listeners, which Spotify shows under the name on its
//! own artist pages but does not give the Web API. They come from the same
//! private Pathfinder service as Made for you (`queryArtistOverview`),
//! through the playback session, so they are only there while playback is
//! connected; the artist page simply leaves the line out otherwise.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use librespot::core::session::Session;
use serde_json::{Value, json};

const ENDPOINT: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const OVERVIEW_HASH: &str = "4bc52527bb77a5f8bbb9afe491e9aa725698d29ab73bff58d49169ee29800167";
/// Listener counts move slowly; an hour is plenty.
const KEEP: Duration = Duration::from_secs(60 * 60);
/// Artists kept, so a long session cannot grow this without end.
const KEPT_ARTISTS: usize = 64;

type Remembered = HashMap<String, (Instant, Option<u64>)>;

fn remembered() -> &'static Mutex<Remembered> {
    static REMEMBERED: OnceLock<Mutex<Remembered>> = OnceLock::new();
    REMEMBERED.get_or_init(Mutex::default)
}

/// The artist's monthly listeners, or `None` when Spotify does not say.
pub(crate) async fn monthly_listeners(session: &Session, source_id: &str) -> Result<Option<u64>> {
    if let Some((at, count)) = remembered()
        .lock()
        .unwrap_or_else(|lock| lock.into_inner())
        .get(source_id)
        && at.elapsed() < KEEP
    {
        return Ok(*count);
    }
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
    let query = json!({
        "operationName": "queryArtistOverview",
        "variables": {
            "uri": format!("spotify:artist:{source_id}"),
            "locale": "",
            "includePrerelease": true
        },
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": OVERVIEW_HASH}}
    });
    let response = crate::made_for_you::client()?
        .post(ENDPOINT)
        .bearer_auth(&token.access_token)
        .header("client-token", client_token)
        .header("content-type", "application/json")
        .body(query.to_string())
        .send()
        .await
        .map_err(|error| anyhow!("artist overview request failed: {}", error.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("artist overview answered {status}"));
    }
    let body: Value = serde_json::from_slice(
        &response
            .bytes()
            .await
            .map_err(|error| anyhow!("artist overview was cut short: {}", error.without_url()))?,
    )
    .map_err(|_| anyhow!("artist overview was not JSON"))?;
    let count = listeners_in(&body);
    let mut kept = remembered().lock().unwrap_or_else(|lock| lock.into_inner());
    if kept.len() >= KEPT_ARTISTS {
        kept.clear();
    }
    kept.insert(source_id.to_owned(), (Instant::now(), count));
    Ok(count)
}

/// The count in an overview answer: a number, or a string of digits.
fn listeners_in(body: &Value) -> Option<u64> {
    match body.pointer("/data/artistUnion/stats/monthlyListeners")? {
        Value::String(text) => text.parse().ok(),
        other => other.as_u64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_is_read_as_a_number_or_digits() {
        let body =
            |value: Value| json!({"data": {"artistUnion": {"stats": {"monthlyListeners": value}}}});
        assert_eq!(listeners_in(&body(json!(8_123_456))), Some(8_123_456));
        assert_eq!(listeners_in(&body(json!("8123456"))), Some(8_123_456));
        assert_eq!(listeners_in(&body(json!("many"))), None);
        assert_eq!(listeners_in(&json!({})), None);
    }
}
