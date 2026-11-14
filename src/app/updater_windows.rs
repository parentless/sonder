//! Windows releases use the same repository's portable executable and checksum.
//! A short-lived helper replaces the executable after orderly app shutdown.
use super::*;
use crate::update_release::Release;
use anyhow::{Context as _, Result, ensure};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

#[derive(Default)]
pub(super) struct UpdateStatus {
    pub(super) check: UpdateCheck,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum UpdateCheck {
    #[default]
    Unknown,
    Checking,
    Downloading,
    /// Downloaded and verified; it installs on restart or when Sonder quits.
    Ready(String),
    UpToDate,
    Available(String),
    #[allow(dead_code)] // Shared settings UI also displays Sparkle's skipped versions.
    Skipped(String),
    Failed(String),
}
struct Updater {
    status: Entity<UpdateStatus>,
    release: Option<Release>,
    automatic: bool,
    /// Download new versions in the background and install them at quit.
    /// Off until the listener turns it on.
    auto_install: bool,
    /// A verified download waiting for a restart.
    staged: Option<PathBuf>,
    busy: bool,
    /// The library has loaded, so a check no longer competes with startup.
    settled: bool,
    _timer: gpui_kit::Task<()>,
    _quit_hook: Entity<QuitHook>,
}
struct QuitHook;
impl gpui_kit::Global for Updater {}

fn preference_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("dev", "twoducks", "cadence")?
            .config_dir()
            .join("automatic-updates.json"),
    )
}

fn updater_mut(cx: &mut App) -> Option<&mut Updater> {
    cx.has_global::<Updater>()
        .then(|| cx.global_mut::<Updater>())
}

fn install_preference_path() -> Option<PathBuf> {
    Some(
        ProjectDirs::from("dev", "twoducks", "cadence")?
            .config_dir()
            .join("automatic-install.json"),
    )
}

pub(super) fn start(cx: &mut App) -> bool {
    // Source builds stay inert. Release CI pins the feed to the fork producing
    // this executable, rather than silently fetching upstream's macOS releases.
    if cfg!(debug_assertions) || option_env!("CADENCE_UPDATE_REPOSITORY").is_none() {
        return false;
    }
    let automatic = preference_path()
        .and_then(|p| fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(true);
    let auto_install = install_preference_path()
        .and_then(|p| fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(false);
    let status = cx.new(|_| UpdateStatus::default());
    // A downloaded update installs when Sonder quits, without relaunching.
    let quit_hook = cx.new(|cx| {
        cx.on_app_quit(|_, cx| {
            if let Some(staged) = updater_mut(cx).and_then(|updater| updater.staged.take())
                && let Err(error) = launch_helper(&staged, false)
            {
                log::warn!("could not install the downloaded update: {error:#}");
                let _ = fs::remove_file(staged);
            }
            async {}
        })
        .detach();
        QuitHook
    });
    let timer = cx.spawn(async move |cx| {
        // Stay out of the way of startup: wait for the library to load (or give
        // up waiting after 45 s), then let the first screen settle for a moment.
        for _ in 0..45 {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if cx.update(|cx| cx.try_global::<Updater>().is_some_and(|u| u.settled)) {
                break;
            }
        }
        cx.background_executor().timer(Duration::from_secs(3)).await;
        let mut delay = Duration::ZERO;
        loop {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                if cx
                    .try_global::<Updater>()
                    .is_some_and(|u| (u.automatic || u.auto_install) && !u.busy)
                {
                    check(cx, false);
                }
            });
            delay = Duration::from_secs(24 * 60 * 60);
        }
    });
    cx.set_global(Updater {
        status,
        release: None,
        automatic,
        auto_install,
        staged: None,
        busy: false,
        settled: false,
        _timer: timer,
        _quit_hook: quit_hook,
    });
    true
}

/// Called once the library has loaded; the first update check follows shortly.
pub(super) fn startup_settled(cx: &mut App) {
    if let Some(updater) = updater_mut(cx) {
        updater.settled = true;
    }
}

