//! Saves audio from a link into a Local Files folder.
//!
//! Two routes, so almost any link works without waiting on a download tool:
//!
//! * A link straight to an audio file is fetched by Sonder itself.
//! * Anything else (a page or a stream on one of the 1,800+ sites that
//!   yt-dlp understands) goes through yt-dlp, which Sonder fetches once into
//!   its own data folder, verifies against the checksum its publisher posts,
//!   and keeps current.
//!
//! ffmpeg does the conversions (MP3, FLAC, M4A, Ogg, WAV) and tags the file.
//! Everything here blocks, so callers run it on its own thread.
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde::Deserialize;
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

/// Most links Sonder accepts in one go.
pub const MAX_LINKS: usize = 25;
/// Most songs taken from one playlist link.
pub const MAX_PLAYLIST_SONGS: usize = 100;
/// The largest single file a direct link may deliver.
const MAX_DIRECT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How long a downloader goes unchecked before Sonder asks it to update.
const TOOL_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// A file type the listener can ask for. Every one plays in Sonder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// The best the source has, kept as it is when Sonder can play it and
    /// otherwise saved as FLAC so nothing more is lost.
    Best,
    Mp3,
    Flac,
    M4a,
    Vorbis,
    Wav,
}

impl Format {
    pub const ALL: [Format; 6] = [
        Format::Best,
        Format::Mp3,
        Format::Flac,
        Format::M4a,
        Format::Vorbis,
        Format::Wav,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::M4a => "m4a",
            Self::Vorbis => "ogg",
            Self::Wav => "wav",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Best => "Best",
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::M4a => "M4A",
            Self::Vorbis => "Ogg",
            Self::Wav => "WAV",
        }
    }

    /// One line on what the choice gives.
    pub fn detail(self) -> &'static str {
        match self {
            Self::Best => "The highest quality the source has, kept as it is",
            Self::Mp3 => "320 kbps, plays everywhere",
            Self::Flac => "Lossless, the largest files",
            Self::M4a => "256 kbps AAC",
            Self::Vorbis => "Vorbis at its highest quality",
            Self::Wav => "Uncompressed",
        }
    }

    /// The codec yt-dlp should extract, and its quality, given the source's
    /// best audio codec when that is known.
    fn extraction(self, source_codec: Option<&str>) -> (&'static str, Option<&'static str>) {
        match self {
            Self::Mp3 => ("mp3", Some("320K")),
            Self::Flac => ("flac", None),
            Self::M4a => ("m4a", Some("256K")),
            Self::Vorbis => ("vorbis", Some("0")),
            Self::Wav => ("wav", None),
            Self::Best => match source_codec.map(str::to_ascii_lowercase) {
                Some(codec) if codec.starts_with("mp4a") || codec == "aac" => ("m4a", None),
                Some(codec) if codec == "mp3" || codec.starts_with("mp3") => ("mp3", None),
                Some(codec) if codec == "flac" => ("flac", None),
                Some(codec) if codec == "vorbis" => ("vorbis", None),
                _ => ("flac", None),
            },
        }
    }

    /// The ffmpeg arguments that write this format, for a file Sonder fetched
    /// itself. `None` for `Best`, which keeps the file.
    fn ffmpeg_encoder(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Best => None,
            Self::Mp3 => Some(&["-c:a", "libmp3lame", "-b:a", "320k"]),
            Self::Flac => Some(&["-c:a", "flac"]),
            Self::M4a => Some(&["-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"]),
            Self::Vorbis => Some(&["-c:a", "libvorbis", "-q:a", "10"]),
            Self::Wav => Some(&["-c:a", "pcm_s16le"]),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Best | Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Vorbis => "ogg",
            Self::Wav => "wav",
        }
    }
}

/// What Sonder learned about a link before saving it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub title: String,
    pub artist: Option<String>,
    pub duration_secs: Option<f64>,
    pub thumbnail: Option<String>,
    /// The site's name, for the line under the title.
    pub site: String,
    /// How many songs the link holds when it is a playlist.
    pub playlist_count: Option<usize>,
    /// The link is an audio file Sonder can fetch without a downloader.
    pub direct: bool,
    /// The codec of the best audio stream the source offers.
    pub source_codec: Option<String>,
    /// The artist is one the source named, not a channel or uploader stood
    /// in for it.
    pub tagged_artist: bool,
}

/// Why a link was turned down before any work started.
pub fn refusal(url: &str) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if is("spotify.com") || is("spotify.link") {
        return Some(
            "Spotify songs are copy-protected, so they can’t be saved. Play them right here in Sonder."
                .to_owned(),
        );
    }
    let service = if is("music.apple.com") {
        "Apple Music"
    } else if is("tidal.com") {
        "Tidal"
    } else if host.starts_with("music.amazon.") || host.starts_with("music.amazon") {
        "Amazon Music"
    } else if is("deezer.com") {
        "Deezer"
    } else if is("qobuz.com") {
        "Qobuz"
    } else {
        return None;
    };
    Some(format!(
        "{service} songs are copy-protected, so they can’t be saved."
    ))
}

