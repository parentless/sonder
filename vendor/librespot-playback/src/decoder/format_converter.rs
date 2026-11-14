//! Cadence patch: brings decoded local files to the player's one format,
//! 44.1 kHz interleaved stereo. Spotify streams already match and pass
//! straight through without touching the resampler.

use rubato::{
    Fft, FixedSync, Indexing, Resampler as _,
    audioadapter_buffers::direct::InterleavedSlice,
};

use super::{DecoderError, DecoderResult};
use crate::{NUM_CHANNELS, SAMPLE_RATE};

const STEREO: usize = NUM_CHANNELS as usize;
/// Input frames per resampler pass: about 23 ms at 44.1 kHz.
const CHUNK_FRAMES: usize = 1024;

pub(crate) struct FormatConverter {
    input_rate: u32,
    channels: usize,
    resampler: Option<Resampling>,
}

struct Resampling {
    resampler: Box<Fft<f64>>,
    /// Stereo input frames waiting for a full resampler chunk.
    pending: Vec<f64>,
    chunk_output: Vec<f64>,
    /// Output frames still to drop: the resampler's start-up delay.
    skip_frames: usize,
    input_rate: u64,
    /// Frames taken in and handed out since the start or the last seek, so
    /// the output ends exactly as long in time as the file.
    frames_in: u64,
    frames_out: u64,
}

impl FormatConverter {
    pub(crate) fn new(input_rate: u32, channels: usize) -> DecoderResult<Self> {
        let resampler = if input_rate == SAMPLE_RATE {
            None
        } else {
            let resampler = Fft::<f64>::new(
                input_rate as usize,
                SAMPLE_RATE as usize,
                CHUNK_FRAMES,
                STEREO,
                FixedSync::Input,
            )
            .map_err(|error| {
                DecoderError::SymphoniaDecoder(format!(
                    "Unsupported sample rate {input_rate}: {error}"
                ))
            })?;
            let skip_frames = resampler.output_delay();
            let chunk_output = vec![0.; resampler.output_frames_max() * STEREO];
            Some(Resampling {
                resampler: Box::new(resampler),
                pending: Vec::with_capacity(CHUNK_FRAMES * STEREO),
                chunk_output,
                skip_frames,
                input_rate: u64::from(input_rate),
                frames_in: 0,
                frames_out: 0,
            })
        };
        Ok(Self {
            input_rate,
            channels,
            resampler,
        })
    }

    /// Whether this converter was built for audio of this format.
    pub(crate) fn accepts(&self, input_rate: u32, channels: usize) -> bool {
        self.input_rate == input_rate && self.channels == channels
    }

    /// Converts one decoded packet. A resampled packet can come back empty
    /// while the resampler waits for a full chunk; the player skips those.
    pub(crate) fn convert(&mut self, samples: &[f64]) -> DecoderResult<Vec<f64>> {
        let channels = self.channels;
        let Some(resampling) = self.resampler.as_mut() else {
            return Ok(to_stereo(samples, channels));
        };
        resampling.frames_in += (samples.len() / channels) as u64;
        if channels == STEREO {
            resampling.pending.extend_from_slice(samples);
        } else {
            resampling.pending.extend(to_stereo(samples, channels));
        }
        resampling.drain(false)
    }

    /// Flushes what the resampler still holds at the end of the file.
    pub(crate) fn finish(&mut self) -> DecoderResult<Vec<f64>> {
        match self.resampler.as_mut() {
            Some(resampling) => resampling.drain(true),
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn reset(&mut self) {
        if let Some(resampling) = self.resampler.as_mut() {
            resampling.resampler.reset();
            resampling.pending.clear();
            resampling.skip_frames = resampling.resampler.output_delay();
            resampling.frames_in = 0;
            resampling.frames_out = 0;
        }
    }
}

impl Resampling {
    fn drain(&mut self, flush: bool) -> DecoderResult<Vec<f64>> {
        let error = |error: &dyn std::fmt::Display| {
            DecoderError::SymphoniaDecoder(format!("could not resample local file: {error}"))
        };
        // The output never runs past the input's length in time.
        let frames_due = self.frames_in * u64::from(SAMPLE_RATE) / self.input_rate;
        let mut converted = Vec::new();
        while self.frames_out < frames_due {
            let frames_next = self.resampler.input_frames_next();
            let pending_frames = self.pending.len() / STEREO;
            let partial_len = if pending_frames >= frames_next {
                None
            } else if flush {
                // The final short chunk, then silence until the delayed tail
                // has come out; the resampler pads both.
                self.pending.resize(frames_next * STEREO, 0.);
                Some(pending_frames)
            } else {
                break;
            };
            let input = InterleavedSlice::new(
                &self.pending[..frames_next * STEREO],
                STEREO,
                frames_next,
            )
            .map_err(|e| error(&e))?;
            let frames_out = self.resampler.output_frames_max();
            let mut output = InterleavedSlice::new_mut(&mut self.chunk_output, STEREO, frames_out)
                .map_err(|e| error(&e))?;
            let indexing = partial_len.map(|partial_len| Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: Some(partial_len),
            });
            let (_, produced) = self
                .resampler
                .process_into_buffer(&input, &mut output, indexing.as_ref())
                .map_err(|e| error(&e))?;
            let skip = self.skip_frames.min(produced);
            self.skip_frames -= skip;
            let wanted = (frames_due - self.frames_out).min((produced - skip) as u64) as usize;
            converted.extend_from_slice(&self.chunk_output[skip * STEREO..(skip + wanted) * STEREO]);
            self.frames_out += wanted as u64;
            if partial_len.is_some() {
                self.pending.clear();
            } else {
                self.pending.drain(..frames_next * STEREO);
            }
        }
        Ok(converted)
    }
}

/// Interleaved samples with any channel count, as interleaved stereo. Mono
/// plays on both sides; surround keeps its front left and right.
fn to_stereo(samples: &[f64], channels: usize) -> Vec<f64> {
    match channels {
        STEREO => samples.to_vec(),
        1 => samples.iter().flat_map(|&sample| [sample, sample]).collect(),
        _ => samples
            .chunks_exact(channels)
            .flat_map(|frame| [frame[0], frame[1]])
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{FormatConverter, to_stereo};

    #[test]
    fn spotify_format_passes_through() {
        let mut converter = FormatConverter::new(44_100, 2).unwrap();
        let samples = vec![0.25, -0.25, 0.5, -0.5];
        assert_eq!(converter.convert(&samples).unwrap(), samples);
        assert!(converter.finish().unwrap().is_empty());
    }

    #[test]
    fn mono_and_surround_become_stereo() {
        assert_eq!(to_stereo(&[0.1, 0.2], 1), vec![0.1, 0.1, 0.2, 0.2]);
        assert_eq!(
            to_stereo(&[0.1, 0.2, 0.9, 0.3, 0.4, 0.9], 3),
            vec![0.1, 0.2, 0.3, 0.4]
        );
    }

    #[test]
    fn other_rates_resample_to_the_same_length_in_time() {
        let mut converter = FormatConverter::new(48_000, 1).unwrap();
        let second = vec![0.1; 48_000];
        let mut output = converter.convert(&second).unwrap();
        output.extend(converter.finish().unwrap());
        let frames = output.len() / 2;
        // One second in, exactly one second out.
        assert_eq!(frames, 44_100);
    }
}
