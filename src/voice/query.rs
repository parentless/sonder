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

/// How the word sounds, roughly: enough that "scene" and "seen", or "knight"
/// and "night", come out the same. Speech models spell by ear, so a word that
/// sounds right is often spelled as another word.
fn sound(word: &str) -> String {
    let mut text = word.to_owned();
    for (from, to) in [
        ("ph", "f"),
        ("ck", "k"),
        ("sce", "se"),
        ("sci", "si"),
        ("ce", "se"),
        ("ci", "si"),
        ("cy", "sy"),
        ("kn", "n"),
        ("wr", "r"),
        ("wh", "w"),
        ("gh", ""),
        ("c", "k"),
        ("q", "k"),
        ("z", "s"),
        ("x", "ks"),
        ("y", "i"),
    ] {
        text = text.replace(from, to);
    }
    // A silent final e, and doubled letters, say nothing.
    if text.len() > 3 && text.ends_with('e') && !text.ends_with("ee") {
        text.pop();
    }
    let mut out = String::new();
    for letter in text.chars() {
        if !out.ends_with(letter) {
            out.push(letter);
        }
    }
    out
}

fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(x != y));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// How alike two spoken words are: 1 for the same word, a little less for one
/// that sounds the same or is a letter or two off, 0 for different words.
/// Short words must match exactly, so "in" never stands for "on".
fn similarity(wanted: &str, have: &str) -> f32 {
    if wanted == have {
        return 1.;
    }
    let (a, b): (Vec<char>, Vec<char>) = (wanted.chars().collect(), have.chars().collect());
    if a.len() < 4 || b.len() < 4 {
        // "yt" and "dj" are real words in titles: only the same letters count.
        return 0.;
    }
    if sound(wanted) == sound(have) {
        return 0.9;
    }
    let apart = distance(&a, &b);
    let closeness = 1. - apart as f32 / a.len().max(b.len()) as f32;
    if closeness >= 0.74 {
        closeness * 0.9
    } else {
        0.
    }
}

/// How much of `wanted` is in `have`, as a fraction of `wanted`. A word that
/// was misheard but sounds alike counts for most of a word.
fn recall(wanted: &[String], have: &[String]) -> f32 {
    if wanted.is_empty() {
        return 0.;
    }
    let found: f32 = wanted
        .iter()
        .map(|word| {
            have.iter()
                .map(|other| similarity(word, other))
                .fold(0., f32::max)
        })
        .sum();
    found / wanted.len() as f32
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

/// What a library match must score before it is trusted over a search: every
/// word asked for found, and the artist too when one was named. Short of
/// that the request goes to Spotify.
const KNOWN_CONFIDENCE: f32 = 2.55;
/// The most that liking or often playing a song can add. Less than one
/// unwanted version word costs, so a better match always wins.
const MAX_FAMILIARITY: f32 = 0.6;

/// Songs the listener already has, in the order they matter.
pub(crate) struct Known<'a> {
    pub liked: &'a [Track],
    pub favorites: &'a [Track],
    pub recent: &'a [Track],
    pub local: &'a [Track],
}

