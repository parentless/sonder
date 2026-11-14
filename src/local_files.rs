//! Music files from folders the listener chose, played like Spotify plays
//! local files: each file becomes a track with a `spotify:local:` URI, which
//! librespot's player loads from disk instead of from Spotify.
//!
//! A scan only reads files that are new or changed since the last one; the
//! rest come from the saved index after a single `stat`.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow};
use symphonia::core::{
    codecs::DecoderOptions,
    formats::{FormatOptions, FormatReader},
    io::MediaSourceStream,
    meta::{MetadataOptions, MetadataRevision, StandardTagKey, Value},
    probe::Hint,
    units::TimeBase,
};

use gpui_kit::SharedString;

use crate::model::{AlbumRef, ArtistRef, Provider, Track};

/// File types the player can decode: MP3, FLAC, Ogg Vorbis, AAC or ALAC in
/// MP4, and WAV.
const EXTENSIONS: [&str; 7] = ["mp3", "flac", "ogg", "oga", "m4a", "mp4", "wav"];

/// The most tracks one library holds, so a folder pointed at a whole drive
/// cannot grow the index without bound.
pub const MAX_TRACKS: usize = 20_000;

/// Artwork URLs with this prefix name a local file whose embedded picture is
/// the cover. The image cache reads them from disk instead of the network.
pub const ARTWORK_SCHEME: &str = "sonder-local-art:";

/// One file as the last scan found it. `track` is `None` for a file that
/// could not be read, so it is not probed again until it changes.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub path: String,
    pub modified_ms: i64,
    pub size: u64,
    pub sort_key: String,
    pub track: Option<Track>,
}

/// What the Local Files page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalLibrary {
    pub tracks: Vec<Track>,
    /// Music files that were found but could not be read.
    pub unreadable: usize,
    /// More music was found than [`MAX_TRACKS`].
    pub truncated: bool,
}

impl LocalLibrary {
    /// Builds the page's list from index entries already in display order.
    pub fn from_index(entries: Vec<IndexEntry>) -> Self {
        let mut library = Self::default();
        // The cover of the album the previous track came from, by folder and
        // album name.
        let mut album_cover: Option<(PathBuf, SharedString, String)> = None;
        for entry in entries {
            match entry.track {
                Some(mut track) if library.tracks.len() < MAX_TRACKS => {
                    share_album_cover(&mut track, &mut album_cover);
                    library.tracks.push(track);
                }
                Some(_) => library.truncated = true,
                None => library.unreadable += 1,
            }
        }
        library
    }
}

/// Points every track of an album, in one folder, at the first track's
/// embedded cover. The album's songs then share one picture in the artwork
/// cache instead of decoding the same cover once per song.
fn share_album_cover(track: &mut Track, album_cover: &mut Option<(PathBuf, SharedString, String)>) {
    let Some(artwork) = track.artwork_url.as_mut() else {
        return;
    };
    let folder = Path::new(&track.source_id)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    match album_cover {
        Some((cover_folder, album, cover))
            if !track.album.is_empty() && *cover_folder == folder && *album == track.album =>
        {
            artwork.clone_from(cover);
        }
        _ => *album_cover = Some((folder, track.album.clone(), artwork.clone())),
    }
}

