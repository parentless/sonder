//! Who made a song: writers, producers, engineers and players.
//!
//! Spotify's Web API does not share credits, so they come from MusicBrainz,
//! which volunteers fill in. A song is found by its ISRC (the code Spotify
//! already gives every track), and its recording and work relationships are
//! read. Coverage is patchy for newer or smaller releases, so an empty answer is
//! normal and the app says so. MusicBrainz allows about one request a second.
use crate::musicbrainz::{API, SPACING, client, get};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A song's reissues often split the credits between them (one has the
/// writers, another the producers), so a few are read and merged.
const RECORDINGS_READ: usize = 3;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Credits {
    pub roles: Vec<Role>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Role {
    pub label: String,
    pub people: Vec<Person>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Person {
    pub name: String,
    /// What they did within the role, such as the instruments they played.
    pub detail: Option<String>,
}

impl Credits {
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

#[derive(Deserialize)]
struct IsrcLookup {
    #[serde(default)]
    recordings: Vec<RecordingId>,
}

#[derive(Deserialize)]
struct RecordingId {
    id: String,
}

#[derive(Deserialize)]
struct Recording {
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Deserialize)]
struct Relation {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    attributes: Vec<String>,
    artist: Option<Named>,
    work: Option<Work>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Work {
    #[serde(default)]
    relations: Vec<Relation>,
}

/// The roles shown, in the order they are shown, with the MusicBrainz
/// relationship that fills each.
const ROLES: &[(&str, &[&str])] = &[
    ("Written by", &["writer", "composer"]),
    ("Lyrics", &["lyricist"]),
    ("Arranged by", &["arranger"]),
    ("Produced by", &["producer"]),
    ("Mixed by", &["mix"]),
    ("Mastered by", &["mastering"]),
    ("Engineered by", &["engineer"]),
    ("Recorded by", &["recording"]),
    ("Vocals", &["vocal"]),
    ("Performed by", &["instrument", "performer"]),
    ("Conducted by", &["conductor"]),
    ("Orchestra", &["orchestra"]),
    ("Programming", &["programming"]),
    ("Remixed by", &["remixer"]),
];

/// Everyone credited on the song with this ISRC.
pub async fn fetch(isrc: &str) -> Result<Credits> {
    let client = client()?;
    let lookup = match get(
        &client,
        &format!("{API}/isrc/{}", isrc.trim()),
        &[("fmt", "json")],
    )
    .await
    {
        Ok(bytes) => serde_json::from_slice::<IsrcLookup>(&bytes)
            .context("could not read MusicBrainz's answer")?,
        // An ISRC MusicBrainz has never seen is a plain "no credits yet".
        Err(error) if error.to_string().contains("404") => return Ok(Credits::default()),
        Err(error) => return Err(error),
    };
    let mut credits = Credits::default();
    for (index, recording) in lookup.recordings.iter().take(RECORDINGS_READ).enumerate() {
        tokio::time::sleep(SPACING).await;
        let bytes = match get(
            &client,
            &format!("{API}/recording/{}", recording.id),
            &[
                ("inc", "artist-rels+work-rels+work-level-rels"),
                ("fmt", "json"),
            ],
        )
        .await
        {
            Ok(bytes) => bytes,
            // The first one failing is a failure; a later one only costs detail.
            Err(error) if index == 0 => return Err(error),
            Err(error) => {
                log::debug!("credits: {error:#}");
                continue;
            }
        };
        let recording: Recording =
            serde_json::from_slice(&bytes).context("could not read MusicBrainz's answer")?;
        credits.merge(parse(&recording));
    }
    Ok(credits)
}

fn parse(recording: &Recording) -> Credits {
    let mut credits = Credits::default();
    let mut add = |relation: &Relation| {
        let Some(artist) = relation.artist.as_ref() else {
            return;
        };
        let Some((label, _)) = ROLES
            .iter()
            .find(|(_, kinds)| kinds.contains(&relation.kind.as_str()))
        else {
            return;
        };
        let detail = (!relation.attributes.is_empty()).then(|| relation.attributes.join(", "));
        credits.add(label, artist.name.trim(), detail);
    };
    for relation in &recording.relations {
        match &relation.work {
            Some(work) => work.relations.iter().for_each(&mut add),
            None => add(relation),
        }
    }
    credits.sort();
    credits
}

impl Credits {
    fn add(&mut self, label: &str, name: &str, detail: Option<String>) {
        if name.is_empty() {
            return;
        }
        let role = match self.roles.iter().position(|role| role.label == label) {
            Some(index) => &mut self.roles[index],
            None => {
                self.roles.push(Role {
                    label: label.to_owned(),
                    people: Vec::new(),
                });
                self.roles.last_mut().expect("just pushed")
            }
        };
        match role.people.iter_mut().find(|person| person.name == name) {
            Some(person) => {
                // The same person on the same role twice: keep every detail once.
                if let Some(detail) = detail {
                    let mut parts: Vec<String> = person
                        .detail
                        .take()
                        .map(|existing| existing.split(", ").map(str::to_owned).collect())
                        .unwrap_or_default();
                    for part in detail.split(", ") {
                        if !parts.iter().any(|known| known == part) {
                            parts.push(part.to_owned());
                        }
                    }
                    person.detail = Some(parts.join(", "));
                }
            }
            None => role.people.push(Person {
                name: name.to_owned(),
                detail,
            }),
        }
    }

    fn merge(&mut self, other: Credits) {
        for role in other.roles {
            for person in role.people {
                self.add(&role.label, &person.name, person.detail);
            }
        }
        self.sort();
    }

    /// Roles in the order they are shown; people stay in the order found.
    fn sort(&mut self) {
        self.roles.sort_by_key(|role| {
            ROLES
                .iter()
                .position(|(label, _)| *label == role.label)
                .unwrap_or(ROLES.len())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(json: &str) -> Recording {
        serde_json::from_str(json).expect("fixture parses")
    }

    const PRODUCERS: &str = r#"{"relations":[
        {"type":"producer","attributes":[],"artist":{"name":"Paul Q. Kolderie"}},
        {"type":"producer","attributes":[],"artist":{"name":"Sean Slade"}},
        {"type":"mix","attributes":[],"artist":{"name":"Sean Slade"}},
        {"type":"instrument","attributes":["guitar"],"artist":{"name":"Thom Yorke"}},
        {"type":"instrument","attributes":["piano"],"artist":{"name":"Thom Yorke"}},
        {"type":"instrument","attributes":["guitar"],"artist":{"name":"Thom Yorke"}},
        {"type":"tribute","attributes":[],"artist":{"name":"Nobody Shown"}}
    ]}"#;

    const WRITERS: &str = r#"{"relations":[
        {"type":"performance","attributes":[],"work":{"relations":[
            {"type":"composer","attributes":[],"artist":{"name":"Thom Yorke"}},
            {"type":"lyricist","attributes":[],"artist":{"name":"Thom Yorke"}},
            {"type":"composer","attributes":[],"artist":{"name":"Albert Hammond"}}
        ]}}
    ]}"#;

    fn names(credits: &Credits, label: &str) -> Vec<String> {
        credits
            .roles
            .iter()
            .find(|role| role.label == label)
            .map(|role| role.people.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn reads_the_roles_it_shows_and_ignores_the_rest() {
        let credits = parse(&recording(PRODUCERS));
        assert_eq!(
            names(&credits, "Produced by"),
            ["Paul Q. Kolderie", "Sean Slade"]
        );
        assert_eq!(names(&credits, "Mixed by"), ["Sean Slade"]);
        assert!(
            !credits
                .roles
                .iter()
                .any(|role| role.people.iter().any(|p| p.name == "Nobody Shown"))
        );
    }

    #[test]
    fn one_player_on_two_instruments_is_one_line_with_each_once() {
        let credits = parse(&recording(PRODUCERS));
        let played = credits
            .roles
            .iter()
            .find(|role| role.label == "Performed by")
            .expect("players");
        assert_eq!(played.people.len(), 1);
        assert_eq!(played.people[0].detail.as_deref(), Some("guitar, piano"));
    }

    #[test]
    fn writers_come_from_the_work_and_lead_the_list() {
        let mut credits = parse(&recording(PRODUCERS));
        credits.merge(parse(&recording(WRITERS)));
        assert_eq!(credits.roles[0].label, "Written by");
        assert_eq!(
            names(&credits, "Written by"),
            ["Thom Yorke", "Albert Hammond"]
        );
        assert_eq!(names(&credits, "Lyrics"), ["Thom Yorke"]);
        assert_eq!(credits.roles[1].label, "Lyrics");
        assert_eq!(credits.roles[2].label, "Produced by");
    }

    #[test]
    fn merging_the_same_credits_twice_changes_nothing() {
        let once = parse(&recording(PRODUCERS));
        let mut twice = once.clone();
        twice.merge(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn nothing_found_is_empty() {
        assert!(parse(&recording(r#"{"relations":[]}"#)).is_empty());
        assert!(parse(&recording("{}")).is_empty());
    }

    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_credits_for_a_well_documented_song() {
        let credits = fetch("GBAYE9200070").await.expect("answer");
        assert!(names(&credits, "Written by").contains(&"Thom Yorke".to_owned()));
        assert!(!names(&credits, "Produced by").is_empty());
    }
}
