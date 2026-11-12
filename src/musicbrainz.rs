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
    #[serde(rename = "artist-credit", default)]
    credits: Vec<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    #[serde(default)]
    name: String,
    artist: Option<CreditedArtist>,
}

#[derive(Deserialize)]
struct CreditedArtist {
    #[serde(default)]
    name: String,
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

/// How long the whole sweep may run. Whatever has been found by then is returned,
/// so a slow MusicBrainz costs a few artists, not the whole row.
const BUDGET: Duration = Duration::from_secs(50);

/// Upcoming albums, EPs and singles for these artist names, soonest first. An
/// artist that cannot be answered for is skipped.
pub async fn upcoming(names: &[String], today: &str) -> Result<Vec<UpcomingRelease>> {
    let client = client()?;
    let horizon = format!("{}{}", horizon_year(today), &today[4..]);
    let started = std::time::Instant::now();
    let mut found = Vec::new();
    for name in names {
        if started.elapsed() > BUDGET {
            break;
        }
        match artist_upcoming(&client, name, today, &horizon).await {
            Ok(mut releases) => found.append(&mut releases),
            Err(error) => log::debug!("musicbrainz: {name}: {error:#}"),
        }
        tokio::time::sleep(SPACING).await;
    }
    found.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
    found.dedup_by(|a, b| a.artist == b.artist && a.title == b.title);
    Ok(found)
}

/// One request per artist: release groups credited to this name whose first
/// release date is still ahead.
async fn artist_upcoming(
    client: &reqwest::Client,
    name: &str,
    today: &str,
    horizon: &str,
) -> Result<Vec<UpcomingRelease>> {
    let escaped = name.replace(['"', '\\'], " ");
    let query = format!(
        "artist:\"{escaped}\" AND firstreleasedate:[{today} TO {horizon}] AND (primarytype:album OR primarytype:ep OR primarytype:single)"
    );
    let bytes = get(
        client,
        &format!("{API}/release-group/"),
        &[("query", &query), ("fmt", "json"), ("limit", "10")],
    )
    .await?;
    let groups: GroupSearch = serde_json::from_slice(&bytes)?;
    Ok(groups
        .groups
        .into_iter()
        .filter(|group| usable(group, name, today, horizon))
        .map(|group| UpcomingRelease {
            artist: name.to_owned(),
            title: group.title,
            date: group.date,
            kind: group.kind.unwrap_or_default(),
        })
        .collect())
}

/// Only releases credited to this exact artist, with a whole date in range, and
/// only the studio kinds the listener plays. A different artist who shares part
/// of the name is worse than none.
/// Only whole dates in range, and only the studio kinds the listener plays.
fn usable(group: &Group, name: &str, today: &str, horizon: &str) -> bool {
    let credited = group.credits.iter().any(|credit| {
        credit.name.eq_ignore_ascii_case(name)
            || credit
                .artist
                .as_ref()
                .is_some_and(|artist| artist.name.eq_ignore_ascii_case(name))
    });
    credited
        && group.date.len() == 10
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

    fn group(date: &str, secondary: &[&str], credit: &str) -> Group {
        Group {
            title: "T".into(),
            date: date.into(),
            kind: Some("Album".into()),
            secondary: secondary.iter().map(|s| s.to_string()).collect(),
            credits: vec![Credit {
                name: credit.into(),
                artist: None,
            }],
        }
    }

    #[test]
    fn keeps_only_whole_future_studio_dates_by_the_artist() {
        let (today, horizon) = ("2026-11-12", "2028-11-12");
        let ok = |g: Group| usable(&g, "glaive", today, horizon);
        assert!(ok(group("2027-03-01", &[], "glaive")));
        assert!(ok(group("2026-11-12", &[], "GLAIVE")));
        assert!(!ok(group("2026-11-11", &[], "glaive")));
        assert!(!ok(group("2027", &[], "glaive")));
        assert!(!ok(group("2030-01-01", &[], "glaive")));
        assert!(!ok(group("2027-03-01", &["Live"], "glaive")));
        assert!(!ok(group("2027-03-01", &[], "someone else")));
    }

    #[test]
    fn horizon_is_two_years_out() {
        assert_eq!(horizon_year("2026-11-12"), 2028);
    }

    /// Live check against MusicBrainz: `cargo test --lib live_upcoming -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_upcoming_finds_an_announced_album() {
        let found = upcoming(&["glaive".to_owned()], "2026-11-12")
            .await
            .unwrap();
        println!("{found:?}");
        assert!(
            found
                .iter()
                .any(|release| release.title == "Polesden Lacey")
        );
    }
}
