//! The speech model Voice play listens with: which ones there are, where they
//! live on disk, and how they are downloaded.
//!
//! Three are whisper.cpp's own, published by its author, each a single file.
//! The fourth is NVIDIA's Parakeet v3: a few files, plus the ONNX Runtime
//! library it runs on. They are large, so they are never bundled: the listener
//! starts the download in Settings and can remove it again.

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
    /// The smallest and quickest. Names are where it slips.
    Fast,
    /// Good for clear speech.
    Standard,
    /// Better with unusual names, a little slower and larger.
    Accurate,
    /// NVIDIA's Parakeet v3: the most accurate, and the largest.
    Parakeet,
    /// NVIDIA's Parakeet Unified: English only, and tuned for it.
    ParakeetUnified,
}

/// One file of a model.
struct Part {
    /// Where it is kept, inside the model's folder (or beside the file, for a
    /// model that is a single file).
    name: &'static str,
    url: &'static str,
    /// What is downloaded.
    bytes: u64,
    sha256: &'static str,
    /// For a download that is an archive: the file taken out of it, with the
    /// size and checksum it must have.
    unpack: Option<Unpack>,
}

struct Unpack {
    entry: &'static str,
    bytes: u64,
}

impl Part {
    /// The size the file on disk must have once it is installed.
    fn installed_bytes(&self) -> u64 {
        self.unpack
            .as_ref()
            .map_or(self.bytes, |unpack| unpack.bytes)
    }
}

const FAST: [Part; 1] = [Part {
    name: "ggml-tiny.en-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q5_1.bin",
    bytes: 32_166_155,
    sha256: "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b",
    unpack: None,
}];
const STANDARD: [Part; 1] = [Part {
    name: "ggml-base.en-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin",
    bytes: 59_721_011,
    sha256: "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f",
    unpack: None,
}];
const ACCURATE: [Part; 1] = [Part {
    name: "ggml-small.en-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q5_1.bin",
    bytes: 190_098_681,
    sha256: "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30",
    unpack: None,
}];
/// Parakeet's folder. The ONNX Runtime library comes from Microsoft's own
/// Python package, a small zip with the DLL inside; it is only needed, and only
/// fetched, on Windows.
// The library's version must be the one the `ort` crate in Cargo.toml is built
// for (1.31): an older one refuses its settings.
#[cfg(target_os = "windows")]
const PARAKEET_FILES: [Part; 4] = [PARAKEET_ENCODER, PARAKEET_DECODER, PARAKEET_VOCAB, RUNTIME];
#[cfg(not(target_os = "windows"))]
const PARAKEET_FILES: [Part; 3] = [PARAKEET_ENCODER, PARAKEET_DECODER, PARAKEET_VOCAB];
const PARAKEET_ENCODER: Part = Part {
    name: "encoder-model.int8.onnx",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx",
    bytes: 652_183_999,
    sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    unpack: None,
};
const PARAKEET_DECODER: Part = Part {
    name: "decoder_joint-model.int8.onnx",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.int8.onnx",
    bytes: 18_202_004,
    sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
    unpack: None,
};
const PARAKEET_VOCAB: Part = Part {
    name: "vocab.txt",
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt",
    bytes: 93_939,
    sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    unpack: None,
};
#[cfg(target_os = "windows")]
const RUNTIME: Part = Part {
    name: RUNTIME_DLL,
    url: "https://files.pythonhosted.org/packages/be/ed/b12cea136ccd7b03d924f46b8393faf7ceac21115c0c50e729faa248cf23/onnxruntime-1.31.0-cp312-cp312-win_amd64.whl",
    bytes: 14_888_841,
    sha256: "09d56445c1753e66e0912de69d3f0184016ad9a191dcd6925bf5dd570d2bfbe5",
    unpack: Some(Unpack {
        entry: "onnxruntime/capi/onnxruntime.dll",
        bytes: 19_288_928,
    }),
};
/// Parakeet Unified's folder: the int8 files from a community ONNX export of
/// NVIDIA's model, which `parakeet-rs` reads by these names.
#[cfg(target_os = "windows")]
const UNIFIED_FILES: [Part; 5] = [
    UNIFIED_ENCODER,
    UNIFIED_ENCODER_DATA,
    UNIFIED_DECODER,
    UNIFIED_TOKENIZER,
    RUNTIME,
];
#[cfg(not(target_os = "windows"))]
const UNIFIED_FILES: [Part; 4] = [
    UNIFIED_ENCODER,
    UNIFIED_ENCODER_DATA,
    UNIFIED_DECODER,
    UNIFIED_TOKENIZER,
];
const UNIFIED_ENCODER: Part = Part {
    name: "encoder.int8.onnx",
    url: "https://huggingface.co/bobNight/parakeet-unified-en-0.6b-onnx/resolve/main/encoder.int8.onnx",
    bytes: 42_606_669,
    sha256: "c81adfab77634e00c1668a221a14f244c5fb3409e7c14eeebaf6ac963425910f",
    unpack: None,
};
/// The encoder's weights, which its `.onnx` file points to by this name.
const UNIFIED_ENCODER_DATA: Part = Part {
    name: "encoder.int8.onnx.data",
    url: "https://huggingface.co/bobNight/parakeet-unified-en-0.6b-onnx/resolve/main/encoder.int8.onnx.data",
    bytes: 611_491_584,
    sha256: "3d54dd04646c15677bd2844a84df3770b12cc1ce183481f7b6e0def31c92114a",
    unpack: None,
};
const UNIFIED_DECODER: Part = Part {
    name: "decoder_joint.int8.onnx",
    url: "https://huggingface.co/bobNight/parakeet-unified-en-0.6b-onnx/resolve/main/decoder_joint.int8.onnx",
    bytes: 8_995_064,
    sha256: "7f76ad5f35035f25630075699c6c942a2c0c05ff42cb398f966f3c256d148e1e",
    unpack: None,
};
const UNIFIED_TOKENIZER: Part = Part {
    name: "tokenizer.model",
    url: "https://huggingface.co/bobNight/parakeet-unified-en-0.6b-onnx/resolve/main/tokenizer.model",
    bytes: 251_056,
    sha256: "07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9",
    unpack: None,
};
/// The name the ONNX Runtime library is kept under.
pub(crate) const RUNTIME_DLL: &str = "onnxruntime.dll";

