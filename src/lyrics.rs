//! Song lyrics from LRCLIB, an open lyrics service that needs no account.
//!
//! Nothing here runs until the listener turns lyrics on and opens them. A song
//! costs one small request (a second only when the exact match is missing), and
//! the parsed lines are a few kilobytes. Finding the current line is a binary
//! search, so following the song costs next to nothing.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

const API: &str = "https://lrclib.net/api";
/// The longest answer read, so a misbehaving server cannot fill memory.
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
/// How far apart, in seconds, a search result's length may be from the song's.
const DURATION_TOLERANCE_SECS: u32 = 3;

/// One line of lyrics. Unsynced lyrics carry `0` for every start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Line {
    pub start_ms: u32,
    pub text: String,
}

/// Lyrics for one song.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// Whether the lines carry timestamps. Without them the lyrics are only
    /// shown, never followed.
    pub synced: bool,
}

impl Lyrics {
    /// The index of the line being sung at `position_ms`, or `None` before the
    /// first line. Always `None` for unsynced lyrics.
    pub fn current(&self, position_ms: u32) -> Option<usize> {
        if !self.synced {
            return None;
        }
        self.lines
            .partition_point(|line| line.start_ms <= position_ms)
            .checked_sub(1)
    }

    /// When the line after `current` starts, for scheduling the next change.
    pub fn next_start_ms(&self, current: Option<usize>) -> Option<u32> {
        if !self.synced {
            return None;
        }
        let next = current.map_or(0, |index| index + 1);
        self.lines.get(next).map(|line| line.start_ms)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// What a song is looked up by.
#[derive(Clone, Debug)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default, rename = "plainLyrics")]
    plain: Option<String>,
    #[serde(default, rename = "syncedLyrics")]
    synced: Option<String>,
}

/// Parses an LRC file: `[mm:ss.xx]text`, with several stamps allowed on one
/// line. Tags such as `[ar:Artist]` are skipped.
pub fn parse_synced(text: &str) -> Lyrics {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start();
        let mut stamps = Vec::new();
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else {
                break;
            };
            match parse_stamp(&inner[..end]) {
                Some(ms) => stamps.push(ms),
                // A tag line like `[ar:Someone]`, or text that is not a stamp.
                None => break,
            }
            rest = inner[end + 1..].trim_start();
        }
        let text = rest.trim();
        for start_ms in stamps {
            lines.push(Line {
                start_ms,
                text: text.to_owned(),
            });
        }
    }
    lines.sort_by_key(|line| line.start_ms);
    // Leading and trailing silence markers carry nothing to show.
    while lines.first().is_some_and(|line| line.text.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    Lyrics {
        synced: !lines.is_empty(),
        lines,
    }
}

fn parse_stamp(stamp: &str) -> Option<u32> {
    let (minutes, seconds) = stamp.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    let (whole, fraction) = seconds.split_once(['.', ':']).unwrap_or((seconds, ""));
    let whole: u32 = whole.trim().parse().ok()?;
    // Two digits are hundredths, three are thousandths.
    let millis = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().ok()? * 100,
        2 => fraction.parse::<u32>().ok()? * 10,
        _ => fraction.get(..3)?.parse::<u32>().ok()?,
    };
    minutes
        .checked_mul(60_000)?
        .checked_add(whole.checked_mul(1000)?)?
        .checked_add(millis)
}

/// Unsynced lyrics: one line per text line, blank lines kept as breaks.
pub fn parse_plain(text: &str) -> Lyrics {
    let mut lines: Vec<Line> = text
        .lines()
        .map(|line| Line {
            start_ms: 0,
            text: line.trim().to_owned(),
        })
        .collect();
    while lines.first().is_some_and(|line| line.text.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.text.is_empty()) {
        lines.pop();
    }
    Lyrics {
        lines,
        synced: false,
    }
}

fn from_entry(entry: &Entry) -> Lyrics {
    if entry.instrumental {
        return Lyrics::default();
    }
    if let Some(synced) = entry.synced.as_deref().map(parse_synced)
        && !synced.is_empty()
    {
        return synced;
    }
    entry.plain.as_deref().map(parse_plain).unwrap_or_default()
}