/// The web links in whatever was pasted, in order and without repeats.
/// Anything between links (spaces, commas, lines) separates them.
pub fn parse_links(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let word = word.trim_matches(|c: char| "<>()[]\"'“”‘’".contains(c));
        // Sentence punctuation after a link is not part of it.
        let word = word
            .trim_end_matches(['.', '!', '?', ':'])
            .trim_end_matches(|c: char| ")]>\"'”’".contains(c));
        let Ok(parsed) = url::Url::parse(word) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            continue;
        }
        let link = parsed.to_string();
        if !links.contains(&link) {
            links.push(link);
        }
        if links.len() == MAX_LINKS {
            break;
        }
    }
    links
}

const DIRECT_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "ogg", "oga", "wav", "aac", "opus", "wma", "aif", "aiff", "mka",
];
/// The file types Sonder plays as they are.
const PLAYABLE: &[&str] = &["mp3", "flac", "m4a", "ogg", "oga", "wav"];

fn direct_extension(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let name = parsed.path_segments()?.next_back()?;
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    DIRECT_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(extension)
}

fn decoded_file_stem(url: &str) -> String {
    let stem = url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            let segment = parsed.path_segments()?.next_back()?.to_owned();
            let decoded = percent_encoding::percent_decode_str(&segment)
                .decode_utf8_lossy()
                .into_owned();
            Some(
                Path::new(&decoded)
                    .file_stem()?
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .unwrap_or_default();
    let spaced = stem.replace(['_', '+'], " ");
    let trimmed = spaced.trim();
    if trimmed.is_empty() {
        "Downloaded song".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// A file name that is safe on Windows, macOS and Linux.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(150)
        .collect();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    if trimmed.is_empty() || reserved.contains(&trimmed.to_ascii_uppercase().as_str()) {
        "Downloaded song".to_owned()
    } else {
        trimmed
    }
}

/// `folder/stem.extension`, or `folder/stem (2).extension` when that is taken.
fn free_path(folder: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = folder.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}).{extension}")))
        .find(|path| !path.exists())
        .expect("an unbounded range always ends")
}

// ---------------------------------------------------------------- tools

/// The downloader and converter Sonder can reach right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Where Sonder keeps the tools it fetched.
pub fn tools_dir() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("dev", "twoducks", "cadence")?
            .data_local_dir()
            .join("tools"),
    )
}

fn which(name: &str) -> Option<PathBuf> {
    let file = exe(name);
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // A packaged app on macOS does not inherit the shell's PATH.
    directories.extend(
        ["/opt/homebrew/bin", "/usr/local/bin"]
            .into_iter()
            .map(PathBuf::from),
    );
    directories
        .into_iter()
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

pub fn find_tools() -> Tools {
    let overridden = |variable: &str| {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.is_file())
    };
    let own = tools_dir();
    let yt_dlp = overridden("SONDER_YT_DLP")
        .or_else(|| {
            own.as_ref()
                .map(|dir| dir.join(exe("yt-dlp")))
                .filter(|path| path.is_file())
        })
        .or_else(|| which("yt-dlp"));
    let ffmpeg = overridden("SONDER_FFMPEG")
        .or_else(|| {
            own.as_ref()
                .map(|dir| dir.join("ffmpeg").join("bin").join(exe("ffmpeg")))
                .filter(|path| path.is_file())
        })
        .or_else(|| which("ffmpeg"));
    Tools { yt_dlp, ffmpeg }
}

/// A command that never flashes a console window on Windows.
fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command.stdin(Stdio::null());
    command
}

/// What the person sees while a job runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// A tool is being fetched for the first time.
    Setup {
        what: &'static str,
        fraction: Option<f32>,
    },
    /// Audio is arriving.
    Working {
        fraction: Option<f32>,
        bytes_per_second: Option<f64>,
        /// Which song of a playlist this is, counted from 1.
        item: Option<(usize, usize)>,
    },
    /// The file is being converted and tagged.
    Converting,
}

pub type Report<'a> = &'a mut dyn FnMut(Update);

fn cancelled() -> anyhow::Error {
    anyhow!("Cancelled")
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Sonder")
        .connect_timeout(Duration::from_secs(20))
        .build()?)
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

/// `(sha256, name)` pairs from a `SHA2-256SUMS` or `checksums.sha256` file.
fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (hash.len() == 64).then(|| (hash.to_ascii_lowercase(), name.to_owned()))
        })
        .collect()
}

