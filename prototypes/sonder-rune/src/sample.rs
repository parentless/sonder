//! Sample data for pictures taken without Spotify (`SONDER_OFFLINE=1 ... --snap`).

use cadence::model::{Playlist, Provider, Track};

use crate::{Conn, Page, Source, State};

pub fn track(i: usize) -> Track {
    const ARTISTS: [&str; 6] = [
        "Harry Styles",
        "Radiohead",
        "Phoebe Bridgers",
        "Frank Ocean",
        "Tame Impala",
        "Mitski",
    ];
    const WORDS: [&str; 8] = [
        "Midnight", "Static", "Honey", "Parade", "Glass", "Harbor", "Velvet", "Orbit",
    ];
    Track {
        provider: Provider::Spotify,
        source_id: format!("sample{i}"),
        spotify_uri: Some(format!("spotify:track:sample{i}")),
        isrc: None,
        title: format!("{} {}", WORDS[i % 8], WORDS[(i / 8) % 8]).into(),
        artist: ARTISTS[i % 6].into(),
        artists: Vec::new(),
        album: format!("{} Sessions", WORDS[(i / 5) % 8]).into(),
        album_ref: None,
        duration_ms: 150_000 + (i as u32 * 37_000) % 150_000,
        artwork_url: None,
    }
}

fn base(s: &mut State) {
    let tracks: Vec<Track> = (0..24).map(track).collect();
    s.liked_ids = tracks
        .iter()
        .step_by(3)
        .map(|t| t.source_id.clone())
        .collect();
    s.liked = Source {
        total: 4210,
        done: true,
        tracks: tracks.clone().into(),
        error: None,
    };
    s.recent = Source {
        total: 8,
        done: true,
        tracks: tracks[..8].to_vec().into(),
        error: None,
    };
    s.playlists = (0..9)
        .map(|i| Playlist {
            provider: Provider::Spotify,
            source_id: format!("pl{i}"),
            name: [
                "Late night drive",
                "Focus",
                "Sunday morning",
                "Gym",
                "Road trip",
                "Rainy days",
                "Throwbacks",
                "Deep work",
                "New finds",
            ][i]
                .into(),
            owner: "you".into(),
            track_count: 20 + i as u32 * 7,
            artwork_url: None,
        })
        .collect();
    s.profile = Some(cadence::model::UserProfile {
        display_name: "Trent".into(),
        artwork_url: None,
        id: "trent".into(),
    });
    s.pinned = ["pl3".to_owned()].into();
    s.loaded = true;
    s.conn = Conn::Ready;
    s.now = Some(track(2));
    s.playing = true;
    s.anchor_ms = 64_000;
    s.up_next = (3..8).map(track).collect();
    s.refresh();
}