/// Walks `folders` and returns the new index in display order, reusing
/// `previous` entries for files whose size and modification time match.
/// Returns `None` when `cancelled` reports that nobody wants the answer.
pub fn scan(
    folders: &[PathBuf],
    previous: Vec<IndexEntry>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<IndexEntry>> {
    let mut previous: HashMap<String, IndexEntry> = previous
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for path in music_files(folders, cancelled)? {
        if cancelled() {
            return None;
        }
        let Some(key) = path.to_str().map(str::to_owned) else {
            // The player finds files by this text; a path that is not valid
            // Unicode could not be found again.
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let size = metadata.len();
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| {
                i64::try_from(time.as_millis()).unwrap_or(i64::MAX)
            });
        if let Some(entry) = previous.remove(&key)
            && entry.size == size
            && entry.modified_ms == modified_ms
        {
            entries.push(entry);
            continue;
        }
        let (track, sort_key) = match read_track(&path, &key) {
            Ok(read) => (Some(read.0), read.1),
            Err(error) => {
                log::info!("local files: skipped {key}: {error:#}");
                (None, sort_key_for_path(&key))
            }
        };
        entries.push(IndexEntry {
            path: key,
            modified_ms,
            size,
            sort_key,
            track,
        });
    }
    entries.sort_by(|left, right| left.sort_key.cmp(&right.sort_key));
    Some(entries)
}

/// Every music file under `folders`, without following links or entering
/// hidden folders, stopping at a generous multiple of [`MAX_TRACKS`].
fn music_files(folders: &[PathBuf], cancelled: &dyn Fn() -> bool) -> Option<Vec<PathBuf>> {
    let limit = MAX_TRACKS * 2;
    let mut files = Vec::new();
    let mut pending: Vec<PathBuf> = folders.to_vec();
    let mut visited = HashSet::new();
    while let Some(folder) = pending.pop() {
        if cancelled() {
            return None;
        }
        if !visited.insert(folder.clone()) {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if !is_hidden(&path) {
                    pending.push(path);
                }
            } else if kind.is_file() && is_music_file(&path) {
                files.push(path);
                if files.len() >= limit {
                    return Some(files);
                }
            }
        }
    }
    Some(files)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

pub fn is_music_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// The tags Sonder shows and sorts by.
#[derive(Default)]
struct Tags {
    title: Option<String>,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    track_number: Option<u32>,
    disc_number: Option<u32>,
    has_artwork: bool,
}

impl Tags {
    /// Later revisions fill in only what earlier ones left out.
    fn read(&mut self, revision: &MetadataRevision) {
        for tag in revision.tags() {
            let Some(key) = tag.std_key else {
                continue;
            };
            let text = || tag_text(&tag.value);
            match key {
                StandardTagKey::TrackTitle => fill(&mut self.title, text()),
                StandardTagKey::Artist => fill(&mut self.artist, text()),
                StandardTagKey::AlbumArtist => fill(&mut self.album_artist, text()),
                StandardTagKey::Album => fill(&mut self.album, text()),
                StandardTagKey::TrackNumber => {
                    fill(&mut self.track_number, number(&tag.value));
                }
                StandardTagKey::DiscNumber => fill(&mut self.disc_number, number(&tag.value)),
                _ => {}
            }
        }
        self.has_artwork |= !revision.visuals().is_empty();
    }
}

fn fill<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn tag_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        Value::UnsignedInt(number) => number.to_string(),
        Value::SignedInt(number) => number.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// Track and disc numbers, including the `3/12` form.
fn number(value: &Value) -> Option<u32> {
    match value {
        Value::UnsignedInt(number) => u32::try_from(*number).ok(),
        Value::SignedInt(number) => u32::try_from(*number).ok(),
        Value::String(text) => text.split('/').next()?.trim().parse().ok(),
        _ => None,
    }
}

/// Reads one file's tags and length, and checks that it can be decoded.
fn read_track(path: &Path, key: &str) -> Result<(Track, String)> {
    let mut format = open(path)?;
    let mut tags = Tags::default();
    if let Some(revision) = format.1.as_ref() {
        tags.read(revision);
    }
    let reader = &mut format.0;
    if let Some(revision) = reader.metadata().current() {
        tags.read(revision);
    }
    let params = reader
        .default_track()
        .context("the file has no audio")?
        .codec_params
        .clone();
    symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .context("the audio format is not supported")?;
    let time_base = params
        .time_base
        .or_else(|| params.sample_rate.map(|rate| TimeBase::new(1, rate)))
        .context("the file has no timing")?;
    let frames = match params.n_frames {
        Some(frames) => frames,
        None => count_frames(reader.as_mut()),
    };
    let duration = Duration::from(time_base.calc_time(frames));
    let duration_ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
    if duration_ms == 0 {
        return Err(anyhow!("the file has no length"));
    }
    Ok(track_from_tags(key, tags, duration_ms))
}

/// The probed format, and any tags found in front of the container (ID3v2).
type Probed = (Box<dyn FormatReader>, Option<MetadataRevision>);

fn open(path: &Path) -> Result<Probed> {
    let file = File::open(path).context("could not open the file")?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("not a music file Sonder can read")?;
    let outer = probed
        .metadata
        .get()
        .and_then(|metadata| metadata.current().cloned());
    Ok((probed.format, outer))
}

/// A length for files whose header does not state one, such as MP3s without
/// a Xing header: the packet durations, read without decoding.
fn count_frames(reader: &mut dyn FormatReader) -> u64 {
    let mut frames = 0u64;
    while let Ok(packet) = reader.next_packet() {
        frames = frames.saturating_add(packet.dur);
    }
    frames
}

fn track_from_tags(key: &str, tags: Tags, duration_ms: u32) -> (Track, String) {
    let stem = Path::new(key)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(key)
        .trim();
    // Untagged downloads are often named "Artist - Title".
    let (named_artist, named_title) = match stem.split_once(" - ") {
        Some((artist, title)) if tags.title.is_none() && !artist.trim().is_empty() => {
            (Some(artist.trim().to_owned()), title.trim().to_owned())
        }
        _ => (None, stem.to_owned()),
    };
    let title = tags.title.unwrap_or(named_title);
    let artist = tags
        .artist
        .or_else(|| tags.album_artist.clone())
        .or(named_artist)
        .unwrap_or_else(|| "Unknown artist".to_owned());
    let album = tags.album.unwrap_or_default();
    let sort_key = format!(
        "{}\u{1}{}\u{1}{:04}\u{1}{:04}\u{1}{}\u{1}{}",
        tags.album_artist
            .as_deref()
            .unwrap_or(&artist)
            .to_lowercase(),
        album.to_lowercase(),
        tags.disc_number.unwrap_or(0),
        tags.track_number.unwrap_or(0),
        title.to_lowercase(),
        key.to_lowercase(),
    );
    let spotify_uri = local_uri(&artist, &album, &title, duration_ms);
    let track = Track {
        provider: Provider::Local,
        source_id: key.to_owned(),
        spotify_uri: Some(spotify_uri),
        isrc: None,
        title: title.into(),
        artist: artist.clone().into(),
        artists: vec![ArtistRef {
            name: artist.into(),
            source_id: None,
            spotify_uri: None,
        }],
        album_ref: (!album.is_empty()).then(|| AlbumRef {
            name: album.clone().into(),
            ..AlbumRef::default()
        }),
        album: album.into(),
        duration_ms,
        artwork_url: tags.has_artwork.then(|| format!("{ARTWORK_SCHEME}{key}")),
    };
    (track, sort_key)
}

/// Unreadable files sort by name, after nothing in particular.
fn sort_key_for_path(key: &str) -> String {
    format!("\u{2}{}", key.to_lowercase())
}

/// Spotify's URI for a local file:
/// `spotify:local:{artist}:{album}:{title}:{seconds}`, each part form-encoded.
pub fn local_uri(artist: &str, album: &str, title: &str, duration_ms: u32) -> String {
    let encode =
        |part: &str| url::form_urlencoded::byte_serialize(part.as_bytes()).collect::<String>();
    format!(
        "spotify:local:{}:{}:{}:{}",
        encode(artist),
        encode(album),
        encode(title),
        duration_ms / 1000
    )
}

pub fn is_local_uri(uri: &str) -> bool {
    uri.starts_with("spotify:local:")
}

/// Watches the Local Files folders and calls `changed` when something in them
/// that could be music is added, removed, renamed or rewritten. It holds one
/// operating-system watch per folder and stops when dropped.
pub struct FolderWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl FolderWatcher {
    /// `None` when no folder could be watched; Local Files then still
    /// rescans whenever it is opened.
    pub fn new(folders: &[PathBuf], changed: impl Fn() + Send + 'static) -> Option<Self> {
        use notify::Watcher as _;
        let roots = folders.to_vec();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if !matches!(event.kind, notify::EventKind::Access(_))
                    && event.paths.iter().any(|path| worth_a_rescan(path, &roots))
                {
                    changed();
                }
            })
            .inspect_err(|error| log::warn!("local files: cannot watch folders: {error}"))
            .ok()?;
        let mut watching = 0;
        for folder in folders {
            match watcher.watch(folder, notify::RecursiveMode::Recursive) {
                Ok(()) => watching += 1,
                Err(error) => {
                    log::warn!("local files: cannot watch {}: {error}", folder.display())
                }
            }
        }
        (watching > 0).then_some(Self { _watcher: watcher })
    }
}