/// Downloads `url` to `target`, checking it against `expected` (hex SHA-256).
async fn fetch_verified(
    client: &reqwest::Client,
    url: &str,
    expected: &str,
    target: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Option<f32>),
) -> Result<()> {
    use futures::StreamExt as _;
    use sha2::{Digest as _, Sha256};
    let partial = target.with_extension("part");
    let result = async {
        let response = client.get(url).send().await?.error_for_status()?;
        let total = response.content_length();
        let mut file = std::fs::File::create(&partial)?;
        let mut stream = response.bytes_stream();
        let mut digest = Sha256::new();
        let mut received = 0u64;
        while let Some(chunk) = stream.next().await {
            ensure!(!cancel.load(Ordering::Relaxed), cancelled());
            let chunk = chunk?;
            received += chunk.len() as u64;
            file.write_all(&chunk)?;
            digest.update(&chunk);
            progress(total.map(|total| received as f32 / total.max(1) as f32));
        }
        file.sync_all()?;
        ensure!(
            format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected),
            "A download did not match its checksum, so Sonder threw it away."
        );
        std::fs::rename(&partial, target)?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

const YT_DLP_RELEASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const FFMPEG_RELEASE: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest";

fn require_windows(what: &str) -> Result<()> {
    ensure!(
        cfg!(windows),
        "{what} isn’t installed. Install it and Sonder will find it."
    );
    Ok(())
}

async fn fetch_yt_dlp(cancel: &AtomicBool, report: Report<'_>) -> Result<PathBuf> {
    require_windows("yt-dlp")?;
    let directory = tools_dir().context("no place to keep the downloader")?;
    std::fs::create_dir_all(&directory)?;
    let client = client()?;
    report(Update::Setup {
        what: "the downloader",
        fraction: None,
    });
    let sums = client
        .get(format!("{YT_DLP_RELEASE}/SHA2-256SUMS"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let name = exe("yt-dlp");
    let expected = parse_checksums(&sums)
        .into_iter()
        .find(|(_, file)| *file == name)
        .map(|(hash, _)| hash)
        .context("the downloader’s checksum was not published")?;
    let target = directory.join(&name);
    fetch_verified(
        &client,
        &format!("{YT_DLP_RELEASE}/{name}"),
        &expected,
        &target,
        cancel,
        |fraction| {
            report(Update::Setup {
                what: "the downloader",
                fraction,
            });
        },
    )
    .await?;
    Ok(target)
}

async fn fetch_ffmpeg(cancel: &AtomicBool, report: Report<'_>) -> Result<PathBuf> {
    require_windows("ffmpeg")?;
    let directory = tools_dir().context("no place to keep the converter")?;
    std::fs::create_dir_all(&directory)?;
    let client = client()?;
    report(Update::Setup {
        what: "the audio converter",
        fraction: None,
    });
    let sums = client
        .get(format!("{FFMPEG_RELEASE}/checksums.sha256"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let archive_name = "ffmpeg-master-latest-win64-gpl-shared.zip";
    let expected = parse_checksums(&sums)
        .into_iter()
        .find(|(_, file)| file == archive_name)
        .map(|(hash, _)| hash)
        .context("the converter’s checksum was not published")?;
    let archive = directory.join(archive_name);
    fetch_verified(
        &client,
        &format!("{FFMPEG_RELEASE}/{archive_name}"),
        &expected,
        &archive,
        cancel,
        |fraction| {
            report(Update::Setup {
                what: "the audio converter",
                fraction,
            });
        },
    )
    .await?;
    let staging = directory.join("ffmpeg-new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    // Windows ships a `tar` that reads zip archives.
    let status = command(Path::new("tar"))
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("could not unpack the converter")?;
    let _ = std::fs::remove_file(&archive);
    ensure!(status.success(), "could not unpack the converter");
    // The archive holds one folder; its `bin` is what ffmpeg needs.
    let unpacked = std::fs::read_dir(&staging)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.join("bin").join(exe("ffmpeg")).is_file())
        .context("the converter archive was not what Sonder expected")?;
    let final_dir = directory.join("ffmpeg");
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&unpacked, &final_dir)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(final_dir.join("bin").join(exe("ffmpeg")))
}

fn stamp_path(yt_dlp: &Path) -> PathBuf {
    yt_dlp.with_extension("checked")
}

/// Asks a Sonder-managed yt-dlp to update itself when it has not looked in a
/// while; sites change often and an old downloader stops working on them.
fn refresh_if_stale(yt_dlp: &Path) {
    let Some(own) = tools_dir() else { return };
    if !yt_dlp.starts_with(&own) {
        return;
    }
    let fresh = std::fs::metadata(stamp_path(yt_dlp))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|age| age < TOOL_MAX_AGE);
    if !fresh {
        update_yt_dlp(yt_dlp);
    }
}

fn update_yt_dlp(yt_dlp: &Path) {
    let _ = command(yt_dlp).arg("-U").output();
    let _ = std::fs::write(stamp_path(yt_dlp), b"");
}

/// The tools a link needs, fetching what is missing.
fn ensure_tools(
    need_ffmpeg: bool,
    need_yt_dlp: bool,
    cancel: &AtomicBool,
    report: Report<'_>,
) -> Result<Tools> {
    let mut tools = find_tools();
    if (need_yt_dlp && tools.yt_dlp.is_none()) || (need_ffmpeg && tools.ffmpeg.is_none()) {
        let runtime = runtime()?;
        if need_yt_dlp && tools.yt_dlp.is_none() {
            tools.yt_dlp = Some(runtime.block_on(fetch_yt_dlp(cancel, report))?);
        }
        if need_ffmpeg && tools.ffmpeg.is_none() {
            tools.ffmpeg = Some(runtime.block_on(fetch_ffmpeg(cancel, report))?);
        }
    }
    Ok(tools)
}

// ---------------------------------------------------------------- probing

#[derive(Deserialize, Default)]
struct RawFormat {
    acodec: Option<String>,
    vcodec: Option<String>,
    abr: Option<f64>,
    tbr: Option<f64>,
}

#[derive(Deserialize, Default)]
struct RawInfo {
    title: Option<String>,
    artist: Option<String>,
    artists: Option<Vec<String>>,
    uploader: Option<String>,
    channel: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    webpage_url_domain: Option<String>,
    extractor_key: Option<String>,
    #[serde(rename = "_type")]
    kind: Option<String>,
    entries: Option<Vec<serde_json::Value>>,
    playlist_count: Option<usize>,
    #[serde(default)]
    formats: Vec<RawFormat>,
}

/// The codec of the best audio-only stream in `formats`.
fn best_audio_codec(formats: &[RawFormat]) -> Option<String> {
    formats
        .iter()
        .filter(|format| {
            format
                .acodec
                .as_deref()
                .is_some_and(|codec| codec != "none")
                && format.vcodec.as_deref().is_none_or(|codec| codec == "none")
        })
        .max_by(|a, b| {
            let rate = |f: &RawFormat| f.abr.or(f.tbr).unwrap_or(0.);
            rate(a).total_cmp(&rate(b))
        })
        .and_then(|format| format.acodec.clone())
}

fn probe_from_json(json: &str) -> Result<Probe> {
    let info: RawInfo = serde_json::from_str(json).context("unreadable link details")?;
    let is_playlist = info.kind.as_deref() == Some("playlist");
    let count = is_playlist.then(|| {
        info.entries
            .as_ref()
            .map(Vec::len)
            .or(info.playlist_count)
            .unwrap_or(0)
    });
    let named = info
        .artist
        .or_else(|| info.artists.and_then(|artists| artists.into_iter().next()))
        .filter(|artist| !artist.trim().is_empty());
    let tagged_artist = named.is_some();
    let artist = named
        .or(info.uploader)
        .or(info.channel)
        .filter(|artist| !artist.trim().is_empty());
    let site = info
        .webpage_url_domain
        .map(|domain| domain.trim_start_matches("www.").to_owned())
        .or(info.extractor_key)
        .unwrap_or_default();
    Ok(Probe {
        title: info.title.unwrap_or_else(|| "Untitled".to_owned()),
        artist,
        duration_secs: info.duration,
        thumbnail: info.thumbnail,
        site,
        playlist_count: count,
        direct: false,
        source_codec: best_audio_codec(&info.formats),
        tagged_artist,
    })
}

/// Reads a process's output while watching for cancellation and a deadline.
fn run_capture(
    mut child: std::process::Child,
    deadline: Duration,
    cancel: &AtomicBool,
) -> Result<(String, String)> {
    let mut stdout = child.stdout.take().context("no output")?;
    let mut stderr = child.stderr.take().context("no output")?;
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(cancelled());
        }
        if started.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("That link took too long to answer.");
        }
        match child.try_wait()? {
            Some(_) => break,
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    }
    Ok((
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    ))
}

/// Looks a link up without saving anything.
pub fn probe(url: &str, cancel: &AtomicBool, report: Report<'_>) -> Result<Probe> {
    if let Some(reason) = refusal(url) {
        bail!(reason);
    }
    if let Some(extension) = direct_extension(url) {
        return Ok(Probe {
            title: decoded_file_stem(url),
            artist: None,
            duration_secs: None,
            thumbnail: None,
            site: url::Url::parse(url)
                .ok()
                .and_then(|parsed| {
                    parsed
                        .host_str()
                        .map(|host| host.trim_start_matches("www.").to_owned())
                })
                .unwrap_or_default(),
            playlist_count: None,
            direct: true,
            source_codec: Some(extension),
            tagged_artist: false,
        });
    }
    let tools = ensure_tools(false, true, cancel, report)?;
    let yt_dlp = tools.yt_dlp.context("the downloader is missing")?;
    refresh_if_stale(&yt_dlp);
    let attempt = |yt_dlp: &Path| -> Result<String> {
        let child = command(yt_dlp)
            .args([
                "--dump-single-json",
                "--no-warnings",
                "--skip-download",
                "--flat-playlist",
                "--playlist-end",
                &MAX_PLAYLIST_SONGS.to_string(),
                "--socket-timeout",
                "20",
                "--",
                url,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start the downloader")?;
        let (json, errors) = run_capture(child, Duration::from_secs(60), cancel)?;
        if matches!(json.trim(), "" | "null") {
            bail!(friendly_error(&errors));
        }
        Ok(json)
    };
    let json = match attempt(&yt_dlp) {
        Ok(json) => json,
        Err(error)
            if looks_outdated(&error.to_string())
                && yt_dlp.starts_with(tools_dir().unwrap_or_default()) =>
        {
            update_yt_dlp(&yt_dlp);
            attempt(&yt_dlp)?
        }
        Err(error) => return Err(error),
    };
    probe_from_json(&json)
}

// ---------------------------------------------------------------- errors

fn looks_outdated(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    ["changed how", "try again later", "refused"]
        .iter()
        .any(|hint| message.contains(hint))
}

/// A sentence for the person, from what a downloader printed.
fn friendly_error(stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);
    if has("unsupported url") {
        return "Sonder can’t find audio at that link.".to_owned();
    }
    if has("drm") {
        return "That audio is copy-protected, so it can’t be saved.".to_owned();
    }
    if has("sign in")
        || has("log in")
        || has("login")
        || has("private")
        || has("members-only")
        || has("premium")
    {
        return "That link needs a sign-in, so it can’t be saved.".to_owned();
    }
    if has("not available")
        || has("unavailable")
        || has("has been removed")
        || has("404")
        || has("does not exist")
    {
        return "That link isn’t available any more.".to_owned();
    }
    if has("geo") && (has("restrict") || has("block")) || has("not available in your country") {
        return "That audio isn’t available in your country.".to_owned();
    }
    if has("getaddrinfo")
        || has("name or service not known")
        || has("network is unreachable")
        || has("timed out")
        || has("connection")
    {
        return "Can’t reach that site. Check your connection.".to_owned();
    }
    if has("http error 403")
        || has("http error 429")
        || has("unable to extract")
        || has("nsig")
        || has("sabr")
    {
        return "The site refused the download. Try again later.".to_owned();
    }
    let line = stderr
        .lines()
        .rev()
        .find(|line| line.contains("ERROR"))
        .unwrap_or("")
        .trim_start_matches("ERROR:")
        .trim();
    if line.is_empty() {
        "That link could not be saved.".to_owned()
    } else {
        let short: String = line.chars().take(140).collect();
        format!("That link could not be saved: {short}")
    }
}

// ---------------------------------------------------------------- saving

/// One link to save.
#[derive(Clone, Debug)]
pub struct Job {
    pub url: String,
    pub format: Format,
    pub folder: PathBuf,
    /// Save every song of a playlist link, not just the first.
    pub playlist: bool,
    pub probe: Probe,
}

fn percent_field(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// `SONDER_PROGRESS downloaded total estimate speed`, each possibly `NA`.
fn parse_progress(line: &str) -> Option<Update> {
    let rest = line.strip_prefix("SONDER_PROGRESS ")?;
    let mut fields = rest.split_whitespace().map(percent_field);
    let downloaded = fields.next().flatten();
    let total = fields.next().flatten();
    let estimate = fields.next().flatten();
    let speed = fields.next().flatten();
    let fraction = downloaded
        .zip(total.or(estimate))
        .filter(|(_, total)| *total > 0.)
        .map(|(done, total)| (done / total).clamp(0., 1.) as f32);
    Some(Update::Working {
        fraction,
        bytes_per_second: speed,
        item: None,
    })
}

/// `[download] Downloading item 3 of 12` -> `(3, 12)`.
fn parse_item(line: &str) -> Option<(usize, usize)> {
    let rest = line.split("Downloading item ").nth(1)?;
    let (index, rest) = rest.split_once(" of ")?;
    let total = rest.split_whitespace().next()?;
    Some((index.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// Saves a link and returns the files it made.
pub fn save(job: &Job, cancel: &Arc<AtomicBool>, report: Report<'_>) -> Result<Vec<PathBuf>> {
    if let Some(reason) = refusal(&job.url) {
        bail!(reason);
    }
    std::fs::create_dir_all(&job.folder).context("could not open the folder")?;
    ensure!(!cancel.load(Ordering::Relaxed), cancelled());
    if job.probe.direct {
        save_direct(job, cancel, report)
    } else {
        save_with_yt_dlp(job, cancel, report)
    }
}

fn save_direct(job: &Job, cancel: &Arc<AtomicBool>, report: Report<'_>) -> Result<Vec<PathBuf>> {
    let extension = direct_extension(&job.url).unwrap_or_else(|| "mp3".to_owned());
    let stem = sanitize_file_name(&job.probe.title);
    let plays_as_is = PLAYABLE.contains(&extension.as_str());
    let target_extension = match job.format {
        Format::Best if plays_as_is => extension.clone(),
        other => other.extension().to_owned(),
    };
    let converts = target_extension != extension;
    let tools = ensure_tools(converts, false, cancel, report)?;
    let destination = free_path(&job.folder, &stem, &target_extension);
    let raw = if converts {
        job.folder
            .join(format!(".sonder-{}.{extension}.part", std::process::id()))
    } else {
        destination.with_extension(format!("{target_extension}.part"))
    };
    let fetched = runtime()?.block_on(async {
        use futures::StreamExt as _;
        let response = client()?.get(&job.url).send().await?.error_for_status()?;
        if let Some(length) = response.content_length() {
            ensure!(
                length <= MAX_DIRECT_BYTES,
                "That file is too large to save."
            );
        }
        let total = response.content_length();
        let mut file = std::fs::File::create(&raw)?;
        let mut stream = response.bytes_stream();
        let mut received = 0u64;
        let started = std::time::Instant::now();
        while let Some(chunk) = stream.next().await {
            ensure!(!cancel.load(Ordering::Relaxed), cancelled());
            let chunk = chunk?;
            received += chunk.len() as u64;
            ensure!(
                received <= MAX_DIRECT_BYTES,
                "That file is too large to save."
            );
            file.write_all(&chunk)?;
            let seconds = started.elapsed().as_secs_f64().max(0.001);
            report(Update::Working {
                fraction: total.map(|total| (received as f64 / total.max(1) as f64) as f32),
                bytes_per_second: Some(received as f64 / seconds),
                item: None,
            });
        }
        file.sync_all()?;
        Ok(())
    });
    if let Err(error) = fetched {
        let _ = std::fs::remove_file(&raw);
        return Err(match error.downcast_ref::<reqwest::Error>() {
            Some(_) => anyhow!("Can’t reach that site. Check your connection."),
            None => error,
        });
    }
    if !converts {
        std::fs::rename(&raw, &destination)?;
        return Ok(vec![destination]);
    }
    report(Update::Converting);
    let ffmpeg = tools.ffmpeg.context("the converter is missing")?;
    let encoder = job.format.ffmpeg_encoder().unwrap_or(&["-c:a", "flac"]);
    let temp = destination.with_extension(format!("{target_extension}.part"));
    let child = command(&ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&raw)
        .args(["-vn", "-map_metadata", "0"])
        .args(encoder)
        .args(["-f", container_for(&target_extension)])
        .arg(&temp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start the converter")?;
    let result = run_capture(child, Duration::from_secs(30 * 60), cancel);
    let _ = std::fs::remove_file(&raw);
    let (_, errors) = match result {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
    };
    if !temp.is_file() || std::fs::metadata(&temp).map(|meta| meta.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&temp);
        bail!(
            "The converter could not read that file{}",
            errors
                .lines()
                .last()
                .map(|line| format!(": {}", line.chars().take(120).collect::<String>()))
                .unwrap_or_default()
        );
    }
    std::fs::rename(&temp, &destination)?;
    Ok(vec![destination])
}

fn container_for(extension: &str) -> &'static str {
    match extension {
        "mp3" => "mp3",
        "flac" => "flac",
        "m4a" => "ipod",
        "ogg" => "ogg",
        _ => "wav",
    }
}

/// The yt-dlp arguments for a job, without the program or the link.
fn yt_dlp_arguments(job: &Job, ffmpeg: &Path) -> Vec<String> {
    let (codec, quality) = job.format.extraction(job.probe.source_codec.as_deref());
    let template = job
        .folder
        .join("%(artist,uploader,channel&{} - |)s%(title)s.%(ext)s");
    let mut args: Vec<String> = [
        "--newline",
        "--no-colors",
        "--no-mtime",
        "--no-overwrites",
        "--windows-filenames",
        "--no-simulate",
        // `--print` would otherwise silence progress and conversion lines.
        "--progress",
        "--no-quiet",
        "--socket-timeout",
        "20",
        "--retries",
        "5",
        "-f",
        "bestaudio/best",
        "-x",
        "--audio-format",
        codec,
        "--embed-metadata",
        "--embed-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "--progress-template",
        "download:SONDER_PROGRESS %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s",
        "--print",
        "after_move:SONDER_FILE %(filepath)s",
    ]
    .map(str::to_owned)
    .to_vec();
    if !job.probe.tagged_artist {
        // An upload named "Artist - Title" gets that artist and title tag,
        // unless the site already named the artist.
        args.extend([
            "--parse-metadata".to_owned(),
            r"title:(?P<artist>.+?)\s+[-–—]\s+(?P<title>.+)".to_owned(),
        ]);
    }
    if let Some(quality) = quality {
        args.extend(["--audio-quality".to_owned(), quality.to_owned()]);
    }
    args.extend([
        "--ffmpeg-location".to_owned(),
        ffmpeg.to_string_lossy().into_owned(),
    ]);
    args.extend(["-o".to_owned(), template.to_string_lossy().into_owned()]);
    if job.playlist {
        args.extend([
            "--yes-playlist".to_owned(),
            "--playlist-end".to_owned(),
            MAX_PLAYLIST_SONGS.to_string(),
        ]);
    } else {
        args.push("--no-playlist".to_owned());
    }
    args.push("--".to_owned());
    args.push(job.url.clone());
    args
}

fn save_with_yt_dlp(
    job: &Job,
    cancel: &Arc<AtomicBool>,
    report: Report<'_>,
) -> Result<Vec<PathBuf>> {
    let tools = ensure_tools(true, true, cancel, report)?;
    let yt_dlp = tools.yt_dlp.context("the downloader is missing")?;
    let ffmpeg = tools.ffmpeg.context("the converter is missing")?;
    refresh_if_stale(&yt_dlp);
    let mut updated = false;
    loop {
        match run_yt_dlp(job, &yt_dlp, &ffmpeg, cancel, report) {
            Ok(files) => return Ok(files),
            Err(error)
                if !updated
                    && looks_outdated(&error.to_string())
                    && yt_dlp.starts_with(tools_dir().unwrap_or_default()) =>
            {
                updated = true;
                update_yt_dlp(&yt_dlp);
            }
            Err(error) => return Err(error),
        }
    }
}

fn run_yt_dlp(
    job: &Job,
    yt_dlp: &Path,
    ffmpeg: &Path,
    cancel: &Arc<AtomicBool>,
    report: Report<'_>,
) -> Result<Vec<PathBuf>> {
    let mut child = command(yt_dlp)
        .args(yt_dlp_arguments(job, ffmpeg))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start the downloader")?;
    let stdout = child.stdout.take().context("no output")?;
    let mut stderr = child.stderr.take().context("no output")?;
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    // A watcher ends the child on cancel; reading then runs to end of file.
    let watched = Arc::new(Mutex::new(Some(child)));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (cancel, watched, stop) = (cancel.clone(), watched.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    if let Some(child) = watched.lock().expect("watcher lock").as_mut() {
                        let _ = child.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(60));
            }
        })
    };
    let mut files = Vec::new();
    let mut item: Option<(usize, usize)> = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(path) = line.strip_prefix("SONDER_FILE ") {
            files.push(PathBuf::from(path.trim()));
        } else if let Some(Update::Working {
            fraction,
            bytes_per_second,
            ..
        }) = parse_progress(&line)
        {
            report(Update::Working {
                fraction,
                bytes_per_second,
                item,
            });
        } else if let Some(found) = parse_item(&line) {
            item = Some(found);
        } else if line.starts_with("[ExtractAudio]")
            || line.starts_with("[EmbedThumbnail]")
            || line.starts_with("[Metadata]")
            || line.starts_with("[ThumbnailsConvertor]")
            || line.starts_with("[Merger]")
        {
            report(Update::Converting);
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    let status = watched
        .lock()
        .expect("watcher lock")
        .take()
        .context("the downloader went missing")?
        .wait()?;
    let errors = errors.join().unwrap_or_default();
    ensure!(!cancel.load(Ordering::Relaxed), cancelled());
    files.retain(|path| path.is_file());
    if status.success() && !files.is_empty() {
        return Ok(files);
    }
    if files.is_empty() && status.success() {
        bail!("That link had no audio to save.");
    }
    bail!(friendly_error(&errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_found_among_any_separators_and_deduplicated() {
        let found = parse_links(
            "look: https://a.example/x.mp3, <https://b.example/watch?v=1>\nhttps://a.example/x.mp3 ftp://c.example/y not a link (https://d.example/z).",
        );
        assert_eq!(
            found,
            [
                "https://a.example/x.mp3",
                "https://b.example/watch?v=1",
                "https://d.example/z"
            ]
        );
    }

    #[test]
    fn a_paste_is_capped_at_the_link_limit() {
        let text: String = (0..60).map(|n| format!("https://a.example/{n} ")).collect();
        assert_eq!(parse_links(&text).len(), MAX_LINKS);
    }

    #[test]
    fn protected_services_are_refused_with_a_reason() {
        assert!(
            refusal("https://open.spotify.com/track/1")
                .unwrap()
                .contains("Play them right here")
        );
        assert!(refusal("https://music.apple.com/us/album/x").is_some());
        assert!(refusal("https://listen.tidal.com/track/1").is_some());
        assert!(refusal("https://archive.org/details/x").is_none());
        assert!(refusal("https://notspotify.com/x").is_none());
    }

    #[test]
    fn direct_audio_links_are_recognised_by_their_extension() {
        assert_eq!(
            direct_extension("https://a.example/m/Song%20One.MP3?token=1").as_deref(),
            Some("mp3")
        );
        assert_eq!(direct_extension("https://a.example/watch?v=1"), None);
        assert_eq!(direct_extension("https://a.example/page.html"), None);
    }

    #[test]
    fn a_direct_link_is_named_from_its_file() {
        assert_eq!(
            decoded_file_stem("https://a.example/m/Harbour_Lights%20Live.flac"),
            "Harbour Lights Live"
        );
        assert_eq!(decoded_file_stem("https://a.example/"), "Downloaded song");
    }

    #[test]
    fn file_names_are_safe_everywhere() {
        assert_eq!(
            sanitize_file_name("AC/DC: Back <In> Black?"),
            "AC DC Back In Black"
        );
        assert_eq!(sanitize_file_name("  ..  "), "Downloaded song");
        assert_eq!(sanitize_file_name("NUL"), "Downloaded song");
        assert_eq!(sanitize_file_name(&"a".repeat(400)).chars().count(), 150);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let directory = std::env::temp_dir().join(format!("sonder-free-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Song.mp3"), b"x").unwrap();
        std::fs::write(directory.join("Song (2).mp3"), b"x").unwrap();
        assert_eq!(
            free_path(&directory, "Song", "mp3"),
            directory.join("Song (3).mp3")
        );
        assert_eq!(
            free_path(&directory, "Other", "mp3"),
            directory.join("Other.mp3")
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_best_format_keeps_what_plays_and_saves_the_rest_losslessly() {
        assert_eq!(Format::Best.extraction(Some("mp4a.40.2")), ("m4a", None));
        assert_eq!(Format::Best.extraction(Some("mp3")), ("mp3", None));
        assert_eq!(Format::Best.extraction(Some("flac")), ("flac", None));
        assert_eq!(Format::Best.extraction(Some("opus")), ("flac", None));
        assert_eq!(Format::Best.extraction(None), ("flac", None));
        assert_eq!(Format::Mp3.extraction(Some("opus")), ("mp3", Some("320K")));
        for format in Format::ALL {
            assert_eq!(Format::from_key(format.key()), Some(format));
        }
    }

    #[test]
    fn the_best_audio_stream_is_chosen_by_bitrate() {
        let formats = vec![
            RawFormat {
                acodec: Some("mp4a.40.2".into()),
                vcodec: Some("none".into()),
                abr: Some(128.),
                tbr: None,
            },
            RawFormat {
                acodec: Some("opus".into()),
                vcodec: Some("none".into()),
                abr: Some(160.),
                tbr: None,
            },
            RawFormat {
                acodec: Some("aac".into()),
                vcodec: Some("avc1".into()),
                abr: Some(400.),
                tbr: None,
            },
        ];
        assert_eq!(best_audio_codec(&formats).as_deref(), Some("opus"));
    }

    #[test]
    fn link_details_are_read_from_yt_dlp_json() {
        let probe = probe_from_json(
            r#"{"title":"Harbour Lights","uploader":"Ana Lind","duration":221.5,"thumbnail":"https://i.example/t.jpg","webpage_url_domain":"www.example.com","formats":[{"acodec":"opus","vcodec":"none","abr":160.0}]}"#,
        )
        .unwrap();
        assert_eq!(probe.title, "Harbour Lights");
        assert_eq!(probe.artist.as_deref(), Some("Ana Lind"));
        assert_eq!(probe.site, "example.com");
        assert_eq!(probe.playlist_count, None);
        assert_eq!(probe.source_codec.as_deref(), Some("opus"));
        let playlist =
            probe_from_json(r#"{"_type":"playlist","title":"Mix","entries":[{},{},{}]}"#).unwrap();
        assert_eq!(playlist.playlist_count, Some(3));
    }

    #[test]
    fn progress_and_playlist_lines_are_understood() {
        assert_eq!(
            parse_progress("SONDER_PROGRESS 500 1000 NA 2048.5"),
            Some(Update::Working {
                fraction: Some(0.5),
                bytes_per_second: Some(2048.5),
                item: None
            })
        );
        assert_eq!(
            parse_progress("SONDER_PROGRESS 500 NA 2000 NA"),
            Some(Update::Working {
                fraction: Some(0.25),
                bytes_per_second: None,
                item: None
            })
        );
        assert_eq!(
            parse_progress("SONDER_PROGRESS NA NA NA NA"),
            Some(Update::Working {
                fraction: None,
                bytes_per_second: None,
                item: None
            })
        );
        assert_eq!(parse_progress("[download] 50%"), None);
        assert_eq!(
            parse_item("[download] Downloading item 3 of 12"),
            Some((3, 12))
        );
    }

    #[test]
    fn downloader_errors_become_plain_sentences() {
        assert_eq!(
            friendly_error("ERROR: Unsupported URL: https://x.example"),
            "Sonder can’t find audio at that link."
        );
        assert_eq!(
            friendly_error("ERROR: [youtube] abc: Sign in to confirm your age"),
            "That link needs a sign-in, so it can’t be saved."
        );
        assert_eq!(
            friendly_error("ERROR: [x] 1: Video unavailable"),
            "That link isn’t available any more."
        );
        assert!(
            friendly_error("ERROR: something odd happened")
                .starts_with("That link could not be saved: something odd")
        );
        assert!(looks_outdated(&friendly_error(
            "ERROR: HTTP Error 403: Forbidden"
        )));
    }

    #[test]
    fn checksums_are_read_from_a_sums_file() {
        let sums = format!(
            "{} yt-dlp.exe\n{} *other.zip\nnot a hash line\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(
            parse_checksums(&sums),
            [
                ("a".repeat(64), "yt-dlp.exe".to_owned()),
                ("b".repeat(64), "other.zip".to_owned())
            ]
        );
    }

    #[test]
    fn a_playlist_job_asks_for_the_whole_list_and_a_single_job_does_not() {
        let mut job = Job {
            url: "https://a.example/watch?v=1".into(),
            format: Format::Mp3,
            folder: PathBuf::from("/music"),
            playlist: false,
            probe: Probe::default(),
        };
        let single = yt_dlp_arguments(&job, Path::new("/ff/ffmpeg"));
        assert!(single.contains(&"--no-playlist".to_owned()));
        assert!(single.windows(2).any(|w| w == ["--audio-quality", "320K"]));
        assert_eq!(single.last().unwrap(), "https://a.example/watch?v=1");
        job.playlist = true;
        let list = yt_dlp_arguments(&job, Path::new("/ff/ffmpeg"));
        assert!(list.contains(&"--yes-playlist".to_owned()));
        assert!(!list.contains(&"--no-playlist".to_owned()));
    }

    /// Hits the network; run by hand with `cargo test real_links -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_links_save_in_every_format() {
        let url = std::env::var("SONDER_TEST_URL").expect("SONDER_TEST_URL");
        let formats: Vec<Format> = std::env::var("SONDER_TEST_FORMATS")
            .unwrap_or_else(|_| "best".into())
            .split(',')
            .filter_map(Format::from_key)
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let probe = probe(&url, &cancel, &mut |_| {}).expect("probe");
        println!("{probe:?}");
        for format in formats {
            let folder = std::env::temp_dir().join(format!("sonder-real-{}", format.key()));
            let _ = std::fs::remove_dir_all(&folder);
            let job = Job {
                url: url.clone(),
                format,
                folder,
                playlist: false,
                probe: probe.clone(),
            };
            let mut last = None;
            let files = save(&job, &cancel, &mut |update| last = Some(update)).expect("save");
            println!("{:?} -> {files:?} (last update {last:?})", format);
            assert!(files.iter().all(|f| {
                std::fs::metadata(f)
                    .map(|m| m.len() > 1000)
                    .unwrap_or(false)
            }));
        }
    }
}
