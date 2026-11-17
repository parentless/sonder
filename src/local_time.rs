//! The hour on the listener's own clock, for the Home greeting. The standard
//! library knows only UTC, so this asks the system.

/// 0 to 23, local time.
#[cfg(target_os = "windows")]
pub fn hour() -> u32 {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    // SAFETY: `GetLocalTime` only writes the struct it is given.
    unsafe {
        let mut now: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut now);
        u32::from(now.wHour)
    }
}

/// 0 to 23, local time.
#[cfg(not(target_os = "windows"))]
pub fn hour() -> u32 {
    // SAFETY: `time` accepts a null pointer, and `localtime_r` fills the
    // struct it is given without keeping a reference to it.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut local).is_null() {
            return 12;
        }
        u32::try_from(local.tm_hour).unwrap_or(12)
    }
}
