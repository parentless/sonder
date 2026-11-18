//! Spotify Soloist: Spotify's own headless player, driven over a local
//! WebSocket. Sonder uses it for accounts Spotify won't unlock songs for in
//! the built-in player. Spotify asks apps not to redistribute it, so Sonder
//! fetches it from Spotify's own download address when the listener sets it
//! up, runs it with the listener's own key, and keeps it current: builds
//! expire 90 days after they are made.
//!
//! Spotify's documentation: developer.spotify.com/documentation/soloist

mod protocol;
mod tracker;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow, bail};
use directories::ProjectDirs;
use futures::{SinkExt as _, StreamExt as _};
use keyring::Entry;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    sync::{broadcast, mpsc, watch},
    task::AbortHandle,
};
use tokio_tungstenite::tungstenite::Message;

pub use tracker::Signal;
use tracker::{Out, Tracker};

use crate::credential_worker;

/// Where a listener gets the key Soloist needs.
pub const KEY_PAGE_URL: &str = "https://developer.spotify.com/dashboard/soloist";

/// The name Spotify's apps show for the player.
pub const DEVICE_NAME: &str = "Sonder";

const KEYCHAIN_SERVICE: &str = "dev.twoducks.cadence";
const KEYCHAIN_ACCOUNT: &str = "soloist-api-key";

/// A build expires this long after it was made, and Sonder replaces it a while
/// before that.
const BUILD_LIFETIME_DAYS: i64 = 90;
const REFRESH_WITHIN_DAYS: i64 = 14;
/// Soloist exits with this when its build has expired.
const EXPIRED_EXIT_CODE: i32 = 10;
/// The archives are about 10 MB; this only stops a runaway download.
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
/// How much audio Soloist may keep on disk.
const CACHE_MEGABYTES: &str = "500";
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// Long enough to find the phone and pick the player; not forever.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_millis(500);
const OUTPUT_LINES: usize = 12;

/// Where setup has got to, for the window to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// There is no key yet; the listener has to paste one.
    NeedKey,
    Downloading,
    /// Soloist is running and waiting for the listener to choose it in a
    /// Spotify app.
    Pairing,
    Done,
    Failed(String),
}

/// Setup failed for a reason the listener can act on.
#[derive(Debug)]
pub struct SetupError(pub String);

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetupError {}

fn setup_error(message: impl Into<String>) -> anyhow::Error {
    SetupError(message.into()).into()
}

/// Soloist's own build has run out.
#[derive(Debug)]
struct Expired;

impl std::fmt::Display for Expired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Spotify’s player has expired")
    }
}

impl std::error::Error for Expired {}

struct Archive {
    url: &'static str,
    extension: &'static str,
}

/// Spotify publishes builds for these. There is none for Intel Macs, Windows
/// on ARM or anything else Sonder runs on.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ARCHIVE: Option<Archive> = Some(Archive {
    url: "https://soloist-builds.spotifycdn.com/soloist_release_windows_x86_64.zip",
    extension: "zip",
});
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ARCHIVE: Option<Archive> = Some(Archive {
    url: "https://soloist-builds.spotifycdn.com/soloist_release_osx_arm64.tar.gz",
    extension: "tar.gz",
});
#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const ARCHIVE: Option<Archive> = None;

#[cfg(target_os = "windows")]
const EXECUTABLE: &str = "soloist.exe";
#[cfg(not(target_os = "windows"))]
const EXECUTABLE: &str = "soloist";

/// Whether Spotify makes Soloist for this computer.
pub fn supported() -> bool {
    ARCHIVE.is_some()
}

fn root() -> Result<PathBuf> {
    Ok(ProjectDirs::from("dev", "twoducks", "cadence")
        .context("could not find a folder for Spotify’s player")?
        .data_local_dir()
        .join("soloist"))
}

fn executable(root: &Path) -> PathBuf {
    root.join("bin").join(EXECUTABLE)
}

fn data_dir(root: &Path) -> PathBuf {
    root.join("data")
}

