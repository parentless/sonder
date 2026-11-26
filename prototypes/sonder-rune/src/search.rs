//! Search: instant matches from the songs already loaded, then Spotify's catalog
//! once the typing pauses. Mirrors Sonder's toolbar search (3 letters, short pause).

use cadence::model::{CatalogSearchResults, Track};

use crate::State;

/// Letters before Spotify is asked, and how long typing must pause (seconds).
pub const MIN_CHARS: usize = 3;
pub const PAUSE: f64 = 0.45;
const LIBRARY_LIMIT: usize = 3;

#[derive(Default)]
pub struct Search {
    /// Whether the dropdown is up.
    pub open: bool,
    /// Matches from the loaded songs, shown at once.
    pub library: Vec<Track>,
    /// Spotify's answer for `.0`, once it arrives.
    pub live: Option<(String, CatalogSearchResults)>,
    pub loading: bool,
    /// When the pause ends and Spotify gets asked.
    pub due: Option<f64>,
}

impl Search {
    pub fn query_matches(&self, query: &str) -> bool {
        self.live.as_ref().is_some_and(|(q, _)| q == query)
    }
}

/// Songs among `pools` whose title, artist or album holds every word of `query`.
pub fn find(query: &str, pools: &[&[Track]]) -> Vec<Track> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .take(8)
        .map(str::to_owned)
        .collect();
    let mut found: Vec<Track> = Vec::new();
    if words.is_empty() {
        return found;
    }
    for track in pools.iter().flat_map(|p| p.iter()) {
        if found.len() == LIBRARY_LIMIT {
            break;
        }
        let hay = format!("{} {} {}", track.title, track.artist, track.album).to_lowercase();
        if words.iter().all(|w| hay.contains(w.as_str()))
            && !found.iter().any(|f| f.source_id == track.source_id)
        {
            found.push(track.clone());
        }
    }
    found
}

impl State {
    /// The field changed: refresh the instant matches and start the pause timer.
    pub fn on_search_input(&mut self) {
        let query = self.query.text.trim().to_owned();
        self.search.open = !query.is_empty();
        self.search.library = find(&query, &[&self.liked.tracks, &self.recent.tracks]);
        if query.chars().count() >= MIN_CHARS {
            if !self.search.query_matches(&query) {
                self.search.due = Some(self.clock + PAUSE);
            }
        } else {
            self.search.due = None;
            self.search.live = None;
            self.search.loading = false;
        }
    }

    /// Asks Spotify once the pause is over. Returns whether it is still waiting.
    pub fn tick_search(&mut self) -> bool {
        let Some(due) = self.search.due else {
            return false;
        };
        if self.clock < due {
            return true;
        }
        self.search.due = None;
        let query = self.query.text.trim().to_owned();
        if let Some(bridge) = &self.bridge {
            bridge.search(query);
            self.search.loading = true;
        }
        false
    }

    pub fn on_search_results(&mut self, query: String, results: Option<CatalogSearchResults>) {
        if query != self.query.text.trim() {
            return;
        }
        self.search.loading = false;
        match results {
            Some(results) => self.search.live = Some((query, results)),
            None => self.message = Some("Spotify search did not answer.".into()),
        }
    }

    /// Plays `tracks[index]` from the dropdown and closes it.
    pub fn play_found(&mut self, tracks: Vec<Track>, index: usize) {
        self.search.open = false;
        self.send(cadence::backend::BackendCommand::PlayContext {
            tracks: tracks.into(),
            index,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(n: usize, title: &str) -> Track {
        let mut t = crate::sample::track(n);
        t.title = title.to_owned().into();
        t
    }

    #[test]
    fn every_word_must_match_and_repeats_are_dropped() {
        let a = [song(1, "Glass Houses"), song(2, "Other")];
        let b = [song(1, "Glass Houses")];
        let found = find("glass house", &[&a, &b]);
        assert_eq!(found.len(), 1);
        assert!(find("", &[&a]).is_empty());
    }

    #[test]
    fn spotify_is_asked_only_after_three_letters_and_a_pause() {
        let mut s = State::default();
        s.clock = 10.0;
        s.query = rune::TextEdit::new("gl");
        s.on_search_input();
        assert!(s.search.open && s.search.due.is_none());
        s.query = rune::TextEdit::new("glass");
        s.on_search_input();
        assert_eq!(s.search.due, Some(10.0 + PAUSE));
        assert!(s.tick_search()); // still waiting
        s.clock = 11.0;
        assert!(!s.tick_search() && s.search.due.is_none());
    }

    #[test]
    fn a_late_answer_for_old_words_is_ignored() {
        let mut s = State::default();
        s.query = rune::TextEdit::new("glass");
        s.on_search_results("gla".into(), Some(Default::default()));
        assert!(s.search.live.is_none());
        s.on_search_results("glass".into(), Some(Default::default()));
        assert!(s.search.live.is_some());
    }
}