pub(super) fn status(cx: &App) -> Option<Entity<UpdateStatus>> {
    cx.try_global::<Updater>().map(|u| u.status.clone())
}
pub(super) fn automatic_checks(cx: &App) -> Option<bool> {
    cx.try_global::<Updater>().map(|u| u.automatic)
}
pub(super) fn set_automatic_checks(enabled: bool, cx: &mut App) {
    if let Some(path) = preference_path() {
        let result = (|| -> Result<()> {
            fs::create_dir_all(path.parent().context("missing config directory")?)?;
            fs::write(path, serde_json::to_vec(&enabled)?)?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("could not save update preference: {error}");
        }
    }
    if cx.has_global::<Updater>() {
        cx.global_mut::<Updater>().automatic = enabled;
    }
}
pub(super) fn automatic_install(cx: &App) -> Option<bool> {
    cx.try_global::<Updater>().map(|u| u.auto_install)
}
pub(super) fn set_automatic_install(enabled: bool, cx: &mut App) {
    if let Some(path) = install_preference_path() {
        let result = (|| -> Result<()> {
            fs::create_dir_all(path.parent().context("missing config directory")?)?;
            fs::write(path, serde_json::to_vec(&enabled)?)?;
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("could not save update preference: {error}");
        }
    }
    if !cx.has_global::<Updater>() {
        return;
    }
    cx.global_mut::<Updater>().auto_install = enabled;
    // Already found one? Fetch it now rather than at tomorrow's check.
    if enabled {
        download_in_background(cx);
    }
}
pub(super) fn check_for_updates(cx: &mut App) {
    if cx
        .try_global::<Updater>()
        .is_some_and(|updater| updater.staged.is_some())
    {
        return restart_to_update(cx);
    }
    check(cx, true);
}

/// Hands the downloaded update to the installer and quits so it can replace
/// the executable. The installer starts the new version.
pub(super) fn restart_to_update(cx: &mut App) {
    let Some(staged) = updater_mut(cx).and_then(|updater| updater.staged.take()) else {
        return;
    };
    match launch_helper(&staged, true) {
        Ok(()) => cx.quit(),
        Err(error) => {
            log::warn!("could not start the update: {error:#}");
            let _ = fs::remove_file(&staged);
            let status = cx.global::<Updater>().status.clone();
            status.update(cx, |status, cx| {
                status.check = UpdateCheck::Failed(error.to_string());
                cx.notify();
            });
        }
    }
}

/// Downloads the release found by the last check, if there is one and nothing
/// else is under way. The work runs on its own thread and streams to disk.
fn download_in_background(cx: &mut App) {
    let Some(updater) = updater_mut(cx) else {
        return;
    };
    let Some(release) = updater.release.clone() else {
        return;
    };
    if updater.busy || updater.staged.is_some() {
        return;
    }
    updater.busy = true;
    let status = updater.status.clone();
    status.update(cx, |status, cx| {
        status.check = UpdateCheck::Downloading;
        cx.notify();
    });
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let client = reqwest::Client::builder()
                    .user_agent("Sonder Windows updater")
                    .timeout(Duration::from_secs(180))
                    .build()?;
                download_staged(&client, &release).await
            })
        })();
        let _ = sender.send_blocking(result.map(|path| (path, release.version)));
    });
    cx.spawn(async move |cx| {
        if let Ok(result) = receiver.recv().await {
            cx.update(|cx| {
                let updater = cx.global_mut::<Updater>();
                updater.busy = false;
                let check = match result {
                    Ok((path, version)) => {
                        updater.staged = Some(path);
                        UpdateCheck::Ready(version)
                    }
                    Err(error) => {
                        log::warn!("background update download failed: {error:#}");
                        UpdateCheck::Failed(error.to_string())
                    }
                };
                let status = updater.status.clone();
                status.update(cx, |status, cx| {
                    status.check = check;
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

fn check(cx: &mut App, install_if_available: bool) {
    if !cx.has_global::<Updater>() {
        return;
    }
    let updater = cx.global_mut::<Updater>();
    if updater.busy {
        return;
    }
    updater.busy = true;
    let release = if install_if_available {
        updater.release.clone()
    } else {
        None
    };
    let installing = release.is_some();
    let status = updater.status.clone();
    status.update(cx, |status, cx| {
        status.check = if installing {
            UpdateCheck::Downloading
        } else {
            UpdateCheck::Checking
        };
        cx.notify();
    });
    let (sender, receiver) = async_channel::bounded(1);
    // HTTP and disk work never runs on the UI or audio thread.
    std::thread::spawn(move || {
        let result = (|| -> Result<Option<Release>> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let client = reqwest::Client::builder()
                    .user_agent("Sonder Windows updater")
                    .timeout(Duration::from_secs(180))
                    .build()?;
                if let Some(release) = release {
                    stage_update(&client, &release).await?;
                    Ok(None)
                } else {
                    latest_release(&client).await
                }
            })
        })();
        let _ = sender.send_blocking(result);
    });
    cx.spawn(async move |cx| {
        if let Ok(result) = receiver.recv().await {
            cx.update(|cx| {
                let updater = cx.global_mut::<Updater>();
                updater.busy = false;
                let check = match result {
                    Ok(_) if installing => {
                        cx.quit();
                        return;
                    }
                    Ok(release) => {
                        let check = release.as_ref().map_or(UpdateCheck::UpToDate, |r| {
                            UpdateCheck::Available(r.version.clone())
                        });
                        updater.release = release;
                        check
                    }
                    Err(error) => {
                        log::warn!("update failed: {error:#}");
                        updater.release = None;
                        UpdateCheck::Failed(error.to_string())
                    }
                };
                let status = updater.status.clone();
                let fetch = updater.auto_install && matches!(check, UpdateCheck::Available(_));
                status.update(cx, |status, cx| {
                    status.check = check;
                    cx.notify();
                });
                if fetch {
                    download_in_background(cx);
                }
            });
        }
    })
    .detach();
}

async fn latest_release(client: &reqwest::Client) -> Result<Option<Release>> {
    let repository =
        option_env!("CADENCE_UPDATE_REPOSITORY").context("update feed is not configured")?;
    crate::update_release::latest_release(client, repository, env!("CARGO_PKG_VERSION")).await
}

async fn stage_update(client: &reqwest::Client, release: &Release) -> Result<()> {
    let staged = download_staged(client, release).await?;
    launch_helper(&staged, true).inspect_err(|_| {
        let _ = fs::remove_file(&staged);
    })
}

/// Streams the release next to the executable and checks its digest. Memory
/// stays at one chunk however large the download is.
async fn download_staged(client: &reqwest::Client, release: &Release) -> Result<PathBuf> {
    use futures::StreamExt as _;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .context("missing executable directory")?;
    remove_stale_downloads(directory);
    let staged = directory.join(format!("cadence-update-{}.exe", std::process::id()));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .context("cannot write next to Sonder; move it to a writable folder before updating")?;
        let mut response = client
            .get(&release.url)
            .send()
            .await?
            .error_for_status()?
            .bytes_stream();
        let mut digest = Sha256::new();
        let mut size = 0usize;
        while let Some(chunk) = response.next().await {
            let chunk = chunk?;
            size += chunk.len();
            ensure!(size <= 300 * 1024 * 1024, "update exceeds size limit");
            file.write_all(&chunk)?;
            digest.update(&chunk);
        }
        file.sync_all()?;
        ensure!(
            format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&release.checksum),
            "update checksum mismatch"
        );
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(staged),
        Err(error) => {
            let _ = fs::remove_file(staged);
            Err(error)
        }
    }
}

/// Downloads a crashed session left behind are a day old by the time anyone
/// looks; a live installer is always younger than that.
fn remove_stale_downloads(directory: &std::path::Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("cadence-update-")
            && (name.ends_with(".exe") || name.ends_with(".ps1")))
        {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Starts the installer, which waits for Sonder to exit, swaps the
/// executable, and (when `relaunch`) starts the new version.
fn launch_helper(staged: &std::path::Path, relaunch: bool) -> Result<()> {
    use std::{os::windows::process::CommandExt as _, process::Command};
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .context("missing executable directory")?;
    let helper = directory.join(format!("cadence-update-{}.ps1", std::process::id()));
    fs::write(
        &helper,
        include_str!("../../scripts/install-windows-update.ps1"),
    )?;
    let mut command = Command::new("powershell.exe");
    command
        .creation_flags(0x08000000)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&helper)
        .arg("-ProcessId")
        .arg(std::process::id().to_string())
        .arg("-Executable")
        .arg(&executable)
        .arg("-Staged")
        .arg(staged);
    if !relaunch {
        command.arg("-NoRelaunch");
    }
    command.spawn().inspect_err(|_| {
        let _ = fs::remove_file(&helper);
    })?;
    Ok(())
}
