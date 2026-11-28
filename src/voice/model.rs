//! The speech model Voice play listens with: which ones there are, where they
//! live on disk, and how they are downloaded.
//!
//! The models are whisper.cpp's own, published by its author. They are large,
//! so they are never bundled: the listener starts the download in Settings and
//! can remove it again.

use anyhow::{Context as _, Result, ensure};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Model {
    /// Fast and good for clear speech.
    Standard,
    /// Better with unusual names, a little slower and larger.
    Accurate,
}

impl Model {
    pub(crate) const ALL: [Self; 2] = [Self::Standard, Self::Accurate];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Accurate => "accurate",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|model| model.key() == key)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Accurate => "Accurate",
        }
    }

    /// What it is good at, for Settings.
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Standard => "Quick, and good for clear speech.",
            Self::Accurate => "Better with unusual names. A little slower.",
        }
    }

    pub(crate) fn file_name(self) -> &'static str {
        match self {
            Self::Standard => "ggml-base.en-q5_1.bin",
            Self::Accurate => "ggml-small.en-q5_1.bin",
        }
    }

    fn url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name()
        )
    }

    /// The exact size of the file, which also tells a finished download from
    /// a cut-off one.
    pub(crate) fn bytes(self) -> u64 {
        match self {
            Self::Standard => 59_721_011,
            Self::Accurate => 190_098_681,
        }
    }

    fn sha256(self) -> &'static str {
        match self {
            Self::Standard => "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f",
            Self::Accurate => "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30",
        }
    }

    /// "60 MB".
    pub(crate) fn size_label(self) -> String {
        megabytes(self.bytes())
    }
}

pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes + 500_000) / 1_000_000)
}

fn directory() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("dev", "twoducks", "cadence")?
            .data_dir()
            .join("voice"),
    )
}

pub(crate) fn path(model: Model) -> Option<PathBuf> {
    Some(directory()?.join(model.file_name()))
}

/// Whether the whole model is on disk.
pub(crate) fn is_installed(model: Model) -> bool {
    path(model)
        .and_then(|path| fs::metadata(path).ok())
        .is_some_and(|meta| meta.len() == model.bytes())
}

/// Deletes the model, and any half-finished download of it.
pub(crate) fn remove(model: Model) -> Result<()> {
    let path = path(model).context("could not find where the model is kept")?;
    for file in [path.clone(), path.with_extension("part")] {
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not remove the model"),
        }
    }
    Ok(())
}

/// Downloads and checks the model. `progress` is told how many bytes have
/// arrived. Setting `cancel` stops it and leaves nothing behind.
pub(crate) async fn download(
    client: &reqwest::Client,
    model: Model,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    use futures::StreamExt as _;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;

    let target = path(model).context("could not find where to keep the model")?;
    fs::create_dir_all(target.parent().context("no folder for the model")?)
        .context("could not create the model folder")?;
    let partial = target.with_extension("part");
    let result = async {
        let mut file = fs::File::create(&partial).context("could not write the model")?;
        let mut stream = client
            .get(model.url())
            .send()
            .await
            .context("could not reach the model download")?
            .error_for_status()
            .context("the model download was refused")?
            .bytes_stream();
        let mut digest = Sha256::new();
        let mut received = 0u64;
        while let Some(chunk) = stream.next().await {
            ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
            let chunk = chunk.context("the download was interrupted")?;
            received += chunk.len() as u64;
            // A server that sends more than it said is not the model.
            ensure!(
                received <= model.bytes(),
                "the download was larger than expected"
            );
            file.write_all(&chunk)
                .context("could not write the model")?;
            digest.update(&chunk);
            progress(received);
        }
        file.sync_all()?;
        ensure!(received == model.bytes(), "the download was cut short");
        ensure!(
            format!("{:x}", digest.finalize()) == model.sha256(),
            "the downloaded model did not match its checksum"
        );
        fs::rename(&partial, &target).context("could not save the model")?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_has_a_distinct_key_file_and_checksum() {
        for model in Model::ALL {
            assert_eq!(Model::from_key(model.key()), Some(model));
            assert_eq!(model.sha256().len(), 64);
            assert!(model.url().ends_with(model.file_name()));
        }
        assert_ne!(Model::Standard.file_name(), Model::Accurate.file_name());
        assert_ne!(Model::Standard.sha256(), Model::Accurate.sha256());
        assert_eq!(Model::from_key("huge"), None);
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(Model::Standard.size_label(), "60 MB");
        assert_eq!(Model::Accurate.size_label(), "190 MB");
        assert_eq!(megabytes(0), "0 MB");
    }
}
