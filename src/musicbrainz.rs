//! Best-effort "coming soon" releases from MusicBrainz.
//!
//! Spotify only lists music that is already out, so this asks MusicBrainz, whose
//! volunteers enter announced release dates. Coverage is patchy for smaller
//! artists, which is why it is labelled best-effort in the app. MusicBrainz
//! allows about one request a second, so lookups are spaced out.
use crate::model::UpcomingRelease;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::time::Duration;

const API: &str = "https://musicbrainz.org/ws/2";
const SPACING: Duration = Duration::from_millis(1100);
/// How far ahead a date is still believable.
const HORIZON_YEARS: i64 = 2;

#[derive(Deserialize)]
struct ArtistSearch {
    #[serde(default)]
    artists: Vec<FoundArtist>,
}

#[derive(Deserialize)]
struct FoundArtist {
    id: String,
    name: String,
    #[serde(default)]
    score: u32,
}

#[derive(Deserialize)]
struct GroupSearch {
    #[serde(rename = "release-groups", default)]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    title: String,
    #[serde(rename = "first-release-date", default)]
    date: String,
    #[serde(rename = "primary-type")]
    kind: Option<String>,
    #[serde(rename = "secondary-types", default)]
    secondary: Vec<String>,
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .user_agent(concat!("Sonder/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(Into::into)
}

async fn get(client: &reqwest::Client, url: &str, query: &[(&str, &str)]) -> Result<Vec<u8>> {
    // MusicBrainz answers 503 when it is busy; one patient retry is polite.
    for attempt in 0..2 {
        let response = client
            .get(url)
            .query(query)
            .send()
            .await
            .context("could not reach MusicBrainz")?;
        if response.status().is_success() {
            return Ok(response.bytes().await?.to_vec());
        }
        if attempt == 0 && response.status().as_u16() == 503 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        bail!("MusicBrainz answered {}", response.status());
    }
    unreachable!()
}

/// Upcoming albums, EPs and singles for these artist names, soonest first. An
/// artist that cannot be matched exactly or answered for is skipped.
pub async fn upcoming(names: &[String], today: &str) -> Result<Vec<UpcomingRelease>> {
    let client = client()?;
    let horizon = format!("{}{}", horizon_year(today), &today[4..]);
    let mut found = Vec::new();
    for name in names {
        match artist_upcoming(&client, name, today, &horizon).await {
            Ok(mut releases) => found.append(&mut releases),
            Err(error) => log::debug!("musicbrainz: {name}: {error:#}"),
        }
    }
    found.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
    found.dedup_by(|a, b| a.artist == b.artist && a.title == b.title);
    Ok(found)
}

async fn artist_upcoming(
    client: &reqwest::Client,
    name: &str,
    today: &str,
    horizon: &str,
) -> Result<Vec<UpcomingRelease>> {
    let escaped = name.replace(['"', '\\'], " ");
    let bytes = get(
        client,
        &format!("{API}/artist/"),
        &[
            ("query", &format!("artist:\"{escaped}\"")),
            ("fmt", "json"),
            ("limit", "3"),
        ],
    )
    .await?;
    tokio::time::sleep(SPACING).await;
    let search: ArtistSearch = serde_json::from_slice(&bytes)?;
    // A wrong artist is worse than none: insist on the same name.
    let Some(artist) = search
        .artists
        .into_iter()
        .find(|artist| artist.score >= 95 && artist.name.eq_ignore_ascii_case(name))
    else {
        return Ok(Vec::new());
    };
    let query = format!(
        "arid:{} AND firstreleasedate:[{today} TO {horizon}] AND (primarytype:album OR primarytype:ep OR primarytype:single)",
        artist.id
    );
    let bytes = get(
        client,
        &format!("{API}/release-group/"),
        &[("query", &query), ("fmt", "json"), ("limit", "10")],
    )
    .await?;
    tokio::time::sleep(SPACING).await;
    let groups: GroupSearch = serde_json::from_slice(&bytes)?;
    Ok(groups
        .groups
        .into_iter()
        .filter(|group| usable(group, today, horizon))
        .map(|group| UpcomingRelease {
            artist: artist.name.clone(),
            title: group.title,
            date: group.date,
            kind: group.kind.unwrap_or_default(),
        })
        .collect())
}

/// Only whole dates in range, and only the studio kinds the listener plays.
fn usable(group: &Group, today: &str, horizon: &str) -> bool {
    group.date.len() == 10
        && group.date.as_str() >= today
        && group.date.as_str() <= horizon
        && group.secondary.is_empty()
}

fn horizon_year(today: &str) -> i64 {
    today
        .get(..4)
        .and_then(|year| year.parse::<i64>().ok())
        .unwrap_or(2000)
        + HORIZON_YEARS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(date: &str, secondary: &[&str]) -> Group {
        Group {
            title: "T".into(),
            date: date.into(),
            kind: Some("Album".into()),
            secondary: secondary.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn keeps_only_whole_future_studio_dates() {
        let (today, horizon) = ("2026-11-12", "2028-11-12");
        assert!(usable(&group("2027-03-01", &[]), today, horizon));
        assert!(usable(&group("2026-11-12", &[]), today, horizon));
        assert!(!usable(&group("2026-11-11", &[]), today, horizon));
        assert!(!usable(&group("2027", &[]), today, horizon));
        assert!(!usable(&group("2030-01-01", &[]), today, horizon));
        assert!(!usable(&group("2027-03-01", &["Live"]), today, horizon));
    }

    #[test]
    fn horizon_is_two_years_out() {
        assert_eq!(horizon_year("2026-11-12"), 2028);
    }
}
