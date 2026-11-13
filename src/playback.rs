use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use keyring::Entry;
use librespot::{
    core::{SpotifyUri, authentication::Credentials, config::SessionConfig, session::Session},
    oauth::OAuthClientBuilder,
    playback::{
        config::{AudioFormat, Bitrate, PlayerConfig, VolumeCtrl},
        mixer::{self, Mixer, MixerConfig},
        player::{Player, PlayerEventChannel},
    },
};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
    basic::BasicClient,
};
use tokio::net::TcpListener;

use crate::{
    audio_output, credential_worker,
    model::Track,
    oauth_callback::receive_callback,
    oauth_page::{OAuthStep, success_page},
};

const PLAYBACK_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const PLAYBACK_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const KEYCHAIN_SERVICE: &str = "dev.twoducks.cadence";
const KEYCHAIN_ACCOUNT: &str = "playback-refresh-token";
const LOGGED_OUT_CREDENTIAL: &str = "cadence-logged-out";

type PlaybackOAuthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

pub(crate) struct PlaybackAuthorization {
    url: String,
    csrf_token: CsrfToken,
    pkce_verifier: PkceCodeVerifier,
    listener: TcpListener,
}

impl PlaybackAuthorization {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    async fn authorize(self) -> Result<PlaybackOAuthToken> {
        let mut callback = receive_callback(&self.listener, "/login")
            .await
            .context("could not receive the Spotify playback callback")?;
        let parameter = |name| {
            callback
                .url()
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if let Some(error) = parameter("error") {
            return Err(anyhow!("Spotify playback authorization failed: {error}"));
        }
        let state = parameter("state").context("Spotify playback callback omitted state")?;
        if state != self.csrf_token.secret().as_str() {
            return Err(anyhow!("Spotify playback callback state did not match"));
        }
        let code = parameter("code").context("Spotify playback callback omitted code")?;

        let http_client = oauth2_reqwest::ClientBuilder::new()
            .redirect(oauth2_reqwest::redirect::Policy::none())
            .build()?;
        let response = playback_oauth_client()?
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(self.pkce_verifier)
            .request_async(&http_client)
            .await
            .map_err(|error| anyhow!("could not exchange Spotify playback code: {error}"))?;
        let body = success_page(OAuthStep::Playback, None);
        callback
            .respond_html(&body)
            .await
            .context("could not write the Spotify playback callback response")?;
        Ok(PlaybackOAuthToken {
            access_token: response.access_token().secret().to_owned(),
            refresh_token: response
                .refresh_token()
                .map(|token| token.secret().to_owned())
                .unwrap_or_default(),
        })
    }
}

struct PlaybackOAuthToken {
    access_token: String,
    refresh_token: String,
}

#[derive(Clone)]
pub struct Playback {
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,
    session: Session,
    local_files: LocalFilePaths,
}

/// How many local files the player can find at once: the one loading, plus
/// any it has not finished opening when the listener skips on.
const REMEMBERED_LOCAL_FILES: usize = 8;

/// Which file each recently loaded `spotify:local:` URI means. A local URI
/// only names tags and a length, so the path comes from the track itself
/// just before it loads. Nothing scans folders when playback connects.
#[derive(Clone, Default)]
struct LocalFilePaths(Arc<Mutex<VecDeque<(SpotifyUri, PathBuf)>>>);

impl LocalFilePaths {
    fn remember(&self, uri: SpotifyUri, path: PathBuf) {
        let mut paths = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        paths.retain(|(known, _)| *known != uri);
        paths.push_back((uri, path));
        while paths.len() > REMEMBERED_LOCAL_FILES {
            paths.pop_front();
        }
    }

