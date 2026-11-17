//! Moves a file to the Recycle Bin (Windows) or the Trash (macOS), so a
//! deleted song can still be put back. Nothing is erased for good, and the
//! listener's own files never need a permission prompt.

use std::path::Path;

use anyhow::{Result, anyhow};

/// Sends `path` to the system's bin. Blocks on the file system, so call it
/// off the interface thread.
pub fn move_to_trash(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("that file is already gone"));
    }
    platform(path)
}

#[cfg(target_os = "windows")]
fn platform(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    // The shell wants a list of paths, each ending in a null, and one more
    // null after the last.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY: `operation` and the buffer `from` points into outlive the call,
    // and the buffer is double-null-terminated as the API requires.
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code == 0 && operation.fAnyOperationsAborted == 0 {
        Ok(())
    } else {
        Err(anyhow!(
            "Windows could not move it to the Recycle Bin (code {code})"
        ))
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn platform(path: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    use objc::runtime::{BOOL, Object, YES};
    use objc::{class, msg_send, sel, sel_impl};

    let text = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("the file name is not valid"))?;
    // SAFETY: plain Foundation calls on objects that live for this call. The
    // string is copied by `stringWithUTF8String:` before `text` drops.
    unsafe {
        let string: *mut Object = msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()];
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: string];
        let manager: *mut Object = msg_send![class!(NSFileManager), defaultManager];
        let mut error: *mut Object = std::ptr::null_mut();
        let trashed: BOOL = msg_send![manager,
            trashItemAtURL: url
            resultingItemURL: std::ptr::null_mut::<*mut Object>()
            error: &mut error];
        if trashed == YES {
            Ok(())
        } else {
            Err(anyhow!("the Trash would not take it"))
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform(_path: &Path) -> Result<()> {
    Err(anyhow!("deleting is not supported on this system"))
}

#[cfg(test)]
mod tests {
    use super::move_to_trash;

    /// Puts a real file in the real bin, so it is run by hand:
    /// `cargo test real_file -- --ignored`.
    #[test]
    #[ignore = "writes to the real Trash or Recycle Bin"]
    fn a_real_file_leaves_its_folder_for_the_bin() {
        let file = std::env::temp_dir().join(format!("sonder-trash-{}.mp3", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        move_to_trash(&file).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn a_missing_file_is_reported_not_ignored() {
        let missing = std::env::temp_dir().join("sonder-trash-test-missing.mp3");
        assert!(move_to_trash(&missing).is_err());
    }
}