fn cache_dir(root: &Path) -> PathBuf {
    root.join("cache")
}

// ---- the key -------------------------------------------------------------

fn key_entry() -> Result<Entry> {
    Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(Into::into)
}

/// The key the listener saved, if any.
pub async fn saved_key() -> Result<Option<String>> {
    credential_worker::run(|| match key_entry()?.get_password() {
        Ok(key) if key.trim().is_empty() => Ok(None),
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(anyhow!(error)),
    })
    .await
}

async fn save_key(key: String) -> Result<()> {
    credential_worker::run(move || key_entry()?.set_password(&key).map_err(Into::into)).await
}

/// Forgets the key and the pairing, so the next setup starts clean.
pub async fn forget() -> Result<()> {
    credential_worker::run(|| match key_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(anyhow!(error)),
    })
    .await?;
    let data = data_dir(&root()?);
    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(data) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })
    .await??;
    Ok(())
}

/// Soloist takes its key only on the command line, so the key must never end
/// up in anything shown or logged.
fn scrub(line: &str, key: &str) -> String {
    if key.is_empty() {
        line.to_owned()
    } else {
        line.replace(key, "…")
    }
}

// ---- installing ----------------------------------------------------------

/// The date a build was made, from `soloist --version`:
/// `soloist 1.3.10.83 build 1795024918 (20261118) (g19412df6ea) (macos/aarch64)`.
fn build_day(version: &str) -> Option<i64> {
    version.split(['(', ')']).find_map(|part| {
        let digits = part.trim();
        if digits.len() != 8 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let year: i64 = digits[..4].parse().ok()?;
        let month: i64 = digits[4..6].parse().ok()?;
        let day: i64 = digits[6..].parse().ok()?;
        ((1..=12).contains(&month) && (1..=31).contains(&day))
            .then(|| days_from_civil(year, month, day))
    })
}

/// Days since 1970-01-01 for a calendar date (Howard Hinnant's algorithm), so
/// no date library is needed to count to 90.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn today() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    i64::try_from(seconds / 86_400).unwrap_or(0)
}

fn days_left(build_day: i64, today: i64) -> i64 {
    BUILD_LIFETIME_DAYS - (today - build_day)
}

