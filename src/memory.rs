//! Gives idle memory back to the system.

/// Asks Windows to move this process's pages out of the working set. They are
/// paged back in on demand, so it is only worth doing while nothing is on screen.
pub(crate) fn trim_working_set() {
    #[cfg(target_os = "windows")]
    // SAFETY: both calls take the current-process pseudo handle and no pointers.
    unsafe {
        use windows_sys::Win32::System::{
            ProcessStatus::K32EmptyWorkingSet, Threading::GetCurrentProcess,
        };
        K32EmptyWorkingSet(GetCurrentProcess());
    }
}
