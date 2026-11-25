//! A small, bounded on-disk cache of downloaded cover bytes, as Sonder keeps.
//!
//! Bounded to `CAP` bytes with the oldest files dropped first, never touched on
//! the UI thread, and best effort: any I/O trouble falls back to the network.
//! Covers kept here need not stay in memory, so a repaint reads them back.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

const CAP: u64 = 48 << 20;

struct Disk {
    dir: PathBuf,
    total: AtomicU64,
}

fn disk() -> Option<&'static Disk> {
    static DISK: OnceLock<Option<Disk>> = OnceLock::new();
    DISK.get_or_init(|| {
        let base = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        let dir = base.join("sonder-rune").join("artwork");
        fs::create_dir_all(&dir).ok()?;
        let mut total = 0;
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            match entry.metadata() {
                Ok(m) if entry.path().extension().is_some_and(|e| e == "tmp") => {
                    let _ = (m, fs::remove_file(entry.path()));
                }
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
        Some(Disk {
            dir,
            total: AtomicU64::new(total),
        })
    })
    .as_ref()
}

/// FNV-1a: stable across runs, unlike the standard hasher.
fn name(url: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(&[0x89, b'P', b'N', b'G'])
        || bytes.starts_with(b"RIFF")
        || bytes.starts_with(b"GIF8")
}

pub fn load(url: &str) -> Option<Vec<u8>> {
    let d = disk()?;
    let path = d.dir.join(name(url));
    let bytes = fs::read(&path).ok()?;
    if looks_like_image(&bytes) {
        Some(bytes)
    } else {
        let _ = fs::remove_file(path);
        d.total.fetch_sub(bytes.len() as u64, Ordering::Relaxed);
        None
    }
}

pub fn store(url: &str, bytes: &[u8]) {
    let Some(d) = disk() else { return };
    // One file should never flush the whole cache.
    if !looks_like_image(bytes) || bytes.len() as u64 > CAP / 8 {
        return;
    }
    let path = d.dir.join(name(url));
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let replaced = fs::metadata(&path).map_or(0, |m| m.len());
    if fs::write(&temp, bytes).is_err() || fs::rename(&temp, &path).is_err() {
        let _ = fs::remove_file(&temp);
        return;
    }
    let size = bytes.len() as u64;
    let total = d.total.load(Ordering::Relaxed).saturating_sub(replaced) + size;
    d.total.store(total, Ordering::Relaxed);
    if total > CAP {
        evict(d);
    }
}

/// Removes the oldest files until the cache is at three quarters of its cap.
fn evict(d: &Disk) {
    let mut files: Vec<_> = fs::read_dir(&d.dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort_by_key(|(time, ..)| *time);
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    for (_, len, path) in files {
        if total <= CAP / 4 * 3 {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
    d.total.store(total, Ordering::Relaxed);
}