/// Everything but other kinds of files (covers, lyrics, part-downloaded
/// files) and anything inside a hidden folder, which the scan skips too. A
/// path that is gone can't be told apart, so it always counts.
fn worth_a_rescan(path: &Path, roots: &[PathBuf]) -> bool {
    let inside = roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    if inside
        .iter()
        .any(|part| part.to_str().is_some_and(|part| part.starts_with('.')))
    {
        return false;
    }
    is_music_file(path) || !path.is_file()
}

/// The embedded cover picture of a local file, if it has one.
pub fn embedded_artwork(path: &Path) -> Option<Vec<u8>> {
    let (mut reader, outer) = open(path).ok()?;
    let pick = |revision: &MetadataRevision| {
        let visuals = revision.visuals();
        visuals
            .iter()
            .find(|visual| {
                visual.usage == Some(symphonia::core::meta::StandardVisualKey::FrontCover)
            })
            .or_else(|| visuals.first())
            .map(|visual| visual.data.to_vec())
    };
    reader
        .metadata()
        .current()
        .and_then(pick)
        .or_else(|| outer.as_ref().and_then(pick))
}

#[cfg(test)]
/// A tiny 16-bit PCM WAV file: `seconds` of silence at `rate` Hz.
pub(crate) fn write_test_wav(path: &Path, rate: u32, channels: u16, seconds: u32) {
    let frames = rate * seconds;
    let data_len = frames * u32::from(channels) * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + data_len as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("sonder-local-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn an_albums_tracks_share_one_cover_within_a_folder() {
        let entry = |path: &str, album: &str, art: bool| {
            let (mut track, _) = track_from_tags(path, Tags::default(), 1000);
            track.album = album.to_owned().into();
            track.artwork_url = art.then(|| format!("{ARTWORK_SCHEME}{path}"));
            IndexEntry {
                path: path.to_owned(),
                modified_ms: 0,
                size: 0,
                sort_key: String::new(),
                track: Some(track),
            }
        };
        let library = LocalLibrary::from_index(vec![
            entry("/m/Tidewater/01.flac", "Tidewater", true),
            entry("/m/Tidewater/02.flac", "Tidewater", true),
            entry("/m/Tidewater/03.flac", "Tidewater", false),
            entry("/m/Rooms/01.flac", "Rooms", true),
            entry("/n/Rooms/01.flac", "Rooms", true),
            entry("/m/Loose/a.flac", "", true),
            entry("/m/Loose/b.flac", "", true),
        ]);
        let covers: Vec<_> = library
            .tracks
            .iter()
            .map(|track| {
                track
                    .artwork_url
                    .as_deref()
                    .map(|url| &url[ARTWORK_SCHEME.len()..])
            })
            .collect();
        assert_eq!(
            covers,
            [
                Some("/m/Tidewater/01.flac"),
                Some("/m/Tidewater/01.flac"),
                // A song without art keeps none.
                None,
                Some("/m/Rooms/01.flac"),
                // The same album name in another folder is another album.
                Some("/n/Rooms/01.flac"),
                // Untagged songs keep their own covers.
                Some("/m/Loose/a.flac"),
                Some("/m/Loose/b.flac"),
            ]
        );
    }

    #[test]
    fn local_uris_use_spotify_format_and_round_trip() {
        let uri = local_uri("Ana Lind", "Rooms: Two", "Ten & One", 61_500);
        assert_eq!(uri, "spotify:local:Ana+Lind:Rooms%3A+Two:Ten+%26+One:61");
        let parsed = librespot::core::SpotifyUri::from_uri(&uri).unwrap();
        assert_eq!(parsed.to_string(), uri);
        assert!(is_local_uri(&uri));
        assert!(!is_local_uri("spotify:track:0123456789ABCDEFGHIJKL"));
    }

    #[test]
    fn untagged_files_take_artist_and_title_from_the_name() {
        let (track, _) = track_from_tags("C:/Music/Ana Lind - Harbour.mp3", Tags::default(), 1000);
        assert_eq!(track.artist.as_ref(), "Ana Lind");
        assert_eq!(track.title.as_ref(), "Harbour");
        assert_eq!(track.provider, Provider::Local);
        assert!(track.album_ref.is_none());
        assert!(track.is_displayable());

        let (plain, _) = track_from_tags("C:/Music/Harbour.flac", Tags::default(), 1000);
        assert_eq!(plain.title.as_ref(), "Harbour");
        assert_eq!(plain.artist.as_ref(), "Unknown artist");
    }

    #[test]
    fn tags_order_albums_by_disc_then_track() {
        let tagged = |track_number, disc_number| Tags {
            title: Some(format!("Song {track_number}")),
            artist: Some("Ana Lind".into()),
            album: Some("Rooms".into()),
            track_number: Some(track_number),
            disc_number: Some(disc_number),
            ..Tags::default()
        };
        let mut keys = [
            track_from_tags("b", tagged(1, 2), 1000).1,
            track_from_tags("c", tagged(10, 1), 1000).1,
            track_from_tags("a", tagged(2, 1), 1000).1,
        ];
        keys.sort();
        assert!(keys[0].contains("song 2"));
        assert!(keys[1].contains("song 10"));
        assert!(keys[2].contains("song 1\u{1}"));
    }

    #[test]
    fn track_numbers_accept_the_of_total_form() {
        assert_eq!(number(&Value::String("3/12".into())), Some(3));
        assert_eq!(number(&Value::UnsignedInt(7)), Some(7));
        assert_eq!(number(&Value::String("side A".into())), None);
    }

    #[test]
    fn scans_find_music_skip_other_files_and_reuse_unchanged_entries() {
        let folder = temp_folder("scan");
        std::fs::create_dir_all(folder.join("Album")).unwrap();
        std::fs::create_dir_all(folder.join(".hidden")).unwrap();
        write_test_wav(
            &folder.join("Album").join("Ana Lind - Harbour.wav"),
            48_000,
            1,
            2,
        );
        write_test_wav(&folder.join(".hidden").join("secret.wav"), 44_100, 2, 1);
        std::fs::write(folder.join("notes.txt"), "not music").unwrap();
        std::fs::write(folder.join("broken.mp3"), "not really an mp3").unwrap();

        let entries = scan(std::slice::from_ref(&folder), Vec::new(), &|| false).unwrap();
        let library = LocalLibrary::from_index(entries.clone());
        assert_eq!(library.tracks.len(), 1);
        assert_eq!(library.unreadable, 1);
        let track = &library.tracks[0];
        assert_eq!(track.title.as_ref(), "Harbour");
        assert_eq!(track.duration_ms, 2000);
        assert_eq!(
            track.spotify_uri.as_deref(),
            Some("spotify:local:Ana+Lind::Harbour:2")
        );

        // A second scan with the same files reads nothing again: a planted
        // marker in the saved entry survives.
        let mut previous = entries;
        for entry in &mut previous {
            if let Some(track) = entry.track.as_mut() {
                track.title = "from the index".into();
            }
        }
        let again = scan(std::slice::from_ref(&folder), previous, &|| false).unwrap();
        let again = LocalLibrary::from_index(again);
        assert_eq!(again.tracks[0].title.as_ref(), "from the index");

        assert!(scan(std::slice::from_ref(&folder), Vec::new(), &|| true).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn only_music_and_removals_outside_hidden_folders_ask_for_a_rescan() {
        let folder = temp_folder("worth");
        std::fs::write(folder.join("cover.jpg"), b"x").unwrap();
        std::fs::write(folder.join("song.flac"), b"x").unwrap();
        let roots = [folder.clone()];
        assert!(worth_a_rescan(&folder.join("song.flac"), &roots));
        assert!(worth_a_rescan(&folder.join("Album"), &roots));
        assert!(worth_a_rescan(&folder.join("gone.txt"), &roots));
        assert!(!worth_a_rescan(&folder.join("cover.jpg"), &roots));
        assert!(!worth_a_rescan(
            &folder.join(".sync").join("song.flac"),
            &roots
        ));
        assert!(!worth_a_rescan(&folder.join(".partial.flac"), &roots));
        // A dot in the folder above the music folder doesn't hide it.
        let dotted = folder.join(".parent").join("Music");
        std::fs::create_dir_all(&dotted).unwrap();
        assert!(worth_a_rescan(
            &dotted.join("song.mp3"),
            std::slice::from_ref(&dotted)
        ));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_watcher_reports_new_music_and_ignores_other_files() {
        let folder = temp_folder("watch");
        let (sent, changes) = std::sync::mpsc::channel();
        let _watcher = FolderWatcher::new(std::slice::from_ref(&folder), move || {
            let _ = sent.send(());
        })
        .expect("a temporary folder can be watched");
        // Some systems still report the folder's own creation; let it pass.
        while changes
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_ok()
        {}

        std::fs::write(folder.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(folder.join(".hidden")).unwrap();
        std::fs::write(folder.join(".hidden").join("song.flac"), b"x").unwrap();
        assert!(
            changes
                .recv_timeout(std::time::Duration::from_millis(750))
                .is_err(),
            "other files and hidden folders ask for no rescan"
        );

        std::fs::write(folder.join("song.flac"), b"x").unwrap();
        assert!(
            changes
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_ok(),
            "a new song asks for a rescan"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_missing_folder_is_not_watched() {
        let folder = temp_folder("unwatched").join("missing");
        assert!(FolderWatcher::new(&[folder], || {}).is_none());
    }
}