    fn find(&self, uri: &SpotifyUri) -> Option<PathBuf> {
        let paths = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        paths
            .iter()
            .rev()
            .find(|(known, _)| known == uri)
            .map(|(_, path)| path.clone())
    }
}

impl Playback {
    pub(crate) async fn prepare_authorization() -> Result<PlaybackAuthorization> {
        let listener = TcpListener::bind("127.0.0.1:8898")
            .await
            .context("could not listen for the Spotify playback callback")?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, csrf_token) = playback_oauth_client()?
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new("streaming".to_owned()))
            .set_pkce_challenge(pkce_challenge)
            .url();
        Ok(PlaybackAuthorization {
            url: url.to_string(),
            csrf_token,
            pkce_verifier,
            listener,
        })
    }

    pub(crate) async fn connect(
        load_saved_token: bool,
        authorization: Option<PlaybackAuthorization>,
    ) -> Result<Self> {
        let callback_page = success_page(OAuthStep::Playback, None);
        let oauth =
            OAuthClientBuilder::new(PLAYBACK_CLIENT_ID, PLAYBACK_REDIRECT_URI, vec!["streaming"])
                .open_in_browser()
                .with_custom_message(&callback_page)
                .build()
                .context("could not configure librespot authorization")?;
        let saved_refresh_token = if load_saved_token {
            load_playback_refresh_token().await.context(
                if cfg!(target_os = "macos") { "could not read playback credentials from Keychain; choose Always Allow when macOS asks" } else { "could not read playback credentials from the system credential store" },
            )?
        } else {
            None
        };
        log::info!("playback: credentials read from system credential store");
        let token = match saved_refresh_token {
            Some(refresh_token) => {
                let refreshed = oauth.refresh_token_async(&refresh_token).await;
                log::info!("playback: access token refreshed");
                let mut token = match refreshed {
                    Ok(token) => token,
                    Err(_) => match authorization {
                        Some(authorization) => {
                            let token = authorization.authorize().await?;
                            return Self::connect_with_oauth_token(token).await;
                        }
                        None => oauth
                            .get_access_token_async()
                            .await
                            .context("could not reauthorize librespot playback")?,
                    },
                };
                if token.refresh_token.is_empty() {
                    token.refresh_token = refresh_token;
                }
                token
            }
            None => match authorization {
                Some(authorization) => {
                    let token = authorization.authorize().await?;
                    return Self::connect_with_oauth_token(token).await;
                }
                None => oauth
                    .get_access_token_async()
                    .await
                    .context("could not authorize librespot playback")?,
            },
        };
        if token.refresh_token.is_empty() {
            return Err(anyhow!("Spotify returned an empty playback refresh token"));
        }
        let access_token = token.access_token;
        persist_playback_refresh_token(token.refresh_token).await?;
        Self::connect_with_access_token(access_token).await
    }

    pub async fn reconnect() -> Result<Self> {
        let refresh_token = load_playback_refresh_token()
            .await?
            .context("Spotify playback credentials are not available")?;
        let oauth =
            OAuthClientBuilder::new(PLAYBACK_CLIENT_ID, PLAYBACK_REDIRECT_URI, vec!["streaming"])
                .build()
                .context("could not configure librespot reconnection")?;
        let mut token = oauth
            .refresh_token_async(&refresh_token)
            .await
            .context("could not refresh librespot playback credentials")?;
        if token.refresh_token.is_empty() {
            token.refresh_token = refresh_token;
        }
        let access_token = token.access_token;
        persist_playback_refresh_token(token.refresh_token).await?;
        Self::connect_with_access_token(access_token).await
    }

    async fn connect_with_oauth_token(token: PlaybackOAuthToken) -> Result<Self> {
        let PlaybackOAuthToken {
            access_token,
            refresh_token,
        } = token;
        validate_refresh_token(&refresh_token)?;
        persist_playback_refresh_token(refresh_token).await?;
        Self::connect_with_access_token(access_token).await
    }

    async fn connect_with_access_token(access_token: String) -> Result<Self> {
        let session = Session::new(SessionConfig::default(), None);
        session
            .connect(Credentials::with_access_token(access_token), false)
            .await
            .context("librespot could not connect to Spotify")?;
        // Spotify announces the plan right after sign-in. A free account cannot
        // stream here, so say so instead of failing later with a vague error.
        for _ in 0..ACCOUNT_TYPE_POLLS {
            if let Some(kind) = session.get_user_attribute("type") {
                if kind != "premium" {
                    return Err(PremiumRequired.into());
                }
                break;
            }
            tokio::time::sleep(ACCOUNT_TYPE_POLL_DELAY).await;
        }
        let mixer = mixer::find(None).context("no supported audio mixer is available")?(
            MixerConfig::default(),
        )?;
        let volume = mixer.get_soft_volume();
        let local_files = LocalFilePaths::default();
        let resolver = local_files.clone();
        let player_config = PlayerConfig {
            position_update_interval: Some(Duration::from_millis(500)),
            local_file_resolver: Some(Arc::new(move |uri| resolver.find(uri))),
            normalisation: NORMALISE_VOLUME.load(Ordering::Relaxed),
            bitrate: match STREAMING_KBPS.load(Ordering::Relaxed) {
                96 => Bitrate::Bitrate96,
                160 => Bitrate::Bitrate160,
                _ => Bitrate::Bitrate320,
            },
            ..PlayerConfig::default()
        };
        let player = Player::new(player_config, session.clone(), volume, move || {
            audio_output::open(None, AudioFormat::F32)
        });
        Ok(Self {
            player,
            mixer,
            session,
            local_files,
        })
    }

    /// Loads a Spotify track, or a local file by the path its track carries.
    /// `uri` is the track's [`playback_uri`].
    pub fn load_track(&self, track: &Track, uri: SpotifyUri, playing: bool, position_ms: u32) {
        if track.is_local() {
            self.local_files
                .remember(uri.clone(), PathBuf::from(&track.source_id));
        }
        self.player.load(uri, playing, position_ms);
    }

    pub fn play(&self) {
        self.player.play();
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn seek(&self, position_ms: u32) {
        self.player.seek(position_ms);
    }

    pub fn set_volume(&self, volume: f32) {
        self.mixer
            .set_volume((volume.clamp(0., 1.) * f32::from(VolumeCtrl::MAX_VOLUME)) as u16);
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn events(&self) -> PlayerEventChannel {
        self.player.get_player_event_channel()
    }

    pub fn is_connected(&self) -> bool {
        !self.session.is_invalid()
    }

    pub async fn radio_track_uris(&self, seed_uri: &str) -> Result<Vec<String>> {
        let seed = SpotifyUri::from_uri(seed_uri).context("invalid radio seed track URI")?;
        let response = self
            .session
            .spclient()
            .get_apollo_station("tracks", &seed.to_uri()?, Some(30), Vec::new(), true)
            .await
            .context("Spotify track radio endpoint failed")?;
        extract_track_uris(&response).context("Spotify track radio returned invalid JSON")
    }
}

pub(crate) fn playback_uri(track: &Track) -> Result<SpotifyUri> {
    let uri = track
        .spotify_uri
        .as_deref()
        .context("track has no Spotify playback URI")?;
    SpotifyUri::from_uri(uri).context("invalid Spotify track URI")
}

fn playback_oauth_client() -> Result<PlaybackOAuthClient> {
    Ok(
        BasicClient::new(ClientId::new(PLAYBACK_CLIENT_ID.to_owned()))
            .set_auth_uri(AuthUrl::new(
                "https://accounts.spotify.com/authorize".to_owned(),
            )?)
            .set_token_uri(TokenUrl::new(
                "https://accounts.spotify.com/api/token".to_owned(),
            )?)
            .set_redirect_uri(RedirectUrl::new(PLAYBACK_REDIRECT_URI.to_owned())?),
    )
}

fn extract_track_uris(json: &[u8]) -> serde_json::Result<Vec<String>> {
    fn visit(value: &serde_json::Value, seen: &mut HashSet<String>, uris: &mut Vec<String>) {
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, seen, uris);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    visit(value, seen, uris);
                }
            }
            serde_json::Value::String(value) => {
                let mut rest = value.as_str();
                while let Some(offset) = rest.find("spotify:track:") {
                    let candidate = &rest[offset..];
                    let id = candidate["spotify:track:".len()..]
                        .chars()
                        .take_while(char::is_ascii_alphanumeric)
                        .collect::<String>();
                    if id.len() == 22 {
                        let uri = format!("spotify:track:{id}");
                        if seen.insert(uri.clone()) {
                            uris.push(uri);
                        }
                    }
                    rest = &candidate["spotify:track:".len()..];
                }
            }
            _ => {}
        }
    }

    let value = serde_json::from_slice(json)?;
    let mut seen = HashSet::new();
    let mut uris = Vec::new();
    visit(&value, &mut seen, &mut uris);
    Ok(uris)
}

