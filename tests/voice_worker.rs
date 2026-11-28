//! Speech to text runs in a helper process: Sonder's own executable started
//! with `--voice-worker`. These start the real program, as the app does.

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

fn request(prompt: &str, samples: &[f32]) -> Vec<u8> {
    let mut bytes = (prompt.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(prompt.as_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn run_worker(model: &str, input: &[u8]) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args(["--voice-worker", model])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the helper starts");
    child
        .stdin
        .take()
        .expect("input")
        .write_all(input)
        .expect("the request is sent");
    let mut output = String::new();
    child
        .stdout
        .take()
        .expect("output")
        .read_to_string(&mut output)
        .expect("the answer is read");
    let status = child.wait().expect("the helper ends");
    (status.success(), output)
}

#[test]
fn a_missing_model_is_answered_with_a_reason_and_the_helper_ends() {
    let (success, output) = run_worker(
        "/definitely/not/a/model.bin",
        &request("Play a song.", &vec![0.0; 16_000]),
    );
    assert!(success, "the helper ends cleanly");
    assert!(output.starts_with("ERR "), "{output}");
    assert!(output.contains("model"), "{output}");
}

#[test]
fn a_damaged_request_is_answered_with_a_reason() {
    let (success, output) = run_worker("/definitely/not/a/model.bin", &[1, 2]);
    assert!(success);
    assert!(output.starts_with("ERR "), "{output}");
}

/// Needs a real model: `SONDER_VOICE_MODEL=model.bin cargo test --test
/// voice_worker -- --ignored`. Silence comes back as no words, not a crash.
#[test]
#[ignore]
fn a_real_model_answers_for_silence() {
    let model = std::env::var("SONDER_VOICE_MODEL").expect("SONDER_VOICE_MODEL");
    let (success, output) = run_worker(&model, &request("Play a song.", &vec![0.0; 32_000]));
    assert!(success);
    assert!(output.starts_with("OK"), "{output}");
}
