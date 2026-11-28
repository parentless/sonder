//! Turns what was said into a Spotify search, and picks the song it meant from
//! what Spotify found.

use crate::model::Track;

/// What the listener asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Request {
    pub title: String,
    pub artist: Option<String>,
}

impl Request {
    /// The words sent to Spotify's search.
    pub(crate) fn search_text(&self) -> String {
        match &self.artist {
            Some(artist) => format!("{} {artist}", self.title),
            None => self.title.clone(),
        }
    }
}

/// Things people say before the song's name.
const LEAD_INS: &[&str] = &[
    "can you please play",
    "could you please play",
    "can you play",
    "could you play",
    "please play",
    "i want to hear",
    "i wanna hear",
    "i want to listen to",
    "i wanna listen to",
    "put on",
    "play me",
    "play",
    "listen to",
    "start",
];

/// Things people say after it.
const LEAD_OUTS: &[&str] = &["please", "thanks", "thank you", "for me"];

/// Words that mark a different recording of a song. They count against a
/// result unless they were asked for.
const VERSION_WORDS: &[&str] = &[
    "karaoke",
    "instrumental",
    "tribute",
    "cover",
    "slowed",
    "sped",
    "remix",
    "acoustic",
    "live",
    "lullaby",
    "piano",
    "8d",
    "nightcore",
];

/// Removes what a speech model writes for sounds that are not words:
/// `[BLANK_AUDIO]`, `(music)`, `*sigh*` and note symbols.
fn strip_noise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut closing: Option<char> = None;
    for character in text.chars() {
        match (closing, character) {
            (Some(end), c) if c == end => closing = None,
            (Some(_), _) => {}
            (None, '[') => closing = Some(']'),
            (None, '(') => closing = Some(')'),
            (None, '*') => closing = Some('*'),
            (None, '♪' | '♫' | '♬') => out.push(' '),
            (None, c) => out.push(c),
        }
    }
    out
}

fn trim_punctuation(text: &str) -> &str {
    text.trim_matches(|c: char| !c.is_alphanumeric())
}

/// Reads "play blinding lights by the weeknd" as a title and an artist.
/// `None` when nothing that sounds like a song was said.
pub(crate) fn parse(transcript: &str) -> Option<Request> {
    let lowered = strip_noise(transcript).to_lowercase();
    let mut text = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    text = trim_punctuation(&text).to_owned();
    for lead in LEAD_INS {
        if let Some(rest) = text.strip_prefix(lead)
            && (rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric()))
        {
            text = trim_punctuation(rest).to_owned();
            break;
        }
    }
    for out in LEAD_OUTS {
        if let Some(rest) = text.strip_suffix(out)
            && rest.ends_with(|c: char| !c.is_alphanumeric())
        {
            text = trim_punctuation(rest).to_owned();
        }
    }
    if !text.chars().any(char::is_alphanumeric) {
        return None;
    }
    // The last " by " splits the title from the artist, so a title that
    // contains "by" ("Stand By Me by Ben E. King") still reads correctly.
    if let Some(split) = text.rfind(" by ") {
        let title = trim_punctuation(&text[..split]);
        let artist = trim_punctuation(&text[split + 4..]);
        if title.chars().any(char::is_alphanumeric) && artist.chars().any(char::is_alphanumeric) {
            return Some(Request {
                title: title.to_owned(),
                artist: Some(artist.to_owned()),
            });
        }
    }
    Some(Request {
        title: text,
        artist: None,
    })
}

/// Lower-case words without punctuation. An apostrophe joins its word:
/// "don't" is "dont", as people say it.
fn words(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut word = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
        } else if matches!(character, '\'' | '’') {
            continue;
        } else if !word.is_empty() {
            found.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        found.push(word);
    }
    found
}

/// How many of `wanted` appear in `have`, as a fraction of `wanted`.
fn recall(wanted: &[String], have: &[String]) -> f32 {
    if wanted.is_empty() {
        return 0.;
    }
    let found = wanted.iter().filter(|word| have.contains(word)).count();
    found as f32 / wanted.len() as f32
}