impl Model {
    pub(crate) const ALL: [Self; 5] = [
        Self::Fast,
        Self::Standard,
        Self::Accurate,
        Self::Parakeet,
        Self::ParakeetUnified,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Standard => "standard",
            Self::Accurate => "accurate",
            Self::Parakeet => "parakeet",
            Self::ParakeetUnified => "parakeet-unified",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|model| model.key() == key)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Standard => "Standard",
            Self::Accurate => "Accurate",
            Self::Parakeet => "Parakeet v3",
            Self::ParakeetUnified => "Parakeet Unified English",
        }
    }

    /// What it is good at, for Settings.
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Fast => "Smallest and quickest. Can slip on unusual names.",
            Self::Standard => "Quick, and good for clear speech.",
            Self::Accurate => "Better with unusual names. A little slower.",
            Self::Parakeet => {
                "NVIDIA's most accurate model, and fast. Needs about 1.3 GB of memory for the second it works."
            }
            Self::ParakeetUnified => {
                "NVIDIA's English-only Parakeet, tuned for English speech with punctuation. Needs about 1.3 GB of memory for the second it works."
            }
        }
    }

    fn parts(self) -> &'static [Part] {
        match self {
            Self::Fast => &FAST,
            Self::Standard => &STANDARD,
            Self::Accurate => &ACCURATE,
            Self::Parakeet => &PARAKEET_FILES,
            Self::ParakeetUnified => &UNIFIED_FILES,
        }
    }

    /// Whether it is several files in a folder, not one file.
    pub(crate) fn is_folder(self) -> bool {
        matches!(self, Self::Parakeet | Self::ParakeetUnified)
    }

    /// Everything that is downloaded, which tells a finished download from a
    /// cut-off one.
    pub(crate) fn bytes(self) -> u64 {
        self.parts().iter().map(|part| part.bytes).sum()
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

/// Where the model is kept: the file itself, or for Parakeet its folder.
pub(crate) fn path(model: Model) -> Option<PathBuf> {
    let directory = directory()?;
    Some(match model {
        Model::Parakeet => directory.join("parakeet-tdt-v3"),
        Model::ParakeetUnified => directory.join("parakeet-unified-en"),
        other => directory.join(other.parts()[0].name),
    })
}

/// Where each part of the model is kept.
fn part_path(model: Model, part: &Part) -> Option<PathBuf> {
    if model.is_folder() {
        Some(path(model)?.join(part.name))
    } else {
        path(model)
    }
}

fn part_installed(model: Model, part: &Part) -> bool {
    part_path(model, part)
        .and_then(|path| fs::metadata(path).ok())
        .is_some_and(|meta| meta.len() == part.installed_bytes())
}

/// Whether the whole model is on disk.
pub(crate) fn is_installed(model: Model) -> bool {
    model.parts().iter().all(|part| part_installed(model, part))
}

/// Deletes the model, and any half-finished download of it.
pub(crate) fn remove(model: Model) -> Result<()> {
    let path = path(model).context("could not find where the model is kept")?;
    let outcome = if model.is_folder() {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    };
    match outcome {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("could not remove the model"),
    }
    if !model.is_folder() {
        match fs::remove_file(path.with_extension("part")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not remove the model"),
        }
    }
    Ok(())
}

/// Downloads and checks the model. `progress` is told how many bytes have
/// arrived in all. Setting `cancel` stops it and leaves nothing behind that
/// would pass for a finished file; parts already finished are kept, so the
/// next try carries on from them.
pub(crate) async fn download(
    client: &reqwest::Client,
    model: Model,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let mut done = 0u64;
    for part in model.parts() {
        if part_installed(model, part) {
            done += part.bytes;
            progress(done);
            continue;
        }
        let target = part_path(model, part).context("could not find where to keep the model")?;
        download_part(client, part, &target, &cancel, |received| {
            progress(done + received)
        })
        .await?;
        done += part.bytes;
    }
    Ok(())
}

async fn download_part(
    client: &reqwest::Client,
    part: &Part,
    target: &std::path::Path,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    use futures::StreamExt as _;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;

    fs::create_dir_all(target.parent().context("no folder for the model")?)
        .context("could not create the model folder")?;
    let partial = target.with_extension("part");
    let result = async {
        let mut file = fs::File::create(&partial).context("could not write the model")?;
        let mut stream = client
            .get(part.url)
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
                received <= part.bytes,
                "the download was larger than expected"
            );
            file.write_all(&chunk)
                .context("could not write the model")?;
            digest.update(&chunk);
            progress(received);
        }
        file.sync_all()?;
        drop(file);
        ensure!(received == part.bytes, "the download was cut short");
        ensure!(
            format!("{:x}", digest.finalize()) == part.sha256,
            "the downloaded model did not match its checksum"
        );
        match &part.unpack {
            Some(unpack) => unpack_entry(&partial, unpack, target)?,
            None => fs::rename(&partial, target).context("could not save the model")?,
        }
        Ok(())
    }
    .await;
    // An archive is only a way to carry the file.
    let _ = fs::remove_file(&partial);
    result
}