fn playback_token_entry() -> Result<Entry> {
    Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(Into::into)
}

fn playback_refresh_token() -> Result<Option<String>> {
    match playback_token_entry()?.get_password() {
        Ok(token) if token == LOGGED_OUT_CREDENTIAL => Ok(None),
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(anyhow!(error)),
    }
}

fn save_playback_refresh_token(refresh_token: &str) -> Result<()> {
    let refresh_token = validate_refresh_token(refresh_token)?;
    playback_token_entry()?
        .set_password(refresh_token)
        .map_err(Into::into)
}

async fn load_playback_refresh_token() -> Result<Option<String>> {
    credential_worker::run(playback_refresh_token).await
}

async fn persist_playback_refresh_token(refresh_token: String) -> Result<()> {
    credential_worker::run(move || save_playback_refresh_token(&refresh_token)).await
}

/// Whether the next playback connection evens out loudness. Set from the saved
/// preference at startup and whenever the listener changes it.
static NORMALISE_VOLUME: AtomicBool = AtomicBool::new(false);

/// The streaming bitrate in kbps, read when playback connects. Very high
/// (320) unless the listener chose lower.
static STREAMING_KBPS: AtomicU16 = AtomicU16::new(320);

pub fn set_streaming_kbps(kbps: u16) {
    STREAMING_KBPS.store(kbps, Ordering::Relaxed);
}

