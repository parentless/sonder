//! Listens for the Voice play shortcut everywhere on this computer.
//!
//! Windows lets an app watch the keyboard with a low-level hook. The hook runs
//! on the thread that installed it, so that thread does nothing else but pump
//! messages. The hook itself only compares a key code and sends a message; all
//! real work happens in the app.
//!
//! The hook cannot see keys typed into a window that runs as administrator
//! unless Sonder does too. That is a Windows rule, not something to work
//! around.

use super::hotkey::{Hotkey, HotkeyEvent};
use anyhow::Result;

/// Whether this platform can listen for the shortcut.
pub(crate) const SUPPORTED: bool = cfg!(target_os = "windows");

#[cfg(target_os = "windows")]
pub(crate) use windows_impl::Listener;

#[cfg(not(target_os = "windows"))]
pub(crate) use other_impl::Listener;

#[cfg(not(target_os = "windows"))]
mod other_impl {
    use super::*;

    /// Without a keyboard hook there is nothing to listen to.
    pub(crate) struct Listener;

    impl Listener {
        pub(crate) fn start(
            _hotkey: &Hotkey,
            _events: async_channel::Sender<HotkeyEvent>,
        ) -> Result<Self> {
            anyhow::bail!("Voice play needs Windows")
        }

        pub(crate) fn set_hotkey(&self, _hotkey: &Hotkey) {}
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use crate::voice::hotkey::{ALT, CTRL, SHIFT, Tracker, WIN};
    use anyhow::{Context as _, bail};
    use std::{
        sync::Mutex,
        thread::{self, JoinHandle},
    };
    use windows_sys::Win32::{
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::GetAsyncKeyState,
            WindowsAndMessaging::{
                CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW,
                SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
                WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
            },
        },
    };

    struct State {
        tracker: Option<Tracker>,
        events: async_channel::Sender<HotkeyEvent>,
    }

    /// The hook is a plain function, so what it needs lives here. Only one
    /// listener exists at a time.
    static STATE: Mutex<Option<State>> = Mutex::new(None);

    pub(crate) struct Listener {
        thread_id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl Listener {
        pub(crate) fn start(
            hotkey: &Hotkey,
            events: async_channel::Sender<HotkeyEvent>,
        ) -> Result<Self> {
            {
                let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
                if state.is_some() {
                    bail!("the Voice play shortcut is already being listened for");
                }
                *state = Some(State {
                    tracker: Tracker::new(hotkey),
                    events,
                });
            }
            let (ready, started) = std::sync::mpsc::channel();
            let thread = thread::Builder::new()
                .name("sonder-voice-hotkey".into())
                .spawn(move || {
                    // SAFETY: the hook procedure is a valid `extern "system"`
                    // function for the life of the process, and the hook is
                    // removed on this same thread before it ends.
                    unsafe {
                        let thread_id = GetCurrentThreadId();
                        let hook = SetWindowsHookExW(
                            WH_KEYBOARD_LL,
                            Some(hook_proc),
                            GetModuleHandleW(std::ptr::null()),
                            0,
                        );
                        if hook.is_null() {
                            let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
                            return;
                        }
                        let _ = ready.send(Ok(thread_id));
                        let mut message: MSG = std::mem::zeroed();
                        // Returns 0 for the quit message, -1 for an error.
                        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {}
                        UnhookWindowsHookEx(hook);
                    }
                })
                .context("could not start the shortcut listener")?;
            match started.recv() {
                Ok(Ok(thread_id)) => Ok(Self {
                    thread_id,
                    thread: Some(thread),
                }),
                Ok(Err(error)) => {
                    clear_state();
                    bail!("Windows would not let Sonder listen for the shortcut: {error}")
                }
                Err(_) => {
                    clear_state();
                    bail!("the shortcut listener stopped before it started")
                }
            }
        }

        pub(crate) fn set_hotkey(&self, hotkey: &Hotkey) {
            if let Some(state) = STATE
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
            {
                state.tracker = Tracker::new(hotkey);
            }
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // SAFETY: posts a quit message to the thread this listener owns.
            unsafe {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            clear_state();
        }
    }

    fn clear_state() {
        *STATE.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }

    /// Which of Ctrl, Alt, Shift and Windows are down right now.
    fn held_modifiers() -> u8 {
        let down = |vk: i32| {
            // SAFETY: reads the keyboard state; no memory is involved.
            (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
        };
        let mut held = 0;
        if down(0x11) {
            held |= CTRL;
        }
        if down(0x12) {
            held |= ALT;
        }
        if down(0x10) {
            held |= SHIFT;
        }
        if down(0x5B) || down(0x5C) {
            held |= WIN;
        }
        held
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            let down = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
                WM_KEYUP | WM_SYSKEYUP => Some(false),
                _ => None,
            };
            if let Some(down) = down {
                // SAFETY: for a keyboard hook, `lparam` points at a
                // `KBDLLHOOKSTRUCT` that lives for the call.
                let vk = unsafe { (*(lparam as *const KBDLLHOOKSTRUCT)).vkCode };
                // Never wait on a lock here: a slow hook slows every key press.
                if let Ok(mut guard) = STATE.try_lock()
                    && let Some(state) = guard.as_mut()
                    && let Some(tracker) = state.tracker.as_mut()
                {
                    let outcome = tracker.key(vk, down, held_modifiers());
                    if let Some(event) = outcome.event {
                        let _ = state.events.try_send(event);
                    }
                    if outcome.swallow {
                        return 1;
                    }
                }
            }
        }
        // SAFETY: passes the event on unchanged.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }
}