/// One client for the process, so skipping through songs reuses the open
/// connection instead of setting up TLS for every lookup. It is built on the
/// first lookup, never before.
fn client() -> Result<&'static reqwest::Client> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .user_agent(concat!("Sonder/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Reads the body a chunk at a time and stops at the limit, so an answer with
/// no stated length still cannot grow past it in memory.
async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        bail!("lyrics answer is too large");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!("lyrics answer is too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Looks a song up. An empty `Lyrics` means the service has none.
pub async fn fetch(query: &Query) -> Result<Lyrics> {
    let client = client()?;
    let seconds = (query.duration_ms / 1000).to_string();
    let mut exact = vec![
        ("track_name", query.title.as_str()),
        ("artist_name", query.artist.as_str()),
        ("duration", seconds.as_str()),
    ];
    if !query.album.is_empty() {
        exact.push(("album_name", query.album.as_str()));
    }
    let response = client
        .get(format!("{API}/get"))
        .query(&exact)
        .send()
        .await
        .context("could not reach the lyrics service")?;
    if response.status().is_success() {
        let entry: Entry = serde_json::from_slice(&read_limited(response).await?)
            .context("unreadable lyrics answer")?;
        return Ok(from_entry(&entry));
    }
    if response.status().as_u16() != 404 {
        bail!("the lyrics service answered {}", response.status());
    }
    // No exact match: take the closest in length from a search.
    let response = client
        .get(format!("{API}/search"))
        .query(&[
            ("track_name", query.title.as_str()),
            ("artist_name", query.artist.as_str()),
        ])
        .send()
        .await
        .context("could not reach the lyrics service")?;
    if !response.status().is_success() {
        return Ok(Lyrics::default());
    }
    let entries: Vec<Entry> = serde_json::from_slice(&read_limited(response).await?)
        .context("unreadable lyrics answer")?;
    Ok(closest(&entries, query.duration_ms / 1000)
        .map(from_entry)
        .unwrap_or_default())
}

/// The result nearest in length to the song, within a few seconds.
fn closest(entries: &[Entry], seconds: u32) -> Option<&Entry> {
    entries
        .iter()
        .filter(|entry| entry.synced.is_some() || entry.plain.is_some())
        .filter_map(|entry| {
            let gap = (entry.duration? - f64::from(seconds)).abs();
            (gap <= f64::from(DURATION_TOLERANCE_SECS)).then_some((entry, gap))
        })
        // Prefer synced lyrics at equal distance.
        .min_by(|(a, gap_a), (b, gap_b)| {
            gap_a
                .total_cmp(gap_b)
                .then_with(|| b.synced.is_some().cmp(&a.synced.is_some()))
        })
        .map(|(entry, _)| entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_lines_parse_with_all_stamp_forms() {
        let lyrics = parse_synced(
            "[ar:Someone]\n[00:12.50] First\n[00:20.5]Second\n[01:02.123]Third\n[00:05.00][00:30.00]Chorus\n[02:00.00]",
        );
        assert!(lyrics.synced);
        let got: Vec<_> = lyrics
            .lines
            .iter()
            .map(|line| (line.start_ms, line.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (5_000, "Chorus"),
                (12_500, "First"),
                (20_500, "Second"),
                (30_000, "Chorus"),
                (62_123, "Third"),
            ]
        );
    }

    #[test]
    fn the_current_line_follows_the_position() {
        let lyrics = parse_synced("[00:10.00]a\n[00:20.00]b\n[00:30.00]c");
        assert_eq!(lyrics.current(0), None);
        assert_eq!(lyrics.current(9_999), None);
        assert_eq!(lyrics.current(10_000), Some(0));
        assert_eq!(lyrics.current(25_000), Some(1));
        assert_eq!(lyrics.current(10_000_000), Some(2));
        assert_eq!(lyrics.next_start_ms(None), Some(10_000));
        assert_eq!(lyrics.next_start_ms(Some(1)), Some(30_000));
        assert_eq!(lyrics.next_start_ms(Some(2)), None);
    }

    #[test]
    fn unsynced_lyrics_are_shown_but_never_followed() {
        let lyrics = parse_plain("\nOne\nTwo\n\nThree\n\n");
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 4);
        assert_eq!(lyrics.current(50_000), None);
        assert_eq!(lyrics.next_start_ms(None), None);
    }

    #[test]
    fn garbage_is_not_lyrics() {
        assert!(parse_synced("no stamps here\n[bad]x\n[xx:yy]z").is_empty());
        assert!(parse_synced("").is_empty());
    }

    #[test]
    fn the_nearest_length_wins_and_synced_breaks_ties() {
        let entry = |duration: f64, synced: bool| Entry {
            duration: Some(duration),
            instrumental: false,
            plain: Some("p".into()),
            synced: synced.then(|| "[00:01.00]s".to_owned()),
        };
        let entries = [entry(200.0, false), entry(181.0, false), entry(181.0, true)];
        let found = closest(&entries, 180).unwrap();
        assert!(found.synced.is_some());
        assert!(closest(&[entry(300.0, true)], 180).is_none());
    }
}
