//! Running without a window: the notification-area icon, closing to it, and
//! starting quietly at login. Closing the window releases the GPU and the covers,
//! so what stays is the backend and a menu, as in the GPUI app.

use std::sync::{Mutex, OnceLock};

use rune::{AppHandle, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec};

use crate::State;

static HANDLE: OnceLock<Mutex<AppHandle<State>>> = OnceLock::new();
/// Menu picks the UI thread must act on (`toggle`, `next`, `previous`).
static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_handle(handle: AppHandle<State>) {
    let _ = HANDLE.set(Mutex::new(handle));
}

fn with_handle(f: impl FnOnce(&AppHandle<State>)) {
    if let Some(handle) = HANDLE.get().and_then(|h| h.lock().ok()) {
        f(&handle);
    }
}

/// What the tray menu asked for since the last call.
pub fn take_commands() -> Vec<String> {
    QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn icon() -> Option<TrayIcon> {
    let image = image::load_from_memory(include_bytes!("../../../assets/sonder-mark.png"))
        .ok()?
        .resize(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    Some(TrayIcon::rgba(
        image.width(),
        image.height(),
        image.into_raw(),
    ))
}

/// Adds the tray icon. `None` when the platform has no tray or it could not be made.
pub fn start_tray() -> Option<Tray> {
    let spec = TraySpec::new("sonder", "Sonder", icon()?)
        .tooltip("Sonder")
        .menu(vec![
            TrayItem::action("show", "Show Sonder"),
            TrayItem::action("toggle", "Play / Pause"),
            TrayItem::action("next", "Next"),
            TrayItem::action("previous", "Previous"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit"),
        ]);
    Tray::new(spec, |event| match event {
        TrayEvent::Activate { .. } => with_handle(|h| h.open()),
        TrayEvent::Menu(id) => match id.as_str() {
            "show" => with_handle(|h| h.open()),
            "quit" => with_handle(|h| h.quit()),
            other => {
                if let Ok(mut q) = QUEUE.lock() {
                    q.push(other.to_owned());
                }
                with_handle(|h| h.wake());
            }
        },
        _ => {}
    })
    .ok()
}

/// Whether this launch came from the login entry.
pub fn launched_at_startup() -> bool {
    std::env::args().any(|a| a == "--startup")
}

/// Rune's own flags, without Sonder's `--startup`.
pub fn rune_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| a != "--startup")
        .collect()
}

/// Adds or removes the Windows login entry.
pub fn set_launch_at_login(enabled: bool) {
    #[cfg(target_os = "windows")]
    let _ = cadence::startup::set_enabled(enabled);
    #[cfg(not(target_os = "windows"))]
    let _ = enabled;
}
