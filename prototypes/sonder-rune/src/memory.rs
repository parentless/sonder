//! Memory diagnostics for Windows, where the numbers that matter live.
//!
//! `SONDER_MEMLOG=1` writes working set and private bytes to `sonder-rune-memory.txt`
//! in the temp folder every few seconds. `SONDER_TRIM=1` also trims the working set
//! (as Sonder does when hidden) once the first screen has settled.

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn start() {
    use std::io::Write;
    use std::time::Duration;

    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        fn K32EmptyWorkingSet(process: isize) -> i32;
    }

    let log = std::env::var_os("SONDER_MEMLOG").is_some();
    let trim = std::env::var_os("SONDER_TRIM").is_some();
    if !log && !trim {
        return;
    }
    std::thread::spawn(move || {
        let path = std::env::temp_dir().join("sonder-rune-memory.txt");
        let mut file = log.then(|| std::fs::File::create(&path).ok()).flatten();
        let started = std::time::Instant::now();
        let mut trimmed = false;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            if trim && !trimmed && started.elapsed() > Duration::from_secs(6) {
                trimmed = true;
                unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
            }
            let mut c = Counters::default();
            c.cb = size_of::<Counters>() as u32;
            let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
            if let (Some(file), 1) = (file.as_mut(), ok) {
                let _ = writeln!(
                    file,
                    "{:>4}s working set {:>6.1} MB  private {:>6.1} MB  peak working set {:>6.1} MB",
                    started.elapsed().as_secs(),
                    c.working_set as f64 / 1048576.0,
                    c.private as f64 / 1048576.0,
                    c.peak_working_set as f64 / 1048576.0,
                );
                let _ = file.flush();
            }
        }
    });
}

#[cfg(not(windows))]
pub fn start() {}

/// `SONDER_GPU=performance|usage|small` picks wgpu's memory policy, to compare footprints.
pub fn gpu_policy() {
    use rune::GpuMemoryPolicy;
    let policy = match std::env::var("SONDER_GPU").as_deref() {
        Ok("performance") => GpuMemoryPolicy::Performance,
        Ok("small") => GpuMemoryPolicy::Manual {
            block_size: (4 << 20)..(4 << 20),
        },
        _ => return,
    };
    rune::set_gpu_memory_policy(policy);
}