async fn version_of(executable: &Path) -> Result<String> {
    let output = tokio::process::Command::new(executable)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context("could not run Spotify’s player")?;
    if !output.status.success() {
        bail!("Spotify’s player did not report its version");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Days left before the installed build expires, or `None` if none is
/// installed or it cannot be run.
async fn installed_days_left(root: &Path) -> Option<i64> {
    let exe = executable(root);
    if !exe.is_file() {
        return None;
    }
    let version = version_of(&exe).await.ok()?;
    Some(days_left(build_day(&version)?, today()))
}

/// Whether setup has to download before it can start Soloist.
async fn needs_download(root: &Path) -> bool {
    installed_days_left(root)
        .await
        .is_none_or(|left| left <= REFRESH_WITHIN_DAYS)
}

#[cfg(target_os = "windows")]
fn tar_program() -> PathBuf {
    // Windows ships a tar that reads zip files. A tar from Git or MSYS earlier
    // in the path may not, so ask for Windows' own.
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
        .unwrap_or_else(|| PathBuf::from("tar.exe"))
}

#[cfg(not(target_os = "windows"))]
fn tar_program() -> PathBuf {
    PathBuf::from("tar")
}

async fn download(archive: &Archive) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .user_agent(concat!("Sonder/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut response = client
        .get(archive.url)
        .send()
        .await
        .context("could not reach Spotify to download its player")?
        .error_for_status()
        .context("Spotify would not send its player")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_ARCHIVE_BYTES {
            bail!("Spotify’s player download was larger than expected");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        bail!("Spotify’s player download was empty");
    }
    Ok(bytes)
}

/// Downloads Soloist from Spotify, checks that it runs, and puts it in place.
/// Nothing in `bin` changes unless the new build starts.
async fn install(root: &Path) -> Result<()> {
    let archive = ARCHIVE
        .as_ref()
        .context("no Spotify player for this computer")?;
    let bytes = download(archive).await?;
    tokio::fs::create_dir_all(root).await?;
    let file = root.join(format!("download.{}", archive.extension));
    let staging = root.join("staging");
    tokio::fs::write(&file, &bytes).await?;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;
    let extracted = tokio::process::Command::new(tar_program())
        .arg("-xf")
        .arg(&file)
        .arg("-C")
        .arg(&staging)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("could not unpack Spotify’s player")?;
    let _ = tokio::fs::remove_file(&file).await;
    if !extracted.status.success() {
        bail!(
            "could not unpack Spotify’s player: {}",
            String::from_utf8_lossy(&extracted.stderr).trim()
        );
    }
    let staged = staging.join(EXECUTABLE);
    if !staged.is_file() {
        bail!("Spotify’s player was not in its download");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).await?;
    }
    let version = version_of(&staged).await?;
    if build_day(&version).is_some_and(|day| days_left(day, today()) <= 0) {
        bail!("Spotify’s latest player has already expired");
    }
    let bin = root.join("bin");
    let _ = tokio::fs::remove_dir_all(&bin).await;
    tokio::fs::rename(&staging, &bin)
        .await
        .context("could not put Spotify’s player in place")?;
    log::info!("soloist: installed {version}");
    Ok(())
}

// ---- running it ----------------------------------------------------------

/// A Soloist left running by a Sonder that did not get to close it still holds
/// the data folder, and a new one would refuse to start. Stops it, but only if
/// the process with that number really is Soloist.
fn stop_stale(data: &Path) {
    let Some(pid) = std::fs::read_to_string(data.join("soloist.pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return;
    };
    #[cfg(unix)]
    {
        let name = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).to_lowercase())
            .unwrap_or_default();
        if name.contains("soloist")
            && let Ok(pid) = libc::pid_t::try_from(pid)
        {
            log::info!("soloist: stopping a player left running");
            // SAFETY: a plain signal to a process we just confirmed is ours.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            std::thread::sleep(Duration::from_millis(400));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const NO_WINDOW: u32 = 0x0800_0000;
        let listing = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .creation_flags(NO_WINDOW)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).to_lowercase())
            .unwrap_or_default();
        if listing.contains("soloist") {
            log::info!("soloist: stopping a player left running");
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .creation_flags(NO_WINDOW)
                .output();
            std::thread::sleep(Duration::from_millis(400));
        }
    }
}

/// The last lines Soloist printed, for saying why it stopped.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<VecDeque<String>>>);

