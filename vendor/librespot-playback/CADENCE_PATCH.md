# Cadence local files patch

Upstream: librespot-playback 0.8.0, from crates.io; license: MIT.

Upstream can already play `spotify:local:` URIs, but only 44.1 kHz stereo
files, and it finds them by scanning folders once when the player starts. The
changes, each marked "Cadence patch" in the source:

- `src/decoder/format_converter.rs` (new) and `src/decoder/symphonia_decoder.rs`:
  local files at any sample rate or channel count are converted to 44.1 kHz
  stereo with rubato, the resampler the app already ships. The output is
  exactly as long as the file, and the format is read from the decoded audio,
  since ALAC only states its channels there. Spotify streams pass straight
  through.
- `src/config.rs` and `src/player.rs`: `PlayerConfig::local_file_resolver`
  looks a local file's path up when it loads, so the app's own index needs no
  scan at startup and new folders play without a new player. A clip shorter
  than one second no longer divides by zero.
- `Cargo.toml`: Symphonia also decodes AAC and ALAC in `.m4a` and PCM `.wav`;
  rubato is a dependency.

Keep this patch until upstream resamples local files and accepts a resolver.
