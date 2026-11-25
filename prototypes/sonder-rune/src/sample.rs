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
