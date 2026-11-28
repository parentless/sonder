//! Turns recorded speech into text with whisper.cpp, on this computer.
//!
//! This runs only inside the short-lived helper process (see `worker.rs`), so
//! the speech model exists in memory only while the shortcut is held and its
//! words are being worked out.

use anyhow::{Context as _, Result, bail};
use std::{path::Path, thread};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// The model reads at least this much audio, so very short clips are padded.
const MIN_SAMPLES: usize = 16_000;
/// The most prompt text sent along. Names the listener plays are given to the
/// model so it spells them as Spotify does.
const MAX_PROMPT_CHARS: usize = 400;
/// The model's own limit on how much audio it looks at: 30 seconds.
const MAX_AUDIO_CTX: i32 = 1500;
/// The fewest audio positions the model is given, however short the clip.
const MIN_AUDIO_CTX: i32 = 384;

/// What the model is told before it listens.
pub(crate) fn prompt(names: &[String]) -> String {
    let mut text = String::from("Play a song. Song title by artist.");
    if names.is_empty() {
        return text;
    }
    text.push_str(" Artists: ");
    let mut first = true;
    for name in names {
        let extra = name.chars().count() + 2;
        if text.chars().count() + extra > MAX_PROMPT_CHARS {
            break;
        }
        if !first {
            text.push_str(", ");
        }
        text.push_str(name);
        first = false;
    }
    text.push('.');
    text
}

/// How much of the model's 30-second window a clip needs. The model is built
/// to look at 30 seconds every time, which is most of the work for a clip of
/// a few seconds; telling it how much audio there really is makes it several
/// times faster.
pub(crate) fn audio_context(samples: usize) -> i32 {
    // Fifty positions a second, plus room for the model's own margin.
    let positions = i32::try_from(samples / 320).unwrap_or(MAX_AUDIO_CTX);
    (positions + 160).clamp(MIN_AUDIO_CTX, MAX_AUDIO_CTX)
}

/// Loads a model. Slow, so the helper does it while the listener is still
/// speaking.
pub(crate) fn load(path: &Path) -> Result<WhisperContext> {
    let Some(text) = path.to_str() else {
        bail!("the model's folder has a name Sonder cannot read");
    };
    let mut parameters = WhisperContextParameters::default();
    parameters.use_gpu(false);
    WhisperContext::new_with_params(text, parameters)
        .context("could not load the speech model; try removing it and downloading it again")
}

/// Turns the audio into text.
pub(crate) fn run(context: &WhisperContext, samples: &[f32], prompt: &str) -> Result<String> {
    let mut state = context
        .create_state()
        .context("could not prepare speech to text")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let cores = thread::available_parallelism().map_or(2, usize::from);
    // A burst of well under a second, so it may use half the processor.
    params.set_n_threads((cores / 2).clamp(2, 6) as i32);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_single_segment(true);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // No retry at a higher temperature: a wrong guess is cheaper than a wait.
    params.set_temperature_inc(0.0);
    params.set_initial_prompt(prompt);
    let mut audio = samples.to_vec();
    if audio.len() < MIN_SAMPLES {
        audio.resize(MIN_SAMPLES, 0.);
    }
    params.set_audio_ctx(audio_context(audio.len()));
    state
        .full(params, &audio)
        .context("speech to text failed")?;
    let mut text = String::new();
    for index in 0..state.full_n_segments() {
        if let Some(segment) = state.get_segment(index) {
            text.push_str(&segment.to_str_lossy().context("unreadable speech")?);
        }
    }
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_lists_names_up_to_its_limit() {
        assert_eq!(prompt(&[]), "Play a song. Song title by artist.");
        let few = prompt(&["glaive".into(), "2hollis".into()]);
        assert!(few.ends_with("Artists: glaive, 2hollis."));
        let many: Vec<String> = (0..200).map(|index| format!("Artist {index}")).collect();
        let long = prompt(&many);
        assert!(long.chars().count() <= MAX_PROMPT_CHARS + 1);
        assert!(long.contains("Artist 0, Artist 1"));
    }

    #[test]
    fn a_short_clip_needs_a_small_part_of_the_window() {
        // Three seconds: 150 positions plus margin.
        assert_eq!(audio_context(48_000), 310.max(MIN_AUDIO_CTX));
        // Nothing is ever below the floor or above the model's own limit.
        assert_eq!(audio_context(0), MIN_AUDIO_CTX);
        assert_eq!(audio_context(16_000 * 60), MAX_AUDIO_CTX);
        assert!(audio_context(16_000 * 10) < MAX_AUDIO_CTX);
    }

    #[test]
    fn a_missing_model_is_reported_not_panicked() {
        assert!(load(Path::new("/definitely/not/a/model.bin")).is_err());
    }

    /// Needs a real model and recordings, so it only runs when asked:
    /// `SONDER_VOICE_MODEL=model.bin SONDER_VOICE_WAVS=a.wav,b.wav cargo test
    /// --lib real_speech -- --ignored --nocapture`. The recordings are 16 kHz
    /// mono 16-bit WAV files.
    #[test]
    #[ignore]
    fn real_speech_is_transcribed() {
        let model = std::env::var("SONDER_VOICE_MODEL").expect("SONDER_VOICE_MODEL");
        let wavs = std::env::var("SONDER_VOICE_WAVS").expect("SONDER_VOICE_WAVS");
        let started = std::time::Instant::now();
        let context = load(Path::new(&model)).expect("the model loads");
        println!("loaded in {:?}", started.elapsed());
        for wav in wavs.split(',') {
            let bytes = std::fs::read(wav).expect("recording");
            let samples: Vec<f32> = bytes[44..]
                .chunks_exact(2)
                .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.)
                .collect();
            let started = std::time::Instant::now();
            let text =
                run(&context, &samples, &prompt(&["The Weeknd".into()])).expect("a transcript");
            println!("{wav}: {text:?} in {:?}", started.elapsed());
        }
    }
}