impl Output {
    fn push(&self, line: String) {
        let mut lines = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if lines.len() == OUTPUT_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    fn last(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .rev()
            .find(|line| !line.trim().is_empty())
            .cloned()
    }
}

struct Running {
    child: tokio::process::Child,
    address: String,
    output: Output,
}

async fn spawn(root: &Path, key: &str) -> Result<Running> {
    let exe = executable(root);
    let data = data_dir(root);
    let cache = cache_dir(root);
    tokio::fs::create_dir_all(&data).await?;
    tokio::fs::create_dir_all(&cache).await?;
    {
        let data = data.clone();
        tokio::task::spawn_blocking(move || stop_stale(&data)).await?;
    }
    let _ = tokio::fs::remove_file(data.join("ws.port")).await;
    let _ = tokio::fs::remove_file(data.join("ws.addr")).await;
    let mut command = tokio::process::Command::new(&exe);
    command
        .args(["-n", DEVICE_NAME, "-k", key, "-z", CACHE_MEGABYTES])
        .arg("-D")
        .arg(&data)
        .arg("-C")
        .arg(&cache)
        .args(["-w", "127.0.0.1:0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .context("could not start Spotify’s player")?;
    let output = Output::default();
    if let Some(stream) = child.stdout.take() {
        forward(stream, output.clone(), key);
    }
    if let Some(stream) = child.stderr.take() {
        forward(stream, output.clone(), key);
    }
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            if status.code() == Some(EXPIRED_EXIT_CODE) {
                return Err(Expired.into());
            }
            // Let the last of its output arrive.
            tokio::time::sleep(Duration::from_millis(150)).await;
            let detail = output
                .last()
                .map_or_else(String::new, |line| format!(": {line}"));
            return Err(setup_error(format!(
                "Spotify’s player stopped as it started{detail}"
            )));
        }
        let port = tokio::fs::read_to_string(data.join("ws.port")).await;
        if let Ok(port) = port
            && let Ok(port) = port.trim().parse::<u16>()
        {
            let host = tokio::fs::read_to_string(data.join("ws.addr"))
                .await
                .map_or_else(|_| "127.0.0.1".to_owned(), |host| host.trim().to_owned());
            // Only ever the computer's own address; the API has no login.
            let host = if host == "127.0.0.1" || host == "localhost" {
                host
            } else {
                "127.0.0.1".to_owned()
            };
            return Ok(Running {
                child,
                address: format!("{host}:{port}"),
                output,
            });
        }
        if tokio::time::Instant::now() > deadline {
            bail!("Spotify’s player did not start in time");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Keeps what a stream says for the log and for saying why Soloist stopped.
fn forward<R>(stream: R, output: Output, key: &str)
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
{
    let key = key.to_owned();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = scrub(&line, &key);
            log::debug!("soloist: {line}");
            output.push(line);
        }
    });
}

// ---- talking to it -------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Auth {
    logged_in: bool,
    is_active: bool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// What the socket and clock tasks share with the player the app holds.
struct Shared {
    commands: mpsc::UnboundedSender<Value>,
    signals: broadcast::Sender<Signal>,
    tracker: Mutex<Tracker>,
    auth: watch::Sender<Option<Auth>>,
    alive: AtomicBool,
    output: Output,
}

impl Shared {
    fn dispatch(&self, out: Out) {
        for command in out.commands {
            let _ = self.commands.send(command);
        }
        for signal in out.signals {
            let _ = self.signals.send(signal);
        }
    }

