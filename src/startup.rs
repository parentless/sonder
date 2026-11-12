//! Starting Sonder when the listener signs in to Windows.
//!
//! One value under the per-user `Run` key, so no administrator rights are
//! needed and the listener can also manage it in Windows' Startup apps.
use std::{io, os::windows::ffi::OsStrExt as _};
use windows_sys::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
            RegOpenKeyExW, RegSetValueExW,
        },
    },
    core::{PCWSTR, w},
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("Sonder");
/// Added to the command so a login launch can start quietly in the tray.
const STARTUP_ARGUMENT: &str = "--startup";

/// Whether this process was started by the login entry rather than by hand.
pub fn launched_at_startup() -> bool {
    std::env::args().any(|argument| argument == STARTUP_ARGUMENT)
}

/// Adds or removes the login entry for the running executable.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let command = if enabled {
        let executable = std::env::current_exe()?;
        let mut command: Vec<u16> = Vec::new();
        command.push(u16::from(b'"'));
        command.extend(executable.as_os_str().encode_wide());
        command.push(u16::from(b'"'));
        command.push(u16::from(b' '));
        command.extend(STARTUP_ARGUMENT.encode_utf16());
        command.push(0);
        Some(command)
    } else {
        None
    };
    // SAFETY: documented registry calls with a key we open and close here and
    // NUL-terminated constants; the value buffer outlives the call.
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key);
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let status = match &command {
            Some(command) => RegSetValueExW(
                key,
                VALUE_NAME,
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * size_of::<u16>()) as u32,
            ),
            None => match RegDeleteValueW(key, VALUE_NAME) {
                // Already absent is the state asked for.
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                other => other,
            },
        };
        RegCloseKey(key);
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }
}
