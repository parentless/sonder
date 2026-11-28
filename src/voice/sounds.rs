//! The short tones that tell the listener Voice play started, stopped, or
//! could not do it. They are made here rather than shipped as files: a few
//! hundred samples each, built once when first needed.

// The Windows keyboard hook and tones are the only users of some of this.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Sound {
    /// Listening has started: speak now.
    Start,
    /// The key was let go: working on it.
    Stop,
    /// Nothing was heard, or nothing was found.
    Fail,
}

const RATE: u32 = 22_050;

impl Sound {
    /// Pairs of frequency in hertz and length in milliseconds.
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Self::Start => &[(660., 70), (880., 90)],
            Self::Stop => &[(880., 60), (587., 90)],
            Self::Fail => &[(262., 120), (196., 220)],
        }
    }
}

/// A mono 16-bit WAV file of the notes in a row. Each note fades in and out
/// so it does not click.
pub(crate) fn wav(notes: &[(f32, u32)]) -> Vec<u8> {
    const AMPLITUDE: f32 = 0.22;
    let mut samples: Vec<i16> = Vec::new();
    for &(hertz, millis) in notes {
        let count = (RATE * millis / 1000) as usize;
        let fade = (count / 5).max(1);
        for index in 0..count {
            let edge = index.min(count - 1 - index).min(fade) as f32 / fade as f32;
            let phase = index as f32 / RATE as f32 * hertz * std::f32::consts::TAU;
            samples.push((phase.sin() * AMPLITUDE * edge * f32::from(i16::MAX)) as i16);
        }
    }
    let data_length = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Plays a tone without waiting for it. A new tone cuts off one still playing.
#[cfg(target_os = "windows")]
pub(crate) fn play(sound: Sound) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    // The system reads the bytes while they play, so they are kept for good.
    static TONES: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    let tones = TONES
        .get_or_init(|| [Sound::Start, Sound::Stop, Sound::Fail].map(|sound| wav(sound.notes())));
    let tone = &tones[sound as usize];
    // SAFETY: `tone` is a complete WAV file that is never freed or moved.
    unsafe {
        PlaySoundW(
            tone.as_ptr().cast(),
            std::ptr::null_mut(),
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn play(_sound: Sound) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_is_a_well_formed_wav_file() {
        for sound in [Sound::Start, Sound::Stop, Sound::Fail] {
            let bytes = wav(sound.notes());
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            let riff = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(riff + 8, bytes.len());
            let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
            assert_eq!(data + 44, bytes.len());
            let length_ms: u32 = sound.notes().iter().map(|(_, ms)| ms).sum();
            let expected = (RATE * length_ms / 1000) as usize * 2;
            assert!(data.abs_diff(expected) < 64, "{sound:?}");
        }
    }

    #[test]
    fn a_tone_starts_and_ends_silent_so_it_does_not_click() {
        let bytes = wav(Sound::Start.notes());
        let first = i16::from_le_bytes(bytes[44..46].try_into().unwrap());
        let last = i16::from_le_bytes(bytes[bytes.len() - 2..].try_into().unwrap());
        assert!(first.abs() < 200 && last.abs() < 200);
    }

    #[test]
    fn the_tones_are_quiet() {
        let bytes = wav(Sound::Fail.notes());
        let loudest = bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs())
            .max()
            .unwrap();
        assert!(loudest < 10_000);
    }
}