pub fn set_normalisation(enabled: bool) {
    NORMALISE_VOLUME.store(enabled, Ordering::Relaxed);
}

/// Spotify's streaming service only accepts Premium accounts.
#[derive(Debug)]
pub struct PremiumRequired;

impl std::fmt::Display for PremiumRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Spotify Premium is required")
    }
}

impl std::error::Error for PremiumRequired {}

/// The plan normally arrives within milliseconds; give a slow link a few seconds.
const ACCOUNT_TYPE_POLLS: u32 = 60;
const ACCOUNT_TYPE_POLL_DELAY: Duration = Duration::from_millis(100);

fn validate_refresh_token(refresh_token: &str) -> Result<&str> {
    if refresh_token.is_empty() {
        Err(anyhow!("Spotify returned an empty playback refresh token"))
    } else {
        Ok(refresh_token)
    }
}

pub async fn delete_playback_refresh_token() -> Result<()> {
    credential_worker::run(|| match playback_token_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(delete_error) => playback_token_entry()?
            .set_password(LOGGED_OUT_CREDENTIAL)
            .with_context(|| format!("could not invalidate credential after {delete_error}")),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{LocalFilePaths, Playback, extract_track_uris, validate_refresh_token};
    use librespot::core::SpotifyUri;
    use std::path::PathBuf;

    #[test]
    fn local_file_paths_follow_the_latest_load_and_stay_bounded() {
        let paths = LocalFilePaths::default();
        let uri = |n: u32| SpotifyUri::from_uri(&format!("spotify:local:A::T{n}:60")).unwrap();
        paths.remember(uri(0), PathBuf::from("first.mp3"));
        paths.remember(uri(0), PathBuf::from("moved.mp3"));
        assert_eq!(paths.find(&uri(0)), Some(PathBuf::from("moved.mp3")));
        for n in 1..=super::REMEMBERED_LOCAL_FILES as u32 {
            paths.remember(uri(n), PathBuf::from(format!("{n}.mp3")));
        }
        assert_eq!(paths.find(&uri(0)), None);
        assert_eq!(paths.0.lock().unwrap().len(), super::REMEMBERED_LOCAL_FILES);
    }

    #[test]
    fn empty_refresh_tokens_are_rejected() {
        assert!(validate_refresh_token("").is_err());
        assert_eq!(
            validate_refresh_token("refresh-token").unwrap(),
            "refresh-token"
        );
    }

    #[tokio::test]
    async fn prepared_authorization_has_pkce_state_and_expected_redirect() {
        let authorization = Playback::prepare_authorization().await.unwrap();
        let url = url::Url::parse(authorization.url()).unwrap();
        let parameters = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(url.host_str(), Some("accounts.spotify.com"));
        assert_eq!(parameters.get("scope").unwrap(), "streaming");
        assert_eq!(parameters.get("code_challenge_method").unwrap(), "S256");
        assert!(parameters.contains_key("code_challenge"));
        assert!(parameters.contains_key("state"));
    }

    #[test]
    fn extracts_unique_track_uris_from_nested_radio_json() {
        let json = br#"{
            "items":[
                {"uri":"spotify:track:0123456789ABCDEFGHIJKL"},
                {"metadata":{"context":"before spotify:track:abcdefghijklmnopqrstuv after"}},
                {"uri":"spotify:track:0123456789ABCDEFGHIJKL"},
                {"uri":"spotify:album:0123456789ABCDEFGHIJKL"},
                {"uri":"spotify:track:short"}
            ]
        }"#;

        assert_eq!(
            extract_track_uris(json).unwrap(),
            vec![
                "spotify:track:0123456789ABCDEFGHIJKL",
                "spotify:track:abcdefghijklmnopqrstuv"
            ]
        );
    }
}

