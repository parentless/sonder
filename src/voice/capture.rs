//! Records the microphone while the shortcut is held.
//!
//! The microphone is opened when the key goes down and closed when it comes
//! up, so it is only ever on while the listener is speaking. What was heard is
//! kept in memory, never written to disk, and is dropped once it has been
//! turned into text.

use anyhow::{Context as _, Result, bail};
use cpal::{
    FromSample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Speech models read audio at this rate.
pub(crate) const MODEL_RATE: u32 = 16_000;
/// A request is a few words. Anything longer is cut off.
pub(crate) const MAX_SECONDS: u64 = 15;
/// Less than this is an accidental tap, not a request.
pub(crate) const MIN_SECONDS: f32 = 0.45;
/// The start tone is still sounding when recording begins; this much of the
/// beginning is dropped so the tone is not heard as speech.
pub(crate) const TONE_MS: u32 = 170;
/// Quieter than this on average is silence, not speech.
const SILENCE_RMS: f32 = 0.004;

/// A recording in progress, on its own thread.
pub(crate) struct Recording {
    /// How loud the listener is right now, 0 to 1, for the listening
    /// animation. An `f32` kept as bits so the audio thread never waits.
    level: Arc<AtomicU32>,
    stop: std::sync::mpsc::Sender<()>,
    result: async_channel::Receiver<Result<Vec<f32>, String>>,
}

impl Recording {
    pub(crate) fn start() -> Self {
        let (stop, stopped) = std::sync::mpsc::channel();
        let (send, result) = async_channel::bounded(1);
        let level = Arc::new(AtomicU32::new(0));
        let spawned = thread::Builder::new()
            .name("sonder-voice-capture".into())
            .spawn({
                let send = send.clone();
                let level = Arc::clone(&level);
                move || {
                    let outcome = record(&stopped, &level).map_err(|error| format!("{error:#}"));
                    let _ = send.send_blocking(outcome);
                }
            });
        if let Err(error) = spawned {
            let _ = send.try_send(Err(format!("could not start recording: {error}")));
        }
        Self {
            level,
            stop,
            result,
        }
    }

    /// A handle to the live volume, which stays readable after `finish`.
    pub(crate) fn level(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.level)
    }

    /// Stops recording. The answer arrives on the returned channel: the audio
    /// at the model's rate, or what went wrong.
    pub(crate) fn finish(self) -> async_channel::Receiver<Result<Vec<f32>, String>> {
        // The thread may already have ended at the time limit.
        let _ = self.stop.send(());
        self.result
    }
}

fn record(stopped: &std::sync::mpsc::Receiver<()>, level: &Arc<AtomicU32>) -> Result<Vec<f32>> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no microphone was found")?;
    let supported = device.default_input_config().context(
        "could not read the microphone's settings; check Windows' microphone privacy settings",
    )?;
    let rate = supported.sample_rate();
    let channels = usize::from(supported.channels()).max(1);
    let format = supported.sample_format();
    let config = supported.config();

    let limit = rate as usize * MAX_SECONDS as usize;
    let heard = Arc::new(Mutex::new(Vec::<f32>::with_capacity(
        limit.min(rate as usize * 6),
    )));
    let stream = match format {
        SampleFormat::F32 => input_stream::<f32>(&device, config, channels, limit, &heard, level),
        SampleFormat::I16 => input_stream::<i16>(&device, config, channels, limit, &heard, level),
        SampleFormat::U16 => input_stream::<u16>(&device, config, channels, limit, &heard, level),
        SampleFormat::I32 => input_stream::<i32>(&device, config, channels, limit, &heard, level),
        other => bail!("the microphone uses a sound format Sonder cannot read ({other:?})"),
    }
    .context("could not open the microphone; check Windows' microphone privacy settings")?;
    stream.play().context("could not start the microphone")?;

    let started = Instant::now();
    loop {
        match stopped.recv_timeout(Duration::from_millis(10)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if started.elapsed() >= Duration::from_secs(MAX_SECONDS) {
                    break;
                }
            }
        }
    }
    // Closing the stream turns the microphone off before anything is processed.
    drop(stream);
    let samples = std::mem::take(&mut *heard.lock().unwrap_or_else(|error| error.into_inner()));
    Ok(prepare(&samples, rate))
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    limit: usize,
    heard: &Arc<Mutex<Vec<f32>>>,
    level: &Arc<AtomicU32>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let heard = Arc::clone(heard);
    let level = Arc::clone(level);
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut heard = heard.lock().unwrap_or_else(|error| error.into_inner());
            let mut energy = 0f32;
            let mut frames = 0u32;
            for frame in data.chunks(channels) {
                if heard.len() >= limit {
                    return;
                }
                let sum: f32 = frame
                    .iter()
                    .map(|&sample| <f32 as FromSample<T>>::from_sample_(sample))
                    .sum();
                let mono = sum / frame.len() as f32;
                energy += mono * mono;
                frames += 1;
                heard.push(mono);
            }
            if frames > 0 {
                let now = level_of((energy / frames as f32).sqrt());
                let before = f32::from_bits(level.load(Ordering::Relaxed));
                // Rises at once, falls gently, so the animation does not flicker.
                level.store(now.max(before * 0.85).to_bits(), Ordering::Relaxed);
            }
        },
        |error| log::warn!("microphone stream error: {error}"),
        None,
    )
}