    fn with_tracker(&self, change: impl FnOnce(&mut Tracker) -> Out) {
        let out = change(
            &mut self
                .tracker
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        self.dispatch(out);
    }

    fn read(&self, text: &str) {
        match protocol::parse_event(text) {
            protocol::Event::Auth {
                logged_in,
                is_active,
            } => {
                self.auth.send_replace(Some(Auth {
                    logged_in,
                    is_active,
                }));
            }
            protocol::Event::Playback(update) => {
                self.with_tracker(|tracker| tracker.on_update(&update, now_ms()));
            }
            protocol::Event::Error(message) => {
                log::warn!("soloist: {message}");
                self.with_tracker(Tracker::on_error);
            }
            protocol::Event::Other => {}
        }
    }
}

/// Spotify's own player, running beside Sonder.
#[derive(Clone)]
pub struct Player {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    tasks: Vec<AbortHandle>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The task that owns the process is among these; dropping it stops
        // Soloist.
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Player {
    /// Opens the socket of a Soloist that is already running. The process, if
    /// there is one, stops when the player is dropped.
    async fn attach(
        address: &str,
        child: Option<tokio::process::Child>,
        output: Output,
    ) -> Result<Self> {
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(format!("ws://{address}")),
        )
        .await
        .context("Spotify’s player did not answer")?
        .context("could not connect to Spotify’s player")?;
        let (mut sink, mut stream) = socket.split();
        let (commands, mut outgoing) = mpsc::unbounded_channel::<Value>();
        let (signals, _) = broadcast::channel(256);
        let (auth, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            commands,
            signals,
            tracker: Mutex::new(Tracker::default()),
            auth,
            alive: AtomicBool::new(true),
            output,
        });
        let mut tasks = Vec::new();
        tasks.push(
            tokio::spawn({
                let shared = shared.clone();
                async move {
                    loop {
                        tokio::select! {
                            frame = stream.next() => match frame {
                                Some(Ok(Message::Text(text))) => shared.read(text.as_str()),
                                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                Some(Ok(_)) => {}
                            },
                            command = outgoing.recv() => match command {
                                Some(command) => {
                                    if sink.send(Message::text(command.to_string())).await.is_err() {
                                        break;
                                    }
                                }
                                None => break,
                            },
                        }
                    }
                    log::info!("soloist: connection closed");
                    shared.alive.store(false, Ordering::Release);
                }
            })
            .abort_handle(),
        );
        tasks.push(
            tokio::spawn({
                let shared = shared.clone();
                async move {
                    let mut tick = tokio::time::interval(TICK);
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        shared.with_tracker(|tracker| tracker.tick(now_ms()));
                    }
                }
            })
            .abort_handle(),
        );
        if let Some(mut child) = child {
            tasks.push(
                tokio::spawn({
                    let shared = shared.clone();
                    async move {
                        let status = child.wait().await;
                        log::warn!("soloist: the player exited: {status:?}");
                        shared.alive.store(false, Ordering::Release);
                    }
                })
                .abort_handle(),
            );
        }
        Ok(Self {
            inner: Arc::new(Inner { shared, tasks }),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.inner.shared.alive.load(Ordering::Acquire)
    }

    /// What the player reports, as the app understands it.
    pub fn signals(&self) -> broadcast::Receiver<Signal> {
        self.inner.shared.signals.subscribe()
    }

    /// Opens a song. Soloist plays Spotify songs; a local file is not one.
    pub fn load(&self, uri: &str, playing: bool, position_ms: u32) {
        let shared = &self.inner.shared;
        // A player Spotify hasn't picked as the active device may ignore play.
        let active = shared.auth.borrow().is_some_and(|auth| auth.is_active);
        if playing && !active {
            let _ = shared.commands.send(protocol::command("activate"));
        }
        shared.with_tracker(|tracker| tracker.load(uri, playing, position_ms, now_ms()));
    }

    /// Reports a song this player can't take, as if Soloist had refused it.
    pub fn reject(&self, uri: &str, reason: &'static str) {
        let _ = self.inner.shared.signals.send(Signal::Unavailable {
            uri: uri.to_owned(),
            reason,
        });
    }

    pub fn play(&self) {
        self.inner
            .shared
            .with_tracker(|tracker| tracker.play(now_ms()));
    }

    pub fn pause(&self) {
        self.inner.shared.with_tracker(Tracker::pause);
    }

    pub fn seek(&self, position_ms: u32) {
        self.inner
            .shared
            .with_tracker(|tracker| tracker.seek(position_ms, now_ms()));
    }

    pub fn stop(&self) {
        self.inner.shared.with_tracker(Tracker::stop);
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self
            .inner
            .shared
            .commands
            .send(protocol::set_volume(volume));
    }

    /// Waits until Spotify has signed Soloist in, or says why it has not.
    async fn wait_until_signed_in(&self, timeout: Duration) -> Result<()> {
        let shared = &self.inner.shared;
        let mut auth = shared.auth.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if auth.borrow().is_some_and(|auth| auth.logged_in) {
                return Ok(());
            }
            if !self.is_connected() {
                let detail = shared
                    .output
                    .last()
                    .map_or_else(String::new, |line| format!(": {line}"));
                return Err(setup_error(format!(
                    "Spotify’s player stopped before it was ready{detail}"
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(setup_error(
                    "No Spotify app chose “Sonder” in time. Open Spotify, pick “Sonder” from the devices, and try again.",
                ));
            }
            let _ = tokio::time::timeout(Duration::from_millis(500), auth.changed()).await;
        }
    }

    async fn wait_for_first_report(&self) -> Result<()> {
        let mut auth = self.inner.shared.auth.subscribe();
        tokio::time::timeout(Duration::from_secs(10), auth.wait_for(Option::is_some))
            .await
            .map_err(|_| setup_error("Spotify’s player did not say whether it was signed in"))?
            .map_err(|_| setup_error("Spotify’s player stopped before it was ready"))?;
        Ok(())
    }

    fn signed_in(&self) -> bool {
        self.inner
            .shared
            .auth
            .borrow()
            .is_some_and(|auth| auth.logged_in)
    }
}

/// Starts Soloist and returns the player once it is signed in. With
/// `interactive`, a Soloist that is not yet signed in waits for the listener to
/// choose it in a Spotify app; without it, that is an error, since nobody is
/// there to do it.
pub async fn connect(
    key: String,
    interactive: bool,
    report: impl Fn(Step) + Send + Sync,
) -> Result<Player> {
    if !supported() {
        return Err(setup_error(
            "Spotify doesn’t make its player for this computer yet.",
        ));
    }
    let root = root()?;
    if needs_download(&root).await {
        report(Step::Downloading);
        if let Err(error) = install(&root).await {
            if installed_days_left(&root)
                .await
                .is_some_and(|left| left > 0)
            {
                log::warn!("soloist: could not update, keeping the current build: {error:#}");
            } else {
                return Err(setup_error(format!("{error:#}")));
            }
        }
    }
    let running = match spawn(&root, &key).await {
        Err(error) if error.downcast_ref::<Expired>().is_some() => {
            report(Step::Downloading);
            install(&root)
                .await
                .map_err(|error| setup_error(format!("{error:#}")))?;
            spawn(&root, &key).await?
        }
        started => started?,
    };
    let player = Player::attach(&running.address, Some(running.child), running.output).await?;
    player.wait_for_first_report().await?;
    if !player.signed_in() {
        if !interactive {
            return Err(setup_error(
                "Spotify’s player isn’t signed in to your account yet.",
            ));
        }
        report(Step::Pairing);
        player.wait_until_signed_in(PAIRING_TIMEOUT).await?;
    }
    // Saved only now, so a key that never worked is not kept.
    if interactive {
        save_key(key).await?;
    }
    Ok(player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[test]
    fn the_build_date_is_read_from_the_version_line() {
        let line = "soloist 1.3.10.83 build 1795024918 (20261118) (g19412df6ea) (macos/aarch64)";
        assert_eq!(build_day(line), Some(days_from_civil(2026, 11, 18)));
        assert_eq!(build_day("soloist 1.0"), None);
        assert_eq!(build_day("x (20261399) y"), None);
    }

    #[test]
    fn dates_count_in_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(
            days_from_civil(2026, 11, 18) - days_from_civil(2026, 11, 17),
            1
        );
        // 2028 is a leap year.
        assert_eq!(
            days_from_civil(2028, 3, 1) - days_from_civil(2028, 2, 28),
            2
        );
    }

    #[test]
    fn a_build_lasts_ninety_days() {
        let made = days_from_civil(2026, 11, 18);
        assert_eq!(days_left(made, made), 90);
        assert_eq!(days_left(made, made + 76), 14);
        assert_eq!(days_left(made, made + 90), 0);
        assert!(days_left(made, made + 91) < 0);
    }

    #[test]
    fn the_key_is_hidden_from_anything_soloist_prints() {
        assert_eq!(
            scrub("key secret123 rejected", "secret123"),
            "key … rejected"
        );
        assert_eq!(scrub("nothing here", ""), "nothing here");
    }

    /// A stand-in Soloist: says it is signed in, then answers what it is asked
    /// the way the real one does.
    async fn fake_soloist(
        script: impl FnOnce(
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        ) -> futures::future::BoxFuture<'static, ()>
        + Send
        + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = accept_async(stream).await.unwrap();
            script(socket).await;
        });
        address
    }

    async fn next_signal(signals: &mut broadcast::Receiver<Signal>) -> Signal {
        tokio::time::timeout(Duration::from_secs(5), signals.recv())
            .await
            .expect("a signal in time")
            .expect("an open channel")
    }

    #[tokio::test]
    async fn a_song_loads_plays_and_ends_through_the_socket() {
        use futures::future::FutureExt as _;
        let address = fake_soloist(|mut socket| {
            async move {
                socket
                    .send(Message::text(
                        r#"{"type":"auth_state","logged_in":true,"is_active":true,"device_name":"Sonder"}"#,
                    ))
                    .await
                    .unwrap();
                // The player asks for a song.
                let Some(Ok(Message::Text(command))) = socket.next().await else {
                    panic!("expected a command");
                };
                let command: Value = serde_json::from_str(command.as_str()).unwrap();
                assert_eq!(command["command"], "play");
                let uri = command["uri"].as_str().unwrap().to_owned();
                let now = now_ms();
                let state = |status: &str, position: u64| {
                    format!(
                        r#"{{"type":"playback_state","status":"{status}","item":{{"uri":"{uri}","decorations":{{"playback":{{"duration_ms":2000}}}}}},"position":{{"position_ms":{position},"timestamp_ms":{now},"speed":1.0}}}}"#
                    )
                };
                socket.send(Message::text(state("playing", 0))).await.unwrap();
                tokio::time::sleep(Duration::from_millis(1600)).await;
                socket.send(Message::text(state("idle", 0))).await.unwrap();
                // Keep the socket open until the player is done with it.
                let _ = socket.next().await;
            }
            .boxed()
        })
        .await;
        let player = Player::attach(&address, None, Output::default())
            .await
            .unwrap();
        player.wait_for_first_report().await.unwrap();
        assert!(player.signed_in());
        let mut signals = player.signals();
        player.load("spotify:track:one", true, 0);
        assert_eq!(
            next_signal(&mut signals).await,
            Signal::Loading {
                uri: "spotify:track:one".into()
            }
        );
        // The report is stamped when the stand-in sends it, so a slow machine
        // may read a few milliseconds in.
        match next_signal(&mut signals).await {
            Signal::Playing { uri, position_ms } => {
                assert_eq!(uri, "spotify:track:one");
                assert!(position_ms < 500, "started at {position_ms} ms");
            }
            other => panic!("expected the song to start, got {other:?}"),
        }
        // The clock runs while it plays, then the song ends.
        loop {
            match next_signal(&mut signals).await {
                Signal::Position { .. } => continue,
                signal => {
                    assert_eq!(
                        signal,
                        Signal::EndOfTrack {
                            uri: "spotify:track:one".into()
                        }
                    );
                    break;
                }
            }
        }
        assert!(player.is_connected());
    }

    #[tokio::test]
    async fn a_closed_socket_marks_the_player_gone() {
        use futures::future::FutureExt as _;
        let address = fake_soloist(|mut socket| {
            async move {
                socket
                    .send(Message::text(
                        r#"{"type":"auth_state","logged_in":false,"is_active":false}"#,
                    ))
                    .await
                    .unwrap();
                let _ = socket.close(None).await;
            }
            .boxed()
        })
        .await;
        let player = Player::attach(&address, None, Output::default())
            .await
            .unwrap();
        let waited = player.wait_until_signed_in(Duration::from_secs(5)).await;
        assert!(waited.is_err());
        assert!(!player.is_connected());
    }

    #[tokio::test]
    async fn pairing_finishes_when_spotify_signs_soloist_in() {
        use futures::future::FutureExt as _;
        let address = fake_soloist(|mut socket| {
            async move {
                socket
                    .send(Message::text(
                        r#"{"type":"auth_state","logged_in":false,"is_active":false}"#,
                    ))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(300)).await;
                socket
                    .send(Message::text(
                        r#"{"type":"auth_state","logged_in":true,"is_active":true}"#,
                    ))
                    .await
                    .unwrap();
                let _ = socket.next().await;
            }
            .boxed()
        })
        .await;
        let player = Player::attach(&address, None, Output::default())
            .await
            .unwrap();
        player.wait_for_first_report().await.unwrap();
        assert!(!player.signed_in());
        player
            .wait_until_signed_in(Duration::from_secs(5))
            .await
            .unwrap();
        assert!(player.signed_in());
    }
}