/// Local files through librespot's real player, as Sonder configures it,
/// into a sink that records what would reach the speakers.
#[cfg(test)]
mod local_playback_tests {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use librespot::{
        core::{config::SessionConfig, session::Session},
        playback::{
            audio_backend::{Sink, SinkResult},
            config::PlayerConfig,
            convert::Converter,
            decoder::AudioPacket,
            mixer::NoOpVolume,
            player::{Player, PlayerEvent},
        },
    };

    use super::{LocalFilePaths, playback_uri};
    use crate::local_files::{LocalLibrary, scan, write_test_wav};

    struct Recorder(Arc<Mutex<usize>>);

    impl Sink for Recorder {
        fn write(&mut self, packet: AudioPacket, _: &mut Converter) -> SinkResult<()> {
            *self.0.lock().unwrap() += packet.samples().map_or(0, <[f64]>::len);
            Ok(())
        }
    }

    /// Plays `file` to the end and returns the stereo frames the sink got at
    /// 44.1 kHz, or `None` when the player reported it unplayable.
    async fn play_to_end(file: &Path) -> Option<usize> {
        let folder = file.parent().unwrap().to_path_buf();
        let entries = scan(&[folder], Vec::new(), &|| false).unwrap();
        let library = LocalLibrary::from_index(entries);
        let track = library
            .tracks
            .iter()
            .find(|track| Path::new(&track.source_id) == file)
            .expect("the scan reads the file")
            .clone();

        let paths = LocalFilePaths::default();
        let resolver = paths.clone();
        let config = PlayerConfig {
            local_file_resolver: Some(Arc::new(move |uri| resolver.find(uri))),
            ..PlayerConfig::default()
        };
        let samples = Arc::new(Mutex::new(0));
        let recorded = samples.clone();
        let player = Player::new(
            config,
            Session::new(SessionConfig::default(), None),
            Box::new(NoOpVolume),
            move || Box::new(Recorder(recorded)),
        );
        let mut events = player.get_player_event_channel();
        let uri = playback_uri(&track).unwrap();
        paths.remember(uri.clone(), PathBuf::from(&track.source_id));
        player.load(uri, true, 0);
        let finished = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = events.recv().await {
                match event {
                    PlayerEvent::EndOfTrack { .. } => return true,
                    PlayerEvent::Unavailable { .. } => return false,
                    _ => {}
                }
            }
            false
        })
        .await
        .expect("the player answers within 30 seconds");
        player.stop();
        finished.then(|| *samples.lock().unwrap() / 2)
    }

    fn temp_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!(
            "sonder-local-playback-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Spotify's own format and two that upstream librespot refuses: another
    /// sample rate, and mono.
    #[tokio::test(flavor = "multi_thread")]
    async fn local_files_play_through_the_player_at_any_rate_and_channel_count() {
        for (rate, channels) in [(44_100, 2), (48_000, 1), (22_050, 2)] {
            let folder = temp_folder(&format!("{rate}-{channels}"));
            let file = folder.join("Ana Lind - Harbour.wav");
            write_test_wav(&file, rate, channels, 2);
            let frames = play_to_end(&file)
                .await
                .unwrap_or_else(|| panic!("{rate} Hz, {channels} channel(s) did not play"));
            // Exactly two seconds at 44.1 kHz.
            assert_eq!(frames, 88_200, "{rate} Hz, {channels} channel(s)");
            let _ = std::fs::remove_dir_all(&folder);
        }
    }

    /// Real encoded files, when `SONDER_LOCAL_FILES_SAMPLES` names a folder of
    /// them: each must play to the end with about its own length in audio.
    #[tokio::test(flavor = "multi_thread")]
    async fn sample_folder_plays_when_provided() {
        let Some(folder) = std::env::var_os("SONDER_LOCAL_FILES_SAMPLES") else {
            return;
        };
        let entries = scan(&[PathBuf::from(folder)], Vec::new(), &|| false).unwrap();
        let library = LocalLibrary::from_index(entries);
        assert_eq!(library.unreadable, 0, "every sample can be read");
        for track in &library.tracks {
            let frames = play_to_end(Path::new(&track.source_id))
                .await
                .unwrap_or_else(|| panic!("{} did not play", track.source_id));
            let expected = u64::from(track.duration_ms) * 44_100 / 1000;
            let difference = (frames as u64).abs_diff(expected);
            assert!(
                difference < 44_100 / 4,
                "{}: {frames} frames, expected about {expected}",
                track.source_id
            );
            eprintln!("played {} ({frames} frames)", track.source_id);
        }
    }
}