/// The song in the listener's own library that the request clearly names,
/// preferring what they like and play most. Local files count: they play
/// without Spotify. `None` means "search instead".
pub(crate) fn pick_known(request: &Request, known: &Known<'_>) -> Option<Track> {
    use std::collections::HashMap;

    // The same song in several lists is one candidate that is more familiar.
    let mut familiarity: HashMap<String, (&Track, f32)> = HashMap::new();
    for (tracks, value) in [
        (known.liked, 0.25),
        (known.favorites, 0.2),
        (known.recent, 0.1),
        (known.local, 0.1),
    ] {
        for track in tracks {
            let key = track
                .spotify_uri
                .clone()
                .unwrap_or_else(|| format!("{:?}:{}", track.provider, track.source_id));
            familiarity.entry(key).or_insert((track, 0.)).1 += value;
        }
    }

    let mut best: Option<(&Track, f32)> = None;
    for (track, familiar) in familiarity.into_values() {
        let value = score(request, track);
        if value < KNOWN_CONFIDENCE {
            continue;
        }
        let value = value + familiar.min(MAX_FAMILIARITY);
        let better = best.is_none_or(|(_, top)| value > top + 0.001);
        if better {
            best = Some((track, value));
        }
    }
    best.map(|(track, _)| track.clone())
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
    fn a_song_the_listener_has_beats_a_search() {
        let request = parse("africa by toto").unwrap();
        let liked = vec![track("Africa", "Toto", "Toto IV")];
        let known = Known {
            liked: &liked,
            favorites: &[],
            recent: &[],
            local: &[],
        };
        assert_eq!(
            pick_known(&request, &known).map(|track| track.title),
            Some("Africa".into())
        );
    }

    #[test]
    fn a_vague_match_goes_to_search_instead() {
        let liked = vec![track("Africa", "Toto", "Toto IV")];
        let known = Known {
            liked: &liked,
            favorites: &[],
            recent: &[],
            local: &[],
        };
        // Right artist, wrong song.
        assert!(pick_known(&parse("rosanna by toto").unwrap(), &known).is_none());
        // Right song, wrong artist.
        assert!(pick_known(&parse("africa by weezer").unwrap(), &known).is_none());
        // Only part of the title.
        assert!(pick_known(&parse("africa the long version").unwrap(), &known).is_none());
    }

    #[test]
    fn what_is_liked_and_played_most_wins_between_equal_matches() {
        let request = parse("africa by toto").unwrap();
        let mut cover = track("Africa", "Toto", "Covers");
        cover.source_id = "cover".into();
        cover.spotify_uri = Some("spotify:track:cover".into());
        let original = track("Africa", "Toto", "Toto IV");
        let recent = vec![cover.clone(), cover.clone()];
        let liked = vec![original.clone()];
        let known = Known {
            liked: &liked,
            favorites: &[],
            recent: &recent,
            local: &[],
        };
        assert_eq!(
            pick_known(&request, &known).map(|track| track.source_id),
            Some(original.source_id.clone())
        );
        // Played a lot and never liked: familiarity still counts.
        let many = vec![cover.clone(); 4];
        let known = Known {
            liked: &[],
            favorites: &[],
            recent: &many,
            local: std::slice::from_ref(&original),
        };
        assert_eq!(
            pick_known(&request, &known).map(|track| track.source_id),
            Some("cover".into())
        );
    }

    #[test]
    fn a_local_file_can_be_the_match() {
        let request = parse("africa by toto").unwrap();
        let mut local = track("Africa", "Toto", "Toto IV");
        local.provider = Provider::Local;
        local.spotify_uri = None;
        let files = vec![local];
        let known = Known {
            liked: &[],
            favorites: &[],
            recent: &[],
            local: &files,
        };
        assert!(pick_known(&request, &known).is_some());
    }

    #[test]
    fn a_wanted_version_beats_a_familiar_one() {
        let request = parse("africa live by toto").unwrap();
        let studio = vec![track("Africa", "Toto", "Toto IV")];
        let live = vec![track("Africa - Live", "Toto", "Live")];
        let known = Known {
            liked: &studio,
            favorites: &[],
            recent: &[],
            local: &live,
        };
        assert_eq!(
            pick_known(&request, &known).map(|track| track.title),
            Some("Africa - Live".into())
        );
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

    #[test]
    fn a_misheard_word_still_finds_the_song_the_listener_has() {
        let liked = vec![
            track("Scene Dreams YT", "Some Artist", "Album"),
            track("Seen It All", "Other", "Album"),
        ];
        let known = Known {
            liked: &liked,
            favorites: &[],
            recent: &[],
            local: &[],
        };
        let title = |said: &str| pick_known(&parse(said).unwrap(), &known).map(|track| track.title);
        // Heard "seen" for "scene", and heard only the end of the name.
        assert_eq!(title("seen dreams yt"), Some("Scene Dreams YT".into()));
        assert_eq!(title("dreams yt"), Some("Scene Dreams YT".into()));
        assert_eq!(title("play seen dreams yt"), Some("Scene Dreams YT".into()));
    }

    #[test]
    fn words_that_only_sound_alike_in_part_are_not_the_same() {
        assert!(similarity("scene", "seen") > 0.85);
        assert!(similarity("knight", "night") > 0.85);
        assert_eq!(similarity("son", "seen"), 0.);
        assert_eq!(similarity("in", "on"), 0.);
        assert_eq!(similarity("love", "hate"), 0.);
    }
}
