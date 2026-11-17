//! Artists that sound like the ones the listener plays.
//!
//! Spotify closed its related-artists and recommendations endpoints to new
//! apps, so this asks ListenBrainz, which answers from what listeners play in
//! the same sessions and needs no account or key. It speaks in MusicBrainz ids,
//! so each seed artist is first found by name on MusicBrainz, which allows
//! about one request a second.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

use crate::musicbrainz;

const SIMILAR_API: &str = "https://labs.api.listenbrainz.org/similar-artists/json";
/// ListenBrainz's session-based similarity, the one its own site shows.
const ALGORITHM: &str =
    "session_based_days_9000_session_300_contribution_5_threshold_15_limit_50_skip_30";
/// Similar artists kept per seed; enough to find a few the listener lacks.
const PER_SEED: usize = 12;
/// The whole lookup stops here and keeps what it has.
const BUDGET: Duration = Duration::from_secs(40);

/// One artist that sounds like some of the seeds.
#[derive(Clone, Debug, PartialEq)]
pub struct Similar {
    pub name: String,
    /// 1 to 100, against the closest artist found for the same seed.
    pub match_percent: u8,
    /// The seeds it was found through.
    pub because: Vec<String>,
}

#[derive(Deserialize)]
struct ArtistSearch {
    #[serde(default)]
    artists: Vec<Found>,
}

#[derive(Deserialize)]
struct Found {
    id: String,
    name: String,
    #[serde(default)]
    score: u32,
}

#[derive(Deserialize)]
struct Neighbour {
    name: String,
    #[serde(default)]
    score: u64,
}

/// What a name must match to be the same artist: case and spacing aside.
pub fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Artists like `seeds`, ranked by how many seeds they were found through and
/// then by how close they are. A seed that cannot be answered for is skipped.
pub async fn like(seeds: &[String]) -> Result<Vec<Similar>> {
    let client = musicbrainz::client()?;
    let started = std::time::Instant::now();
    let mut merged: HashMap<String, Similar> = HashMap::new();
    for seed in seeds {
        if started.elapsed() > BUDGET {
            break;
        }
        match neighbours(&client, seed).await {
            Ok(found) => merge(&mut merged, seed, found),
            Err(error) => log::debug!("similar: {seed}: {error:#}"),
        }
        tokio::time::sleep(musicbrainz::SPACING).await;
    }
    Ok(ranked(merged))
}

async fn neighbours(client: &reqwest::Client, seed: &str) -> Result<Vec<(String, u8)>> {
    let escaped = seed.replace(['"', '\\'], " ");
    let query = format!("artist:\"{escaped}\"");
    let bytes = musicbrainz::get(
        client,
        &format!("{}/artist/", musicbrainz::API),
        &[("query", &query), ("fmt", "json"), ("limit", "5")],
    )
    .await?;
    let search: ArtistSearch = serde_json::from_slice(&bytes)?;
    let wanted = normalise(seed);
    let artist = search
        .artists
        .into_iter()
        .filter(|found| found.score >= 90 && normalise(&found.name) == wanted)
        .max_by_key(|found| found.score)
        .context("MusicBrainz does not know that artist")?;
    let bytes = musicbrainz::get(
        client,
        SIMILAR_API,
        &[("artist_mbids", &artist.id), ("algorithm", ALGORITHM)],
    )
    .await?;
    let found: Vec<Neighbour> = serde_json::from_slice(&bytes)?;
    Ok(scaled(found))
}

/// The closest neighbours, each as a percentage of the closest one.
fn scaled(mut found: Vec<Neighbour>) -> Vec<(String, u8)> {
    found.sort_by_key(|neighbour| std::cmp::Reverse(neighbour.score));
    found.truncate(PER_SEED);
    let top = found.first().map_or(1, |first| first.score.max(1));
    found
        .into_iter()
        .map(|neighbour| {
            let percent = (neighbour.score * 100 / top).clamp(1, 100) as u8;
            (neighbour.name, percent)
        })
        .collect()
}

fn merge(merged: &mut HashMap<String, Similar>, seed: &str, found: Vec<(String, u8)>) {
    for (name, percent) in found {
        let entry = merged.entry(normalise(&name)).or_insert_with(|| Similar {
            name,
            match_percent: 0,
            because: Vec::new(),
        });
        entry.match_percent = entry.match_percent.max(percent);
        if !entry.because.iter().any(|known| known == seed) {
            entry.because.push(seed.to_owned());
        }
    }
}

fn ranked(merged: HashMap<String, Similar>) -> Vec<Similar> {
    let mut all: Vec<Similar> = merged.into_values().collect();
    all.sort_by(|a, b| {
        b.because
            .len()
            .cmp(&a.because.len())
            .then(b.match_percent.cmp(&a.match_percent))
            .then(a.name.cmp(&b.name))
    });
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_regardless_of_case_and_punctuation() {
        assert_eq!(normalise("Glass Animals"), normalise("glass  animals"));
        assert_eq!(normalise("Jay-Z"), normalise("JAY Z"));
        assert_ne!(normalise("Glaive"), normalise("Glasser"));
    }

    #[test]
    fn neighbours_are_scaled_against_the_closest_one() {
        let found = vec![
            Neighbour {
                name: "b".into(),
                score: 200,
            },
            Neighbour {
                name: "a".into(),
                score: 400,
            },
        ];
        assert_eq!(
            scaled(found),
            vec![("a".to_owned(), 100), ("b".to_owned(), 50)]
        );
    }

    #[test]
    fn artists_found_through_more_seeds_come_first() {
        let mut merged = HashMap::new();
        merge(
            &mut merged,
            "glaive",
            vec![("Ericdoa".into(), 100), ("Bladee".into(), 90)],
        );
        merge(&mut merged, "Glasser", vec![("bladee".into(), 40)]);
        let ranked = ranked(merged);
        assert_eq!(ranked[0].name, "Bladee");
        assert_eq!(
            ranked[0].because,
            vec!["glaive".to_owned(), "Glasser".to_owned()]
        );
        assert_eq!(ranked[0].match_percent, 90);
        assert_eq!(ranked[1].name, "Ericdoa");
    }

    /// Asks the real services; run by hand:
    /// `cargo test real_services -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "uses the network"]
    async fn real_services_find_artists_like_a_known_one() {
        let found = like(&["glaive".to_owned(), "Glass Animals".to_owned()])
            .await
            .unwrap();
        for similar in found.iter().take(8) {
            println!(
                "{} {}% via {:?}",
                similar.name, similar.match_percent, similar.because
            );
        }
        assert!(found.len() > 5);
    }
}
