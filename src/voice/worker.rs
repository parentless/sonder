//! Speech to text in a short-lived helper process.
//!
//! Sonder starts the helper the moment the shortcut goes down. It loads the
//! speech model while the listener is still talking, so loading costs no
//! waiting. When the key comes up Sonder sends the recording, the helper
//! answers and exits, and Windows takes back every byte it used. Nothing of
//! the model, or of the memory it needed, ever lives in the main process.
//!
//! The helper is Sonder's own executable run with `--voice-worker <model>`.
//! It reads one request on its input and writes one line on its output:
//! `OK <words>` or `ERR <reason>`. Its error stream is discarded, because
//! whisper.cpp is chatty and nothing reads it.

use super::transcribe;
use anyhow::{Context as _, Result, bail, ensure};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The first argument that makes the executable act as the helper.
pub(crate) const FLAG: &str = "--voice-worker";
/// A helper that has not answered by then is stopped.
const TIMEOUT: Duration = Duration::from_secs(45);
/// A request is a few seconds of speech.
const MAX_REQUEST_BYTES: usize = 4 + 1024 + 16_000 * 4 * 30;

/// A request: the prompt, then the audio as 32-bit floats.
pub(crate) fn encode_request(prompt: &str, samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + prompt.len() + samples.len() * 4);
    bytes.extend_from_slice(&(prompt.len() as u32).to_le_bytes());
    bytes.extend_from_slice(prompt.as_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

pub(crate) fn decode_request(bytes: &[u8]) -> Result<(String, Vec<f32>)> {
    ensure!(bytes.len() >= 4, "the request was empty");
    let prompt_length = u32::from_le_bytes(bytes[..4].try_into().expect("four bytes")) as usize;
    ensure!(
        bytes.len() >= 4 + prompt_length,
        "the request was cut short"
    );
    let prompt = std::str::from_utf8(&bytes[4..4 + prompt_length])
        .context("the request's prompt was not text")?
        .to_owned();
    let audio = &bytes[4 + prompt_length..];
    ensure!(
        audio.len().is_multiple_of(4),
        "the request's audio was cut short"
    );
    let samples = audio
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect();
    Ok((prompt, samples))
}

/// If this process was started as the helper, does its work and returns true.
/// Called first thing in `main`, before anything else starts.
pub fn run_if_requested() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().is_none_or(|flag| flag != FLAG) {
        return false;
    }
    let model = arguments.next().map(PathBuf::from);
    let answer = match model {
        Some(model) => work(&model),
        None => Err(anyhow::anyhow!("no speech model was given")),
    };
    let line = match answer {
        Ok(text) => format!("OK {}", text.replace(['\r', '\n'], " ")),
        Err(error) => format!("ERR {}", format!("{error:#}").replace(['\r', '\n'], " ")),
    };
    let mut output = std::io::stdout();
    let _ = writeln!(output, "{line}");
    let _ = output.flush();
    true
}

fn work(model: &Path) -> Result<String> {
    // Loading starts before the request arrives: the listener is still
    // speaking, so this time is free.
    let context = transcribe::load(model);
    let mut request = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut request)
        .context("could not read the request")?;
    ensure!(
        request.len() <= MAX_REQUEST_BYTES,
        "the request was too large"
    );
    let (prompt, samples) = decode_request(&request)?;
    transcribe::run(&mut context?, &samples, &prompt)
}

/// A helper process waiting for its request.
pub(crate) struct Worker {
    child: Option<Child>,
}

impl Worker {
    /// Starts the helper, which begins loading the model at once.
    pub(crate) fn start(model: &Path) -> Result<Self> {
        let executable = std::env::current_exe().context("could not find Sonder's own program")?;
        let mut command = Command::new(executable);
        command
            .arg(FLAG)
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            // No console window flashing up.
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().context("could not start speech to text")?;
        Ok(Self { child: Some(child) })
    }

    /// Sends the recording. The words arrive on the returned channel, and the
    /// helper has ended by then.
    pub(crate) fn finish(
        mut self,
        samples: Vec<f32>,
        prompt: String,
    ) -> async_channel::Receiver<Result<String, String>> {
        let (send, answer) = async_channel::bounded(1);
        let Some(child) = self.child.take() else {
            let _ = send.try_send(Err("speech to text was not running".into()));
            return answer;
        };
        let spawned = thread::Builder::new()
            .name("sonder-voice-request".into())
            .spawn({
                let send = send.clone();
                move || {
                    let result = exchange(child, &samples, &prompt).map_err(|e| format!("{e:#}"));
                    let _ = send.send_blocking(result);
                }
            });
        if let Err(error) = spawned {
            let _ = send.try_send(Err(format!("could not start speech to text: {error}")));
        }
        answer
    }
}

impl Drop for Worker {
    /// A helper that was never given a request is stopped.
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            reap(child);
        }
    }
}

/// Stops a helper and collects it, without making anyone wait.
fn reap(mut child: Child) {
    let _ = child.kill();
    let _ = thread::Builder::new()
        .name("sonder-voice-reap".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

fn exchange(mut child: Child, samples: &[f32], prompt: &str) -> Result<String> {
    let started = Instant::now();
    let Some(mut input) = child.stdin.take() else {
        reap(child);
        bail!("speech to text has no input");
    };
    // A helper that already failed has closed its input; its answer says why.
    let _ = input.write_all(&encode_request(prompt, samples));
    drop(input);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > TIMEOUT => {
                reap(child);
                bail!("speech to text took too long");
            }
            Ok(None) => thread::sleep(Duration::from_millis(4)),
            Err(error) => {
                reap(child);
                return Err(error).context("lost speech to text");
            }
        }
    };
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(64 * 1024).read_to_string(&mut output);
    }
    let line = output.lines().last().unwrap_or("").trim();
    if let Some(words) = line.strip_prefix("OK") {
        return Ok(words.trim().to_owned());
    }
    if let Some(reason) = line.strip_prefix("ERR") {
        bail!("{}", reason.trim());
    }
    bail!("speech to text stopped unexpectedly ({status})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_survives_the_trip() {
        let samples = vec![0.0, 0.25, -0.5, 1.0];
        let bytes = encode_request("Play a song. Artists: Glaive.", &samples);
        let (prompt, back) = decode_request(&bytes).unwrap();
        assert_eq!(prompt, "Play a song. Artists: Glaive.");
        assert_eq!(back, samples);
        let (prompt, back) = decode_request(&encode_request("", &[])).unwrap();
        assert!(prompt.is_empty() && back.is_empty());
    }

    #[test]
    fn a_damaged_request_is_refused() {
        assert!(decode_request(&[]).is_err());
        assert!(decode_request(&[9, 0, 0, 0, b'a']).is_err());
        let mut bytes = encode_request("x", &[0.5]);
        bytes.pop();
        assert!(decode_request(&bytes).is_err());
    }
}