fn score(request: &Request, track: &Track) -> f32 {
    let title = words(&track.title);
    let wanted_title = words(&request.title);
    let mut artists: Vec<Vec<String>> = track
        .artists
        .iter()
        .map(|artist| words(&artist.name))
        .collect();
    if artists.is_empty() {
        artists.push(words(&track.artist));
    }
    let everything: Vec<String> = title
        .iter()
        .cloned()
        .chain(artists.iter().flatten().cloned())
        .collect();

    let mut total = match &request.artist {
        Some(artist) => {
            let wanted_artist = words(artist);
            let artist_match = artists
                .iter()
                .map(|have| recall(&wanted_artist, have))
                .fold(0., f32::max);
            // Of the words asked for as the title, how many the title has,
            // with a little against titles that carry a lot of extra words.
            let precision = if title.is_empty() {
                0.
            } else {
                wanted_title
                    .iter()
                    .filter(|word| title.contains(word))
                    .count() as f32
                    / title.len() as f32
            };
            2. * (0.8 * recall(&wanted_title, &title) + 0.2 * precision) + artist_match
        }
        // Without an artist the words may be either, as in "taylor swift
        // shake it off".
        None => 3. * recall(&wanted_title, &everything),
    };
    let requested = words(&request.search_text());
    let album = words(&track.album);
    for version in VERSION_WORDS {
        let present = title.iter().chain(album.iter()).any(|word| word == version);
        if present && !requested.iter().any(|word| word == version) {
            total -= 0.9;
        }
    }
    total
}

