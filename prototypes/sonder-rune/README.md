# Rune prototype of Sonder's Library page

Throwaway. Answers one question: is Rune good enough on Windows to be worth porting to?
Judge: 20,000-row scrolling, search typing, sidebar slide, credits dialog exit, memory.
Build: `cargo build --release` (Rune pins Rust 1.99.1; `rust-toolchain.toml` selects it).
Headless picture: `cargo run --release -- --snap out.png --scene rail`.
