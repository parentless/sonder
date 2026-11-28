#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    // Speech to text runs in a helper process of its own; see voice::worker.
    if cadence::run_voice_worker_if_requested() {
        return;
    }
    cadence::run();
}
