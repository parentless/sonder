//! Turns recorded speech into text with whisper.cpp, on this computer.
//!
//! The model is loaded the first time it is needed and let go again after a
//! while without use, so it costs memory only around the moments it works.

use anyhow::{Context as _, Result, bail};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// How long the model stays in memory after its last use.
const KEEP_LOADED: Duration = Duration::from_secs(90);
/// The model reads at least this much audio, so very short clips are padded.
const MIN_SAMPLES: usize = 16_000;
/// The most prompt text sent along. Names the listener plays are given to the
/// model so it spells them as Spotify does.
const MAX_PROMPT_CHARS: usize = 400;

struct Job {
    model: PathBuf,
    samples: Vec<f32>,
    prompt: String,
    reply: async_channel::Sender<Result<String, String>>,
}

/// The worker thread is started by the first request.
#[derive(Default)]
pub(crate) struct Transcriber {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
}

impl Transcriber {
    /// Starts turning `samples` into text. The answer arrives on the channel.
    pub(crate) fn transcribe(
        &self,
        model: PathBuf,
        samples: Vec<f32>,
        names: &[String],
    ) -> async_channel::Receiver<Result<String, String>> {
        let (reply, answer) = async_channel::bounded(1);
        let mut job = Some(Job {
            model,
            samples,
            prompt: prompt(names),
            reply,
        });
        let mut jobs = self.jobs.lock().unwrap_or_else(|error| error.into_inner());
        for _ in 0..2 {
            if jobs.is_none() {
                let (sender, receiver) = mpsc::channel();
                match thread::Builder::new()
                    .name("sonder-voice-whisper".into())
                    .spawn(move || work(receiver))
                {
                    Ok(_) => *jobs = Some(sender),
                    Err(error) => {
                        if let Some(job) = job.take() {
                            let _ = job
                                .reply
                                .try_send(Err(format!("could not start speech to text: {error}")));
                        }
                        break;
                    }
                }
            }
            let Some(sender) = jobs.as_ref() else { break };
            match sender.send(job.take().expect("job is kept until sent")) {
                Ok(()) => break,
                // The worker ended; start a new one and try once more.
                Err(mpsc::SendError(returned)) => {
                    job = Some(returned);
                    *jobs = None;
                }
            }
        }
        answer
    }
}

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

fn work(jobs: mpsc::Receiver<Job>) {
    let mut loaded: Option<(PathBuf, WhisperContext)> = None;
    loop {
        match jobs.recv_timeout(KEEP_LOADED) {
            Ok(job) => {
                let answer = run(&mut loaded, &job).map_err(|error| format!("{error:#}"));
                let _ = job.reply.send_blocking(answer);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => loaded = None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn run(loaded: &mut Option<(PathBuf, WhisperContext)>, job: &Job) -> Result<String> {
    if loaded.as_ref().is_none_or(|(path, _)| *path != job.model) {
        *loaded = None;
        *loaded = Some((job.model.clone(), load(&job.model)?));
    }
    let (_, context) = loaded.as_ref().expect("the model was just loaded");
    let mut state = context
        .create_state()
        .context("could not prepare speech to text")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let cores = thread::available_parallelism().map_or(2, usize::from);
    // Leave room for whatever is in the foreground, such as a game.
    params.set_n_threads(cores.saturating_sub(2).clamp(2, 4) as i32);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(true);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_initial_prompt(&job.prompt);
    let mut samples = job.samples.clone();
    if samples.len() < MIN_SAMPLES {
        samples.resize(MIN_SAMPLES, 0.);
    }
    state
        .full(params, &samples)
        .context("speech to text failed")?;
    let mut text = String::new();
    for index in 0..state.full_n_segments() {
        if let Some(segment) = state.get_segment(index) {
            text.push_str(&segment.to_str_lossy().context("unreadable speech")?);
        }
    }
    Ok(text.trim().to_owned())
}

fn load(path: &Path) -> Result<WhisperContext> {
    let Some(text) = path.to_str() else {
        bail!("the model's folder has a name Sonder cannot read");
    };
    WhisperContext::new_with_params(text, WhisperContextParameters::default())
        .context("could not load the speech model; try removing it and downloading it again")
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
    fn a_missing_model_is_reported_not_panicked() {
        let transcriber = Transcriber::default();
        let answer = transcriber.transcribe(
            PathBuf::from("/definitely/not/a/model.bin"),
            vec![0.; 16_000],
            &[],
        );
        let result = answer.recv_blocking().expect("an answer");
        assert!(result.is_err());
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
        let transcriber = Transcriber::default();
        for wav in wavs.split(',') {
            let bytes = std::fs::read(wav).expect("recording");
            let samples: Vec<f32> = bytes[44..]
                .chunks_exact(2)
                .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.)
                .collect();
            let started = std::time::Instant::now();
            let text = transcriber
                .transcribe(PathBuf::from(&model), samples, &["The Weeknd".into()])
                .recv_blocking()
                .expect("an answer")
                .expect("a transcript");
            println!("{wav}: {text:?} in {:?}", started.elapsed());
        }
    }
}