/// Speech at a normal distance is around 0.02 to 0.2 RMS; this maps that to 0..1.
fn level_of(rms: f32) -> f32 {
    (rms * 7.).sqrt().min(1.)
}

/// Drops the start tone and converts to what the model reads.
pub(crate) fn prepare(mono: &[f32], rate: u32) -> Vec<f32> {
    let skip = (rate as usize * TONE_MS as usize / 1000).min(mono.len());
    resample(&mono[skip..], rate, MODEL_RATE)
}

/// Averages each stretch of input into one output sample. That is a crude
/// low-pass filter, which is plenty for speech.
pub(crate) fn resample(mono: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || mono.is_empty() {
        return mono.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let length = (mono.len() as f64 / ratio) as usize;
    (0..length)
        .map(|index| {
            let start = ((index as f64 * ratio) as usize).min(mono.len() - 1);
            let end = (((index + 1) as f64 * ratio) as usize).clamp(start + 1, mono.len());
            let window = &mono[start..end];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect()
}

/// How the audio looks before it is sent to the model.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Heard {
    /// A tap, or nothing at all.
    TooShort,
    /// Long enough but too quiet to be speech.
    Silent,
    Speech,
}

pub(crate) fn assess(samples: &[f32]) -> Heard {
    if (samples.len() as f32) < MODEL_RATE as f32 * MIN_SECONDS {
        return Heard::TooShort;
    }
    let energy: f32 = samples.iter().map(|sample| sample * sample).sum();
    if (energy / samples.len() as f32).sqrt() < SILENCE_RMS {
        Heard::Silent
    } else {
        Heard::Speech
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f32, amplitude: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|index| {
                (index as f32 / rate as f32 * 440. * std::f32::consts::TAU).sin() * amplitude
            })
            .collect()
    }

    #[test]
    fn resampling_changes_the_length_by_the_rate_ratio() {
        for rate in [48_000, 44_100, 32_000, 16_000, 8_000] {
            let out = resample(&tone(rate, 1., 0.5), rate, MODEL_RATE);
            assert!(out.len().abs_diff(16_000) <= 2, "{rate}: {}", out.len());
        }
        assert!(resample(&[], 48_000, MODEL_RATE).is_empty());
    }

    #[test]
    fn resampling_keeps_a_voice_range_tone_at_its_level() {
        let out = resample(&tone(48_000, 1., 0.5), 48_000, MODEL_RATE);
        let peak = out.iter().fold(0f32, |top, sample| top.max(sample.abs()));
        assert!((0.4..=0.55).contains(&peak), "peak {peak}");
    }

    #[test]
    fn the_start_tone_is_cut_from_the_front() {
        let mut audio = vec![1.0; 48_000 * 170 / 1000];
        audio.extend(vec![0.25; 48_000]);
        let out = prepare(&audio, 48_000);
        assert!(out.iter().all(|sample| (sample - 0.25).abs() < 0.01));
        assert!(prepare(&[0.5; 100], 48_000).is_empty());
    }

    #[test]
    fn volume_maps_quiet_to_nothing_and_speech_to_most_of_the_range() {
        assert_eq!(level_of(0.), 0.);
        assert!(level_of(0.003) < 0.2);
        assert!((0.4..=1.).contains(&level_of(0.05)));
        assert_eq!(level_of(1.), 1.);
    }

    #[test]
    fn a_tap_silence_and_speech_are_told_apart() {
        assert_eq!(assess(&[]), Heard::TooShort);
        assert_eq!(assess(&tone(16_000, 0.2, 0.5)), Heard::TooShort);
        assert_eq!(assess(&vec![0.0005; 16_000]), Heard::Silent);
        assert_eq!(assess(&tone(16_000, 1., 0.3)), Heard::Speech);
    }
}
