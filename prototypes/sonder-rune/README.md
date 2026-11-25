# Sonder on Rune

The new front end, built on [Rune](https://github.com/anomalyco/rune) (Anomaly's UI
framework). It reuses Sonder's backend crate (`cadence`, built without its `ui`
feature): Spotify sign-in, library, playback and storage. Only the interface is new.

- **Design:** layout A (borderless sidebar, curved page edge, album-tint glow, hearts
  on every song row) plus B's Now Playing sheet, where the player bar's cover flies
  into the big one (`El::matched`).
- **Data:** real. Liked Songs, playlists, recently played, play/pause/next/previous,
  likes, covers (fetched off-thread, 320 kept decoded).
- **Not yet:** Home, search over Spotify, artist and album pages, lyrics, settings,
  seek and volume, queue editing, local files, tray, updates, themes.

Build: `cargo build --release` (Rune pins Rust 1.99.1; `rust-toolchain.toml` selects it).
Pictures without Spotify: `SONDER_OFFLINE=1 cargo run --release -- --snap out.png --scene liked`
(scenes: liked, rail, playlists, playlist, search, nowplaying, login, setup).
Tests: `cargo test --release`.