/// Takes one file out of a downloaded archive.
fn unpack_entry(
    archive: &std::path::Path,
    unpack: &Unpack,
    target: &std::path::Path,
) -> Result<()> {
    let mut zip =
        zip::ZipArchive::new(fs::File::open(archive).context("could not open the download")?)
            .context("the download was not a readable archive")?;
    let mut entry = zip
        .by_name(unpack.entry)
        .context("the download did not hold what was expected")?;
    ensure!(
        entry.size() == unpack.bytes,
        "the downloaded library was not the expected size"
    );
    let unfinished = target.with_extension("unpacking");
    let mut out = fs::File::create(&unfinished).context("could not write the library")?;
    let copied = std::io::copy(&mut entry, &mut out).context("could not unpack the library")?;
    out.sync_all()?;
    drop(out);
    if copied != unpack.bytes {
        let _ = fs::remove_file(&unfinished);
        anyhow::bail!("the unpacked library was cut short");
    }
    fs::rename(&unfinished, target).context("could not save the library")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_has_a_distinct_key_and_checksummed_parts() {
        for model in Model::ALL {
            assert_eq!(Model::from_key(model.key()), Some(model));
            for part in model.parts() {
                assert_eq!(part.sha256.len(), 64, "{}", part.name);
                assert!(part.url.starts_with("https://"), "{}", part.name);
                assert!(part.bytes > 0);
            }
        }
        assert_ne!(
            Model::Standard.parts()[0].sha256,
            Model::Accurate.parts()[0].sha256
        );
        assert_eq!(Model::from_key("huge"), None);
    }

    #[test]
    fn a_single_file_model_is_a_file_and_parakeet_is_a_folder() {
        assert!(
            path(Model::Standard)
                .unwrap()
                .ends_with("ggml-base.en-q5_1.bin")
        );
        assert!(path(Model::Parakeet).unwrap().ends_with("parakeet-tdt-v3"));
        assert!(
            path(Model::Parakeet)
                .unwrap()
                .join(RUNTIME_DLL)
                .ends_with(RUNTIME_DLL)
        );
        assert!(
            Model::Parakeet
                .parts()
                .iter()
                .any(|part| part.name == "vocab.txt")
        );
        // The unified model's weights sit beside the file that names them.
        assert!(
            path(Model::ParakeetUnified)
                .unwrap()
                .ends_with("parakeet-unified-en")
        );
        assert!(
            Model::ParakeetUnified
                .parts()
                .iter()
                .any(|part| part.name == "encoder.int8.onnx.data")
        );
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(Model::Fast.size_label(), "32 MB");
        assert_eq!(Model::Standard.size_label(), "60 MB");
        assert_eq!(Model::Accurate.size_label(), "190 MB");
        assert_eq!(megabytes(0), "0 MB");
        // Windows also fetches the runtime library.
        let unified = Model::ParakeetUnified.bytes();
        assert!(unified > 660_000_000 && unified < 690_000_000, "{unified}");
        let parakeet = Model::Parakeet.bytes();
        assert!(
            parakeet > 670_000_000 && parakeet < 690_000_000,
            "{parakeet}"
        );
    }

    #[test]
    fn a_downloaded_archive_gives_up_only_the_file_asked_for() {
        use std::io::Write as _;
        let folder = std::env::temp_dir().join(format!("sonder-unpack-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let archive = folder.join("a.zip");
        {
            let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("pkg/lib.dll", options).unwrap();
            writer.write_all(b"library bytes").unwrap();
            writer.start_file("pkg/other.txt", options).unwrap();
            writer.write_all(b"other").unwrap();
            writer.finish().unwrap();
        }
        let target = folder.join("lib.dll");
        let unpack = Unpack {
            entry: "pkg/lib.dll",
            bytes: 13,
        };
        unpack_entry(&archive, &unpack, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"library bytes");
        let wrong = Unpack {
            entry: "pkg/lib.dll",
            bytes: 99,
        };
        assert!(unpack_entry(&archive, &wrong, &folder.join("x.dll")).is_err());
        let missing = Unpack {
            entry: "pkg/none.dll",
            bytes: 1,
        };
        assert!(unpack_entry(&archive, &missing, &folder.join("y.dll")).is_err());
        let _ = fs::remove_dir_all(&folder);
    }
}