pub const SCENES: &[(&str, fn(&mut State))] = &[
    ("liked", base),
    ("rail", |s| {
        base(s);
        s.wide = false;
    }),
    ("playlists", |s| {
        base(s);
        s.page = Page::Playlists;
        s.refresh();
    }),
    ("playlist", |s| {
        base(s);
        let id = "pl1".to_string();
        let tracks: Vec<Track> = (8..30).map(track).collect();
        s.opened.insert(
            id.clone(),
            Source {
                total: 22,
                done: true,
                tracks: tracks.into(),
                error: None,
            },
        );
        s.page = Page::Playlist(id);
        s.refresh();
    }),
    ("search", |s| {
        base(s);
        s.query = rune::TextEdit::new("glass");
        s.refresh();
        s.on_search_input();
        s.search.live = Some(("glass".into(), live()));
    }),
    ("menu", |s| {
        base(s);
        s.menu = true;
    }),
    ("settings", |s| {
        base(s);
        s.page = Page::Settings;
        s.prefs.equalizer.enabled = true;
        s.prefs.equalizer.steps = [12, 6, 0, -4, 8];
    }),
    ("album", |s| {
        base(s);
        let tracks: Vec<Track> = (0..11).map(track).collect();
        s.details.insert(
            "al".into(),
            crate::Detail {
                title: "Midnight Sessions".into(),
                sub: "Tame Impala · 2020 · 11 songs".into(),
                art: None,
                round: false,
            },
        );
        s.opened.insert(
            "album:al".into(),
            Source {
                total: 11,
                done: true,
                tracks: tracks.into(),
                error: None,
            },
        );
        s.page = Page::Album("al".into());
        s.refresh();
    }),
    ("home", |s| {
        base(s);
        s.page = Page::Home;
        s.home.data = Some(cadence::model::HomeData {
            artists: (0..8)
                .map(|i| cadence::model::Artist {
                    provider: Provider::Spotify,
                    source_id: format!("ar{i}"),
                    spotify_uri: None,
                    name: [
                        "Tame Impala",
                        "Radiohead",
                        "Frank Ocean",
                        "Mitski",
                        "Harry Styles",
                        "Phoebe Bridgers",
                        "Glass Animals",
                        "Beach House",
                    ][i]
                        .into(),
                    artwork_url: None,
                    genres: Vec::new(),
                })
                .collect(),
            tracks: (3..9).map(track).collect(),
        });
    }),
    ("lyrics", |s| {
        base(s);
        s.lyrics.open = true;
        s.lyrics.id = s.now.as_ref().map(|n| n.source_id.clone());
        let words = [
            "Static on the radio",
            "Honey on the glass",
            "I was never one for waiting",
            "But I waited for it all",
            "Midnight in the harbour",
            "Orbit of a velvet sound",
            "Hold the line a little longer",
            "Let the quiet carry us",
            "Nothing left to say",
        ];
        s.lyrics.data = Some(cadence::lyrics::Lyrics {
            synced: true,
            lines: words
                .iter()
                .enumerate()
                .map(|(i, w)| cadence::lyrics::Line {
                    start_ms: i as u32 * 8000,
                    text: (*w).into(),
                })
                .collect(),
        });
        s.lyrics.line = Some(8.min(7));
        s.anchor_ms = 7 * 8000 + 1000;
    }),
    ("credits", |s| {
        base(s);
        s.credits = Some(crate::CreditsState {
            track: track(2),
            loading: false,
            data: Some(cadence::credits::Credits {
                roles: vec![
                    cadence::credits::Role {
                        label: "Producer".into(),
                        people: vec![cadence::credits::Person {
                            name: "Kevin Parker".into(),
                            detail: None,
                        }],
                    },
                    cadence::credits::Role {
                        label: "Performers".into(),
                        people: vec![
                            cadence::credits::Person {
                                name: "Dominic Simper".into(),
                                detail: Some("bass".into()),
                            },
                            cadence::credits::Person {
                                name: "Jay Watson".into(),
                                detail: Some("keys".into()),
                            },
                        ],
                    },
                ],
            }),
        });
    }),
    ("songmenu", |s| {
        base(s);
        s.page = Page::Liked;
        s.track_menu = Some(crate::TrackMenu {
            track: track(3),
            y: 260.0,
        });
    }),
    ("nowplaying", |s| {
        base(s);
        s.np = true;
    }),
    ("login", |s| {
        s.conn = Conn::Login;
    }),
    ("setup", |s| {
        s.conn = Conn::Setup;
    }),
];

/// What Spotify might answer for the search picture.
fn live() -> cadence::model::CatalogSearchResults {
    use cadence::model::{Album, Artist, CatalogSearchResults, Provider};
    let artist = |n: &str| Artist {
        provider: Provider::Spotify,
        source_id: n.into(),
        spotify_uri: None,
        name: n.to_owned().into(),
        artwork_url: None,
        genres: Vec::new(),
    };
    CatalogSearchResults {
        artists: vec![artist("Glass Animals"), artist("Glassjaw")],
        albums: vec![Album {
            provider: Provider::Spotify,
            source_id: "zabba".into(),
            spotify_uri: None,
            name: "Zabba".to_owned().into(),
            artists: Vec::new(),
            release_date: Some("2020".into()),
            artwork_url: None,
            track_count: Some(11),
        }],
        tracks: (20..24).map(track).collect(),
    }
}