/// The index of the song in `tracks` that best matches the request. Spotify
/// ranks its results, so a tie keeps the earlier one.
pub(crate) fn pick(request: &Request, tracks: &[Track]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, track) in tracks.iter().enumerate() {
        // Local files and anything without a Spotify address cannot stream.
        if track.spotify_uri.is_none() {
            continue;
        }
        let value = score(request, track);
        if best.is_none_or(|(_, top)| value > top + 0.001) {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtistRef, Provider};

    fn track(title: &str, artist: &str, album: &str) -> Track {
        Track {
            provider: Provider::Spotify,
            source_id: format!("{title}{artist}"),
            spotify_uri: Some(format!("spotify:track:{title}{artist}")),
            isrc: None,
            title: title.to_owned().into(),
            artist: artist.to_owned().into(),
            artists: vec![ArtistRef {
                name: artist.to_owned().into(),
                source_id: None,
                spotify_uri: None,
            }],
            album: album.to_owned().into(),
            album_ref: None,
            duration_ms: 200_000,
            artwork_url: None,
        }
    }

    #[test]
    fn a_title_and_artist_are_split_on_by() {
        let request = parse("Blinding Lights by The Weeknd.").unwrap();
        assert_eq!(request.title, "blinding lights");
        assert_eq!(request.artist.as_deref(), Some("the weeknd"));
        assert_eq!(request.search_text(), "blinding lights the weeknd");
    }

    #[test]
    fn lead_ins_and_please_are_dropped() {
        for text in [
            "Play Blinding Lights by The Weeknd",
            "play blinding lights by the weeknd, please.",
            "Can you play Blinding Lights by The Weeknd?",
            "put on blinding lights by the weeknd",
        ] {
            let request = parse(text).unwrap();
            assert_eq!(request.title, "blinding lights", "{text}");
            assert_eq!(request.artist.as_deref(), Some("the weeknd"), "{text}");
        }
    }

    #[test]
    fn the_last_by_splits_so_titles_with_by_survive() {
        let request = parse("Stand By Me by Ben E. King").unwrap();
        assert_eq!(request.title, "stand by me");
        assert_eq!(request.artist.as_deref(), Some("ben e. king"));
    }

    #[test]
    fn a_title_alone_has_no_artist() {
        let request = parse("Play Mr. Brightside").unwrap();
        assert_eq!(request.title, "mr. brightside");
        assert_eq!(request.artist, None);
    }

    #[test]
    fn a_word_that_only_starts_like_a_lead_in_is_kept() {
        // "player" is not "play" + "er".
        assert_eq!(parse("Player by Chic").unwrap().title, "player");
        assert_eq!(parse("Starlight by Muse").unwrap().title, "starlight");
    }

    #[test]
    fn sounds_that_are_not_words_are_ignored() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("  [BLANK_AUDIO] "), None);
        assert_eq!(parse("(upbeat music)"), None);
        assert_eq!(parse("♪ ♪"), None);
        assert_eq!(parse("play"), None);
        let request = parse("[MUSIC] play Africa by Toto").unwrap();
        assert_eq!(request.title, "africa");
    }

    #[test]
    fn a_dangling_by_is_not_an_artist() {
        let request = parse("play something by").unwrap();
        assert_eq!(request.artist, None);
        assert_eq!(request.title, "something by");
    }

    #[test]
    fn the_matching_song_wins_over_a_louder_result() {
        let request = parse("blinding lights by the weeknd").unwrap();
        let tracks = vec![
            track("Blinding Lights - Karaoke", "Karaoke Hits", "Karaoke"),
            track("Starboy", "The Weeknd", "Starboy"),
            track("Blinding Lights", "The Weeknd", "After Hours"),
        ];
        assert_eq!(pick(&request, &tracks), Some(2));
    }

    #[test]
    fn the_right_artist_wins_between_two_songs_with_one_title() {
        let request = parse("hurt by johnny cash").unwrap();
        let tracks = vec![
            track("Hurt", "Nine Inch Nails", "The Downward Spiral"),
            track("Hurt", "Johnny Cash", "American IV"),
        ];
        assert_eq!(pick(&request, &tracks), Some(1));
    }

    #[test]
    fn a_tie_keeps_spotifys_order() {
        let request = parse("africa").unwrap();
        let tracks = vec![
            track("Africa", "Toto", "Toto IV"),
            track("Africa", "Toto", "Greatest Hits"),
        ];
        assert_eq!(pick(&request, &tracks), Some(0));
    }

    #[test]
    fn a_version_is_chosen_only_when_asked_for() {
        let tracks = vec![
            track("Yesterday - Live", "The Beatles", "Live"),
            track("Yesterday", "The Beatles", "Help!"),
        ];
        let plain = parse("yesterday by the beatles").unwrap();
        assert_eq!(pick(&plain, &tracks), Some(1));
        let live = parse("yesterday live by the beatles").unwrap();
        assert_eq!(pick(&live, &tracks), Some(0));
    }

    #[test]
    fn words_may_be_the_artist_when_no_by_was_said() {
        let request = parse("taylor swift shake it off").unwrap();
        let tracks = vec![
            track("Shake It Off", "Some Cover Band", "Covers"),
            track("Shake It Off", "Taylor Swift", "1989"),
        ];
        assert_eq!(pick(&request, &tracks), Some(1));
    }

    #[test]
    fn apostrophes_do_not_split_words() {
        let request = parse("don't stop me now by queen").unwrap();
        let tracks = vec![track("Don't Stop Me Now", "Queen", "Jazz")];
        assert_eq!(pick(&request, &tracks), Some(0));
        assert_eq!(words("Don’t"), vec!["dont"]);
    }

    #[test]
    fn songs_that_cannot_stream_are_skipped() {
        let request = parse("africa by toto").unwrap();
        let mut local = track("Africa", "Toto", "Toto IV");
        local.spotify_uri = None;
        assert_eq!(pick(&request, &[local.clone()]), None);
        assert_eq!(
            pick(&request, &[local, track("Africa", "Toto", "IV")]),
            Some(1)
        );
        assert_eq!(pick(&request, &[]), None);
    }
}
