# Saving songs from links

Local Files has a **From link** button that opens a dialog (close it with ×, Escape or a click outside). It saves the audio behind a link into one of the Local Files folders, where the folder watcher picks it up and it plays like any other file.

## What works

| Link | How it is saved |
| --- | --- |
| A direct link to an audio file (`.mp3`, `.flac`, `.m4a`, `.ogg`, `.wav`, ...) | Sonder fetches it itself. Nothing else is installed. Choosing a different format converts it with ffmpeg. |
| A page or stream on any of the 1,800+ sites yt-dlp supports | yt-dlp finds the best audio, ffmpeg converts and tags it. |
| A playlist link | Offered as **All N** or **First song**; at most 100 songs. |
| Several links pasted together | Looked up three at a time and saved two at a time; at most 25 links. |

Spotify, Apple Music, Tidal, Amazon Music, Deezer and Qobuz links are turned down with a reason: that audio is copy-protected. Anything that needs a sign-in or is DRM-protected fails with a plain sentence, not a stack trace. The dialog reminds the listener to save only audio they have the right to use.

## Formats

| Choice | Result |
| --- | --- |
| Best | The best stream the source has, kept as it is when Sonder can play it (AAC to M4A, MP3, FLAC, Vorbis) and otherwise saved as FLAC so nothing more is lost. |
| MP3 | 320 kbps. |
| FLAC | Lossless. A lossy source stays as good as it was; FLAC does not add quality it never had. |
| M4A | 256 kbps AAC. |
| Ogg | Vorbis at its highest quality. |
| WAV | Uncompressed. |

A source already in the chosen format is kept, not re-encoded. Opus and WebM are not offered because Sonder's player does not decode them. Cover art and tags are embedded; an upload named `Artist - Title` gets that artist and title when the site did not name the artist.

## The tools

The first save from a page (not a direct link) fetches, once:

* `yt-dlp` from the yt-dlp releases, checked against the `SHA2-256SUMS` file published beside it.
* `ffmpeg` from the yt-dlp FFmpeg builds, checked against `checksums.sha256` and unpacked with the `tar` Windows ships.

They live in `%LOCALAPPDATA%\twoducks\cadence\data\tools`. A Sonder-managed yt-dlp updates itself after 14 days, and once more and retries when a site refuses a download, because sites change often. A `yt-dlp` or `ffmpeg` already on `PATH` is used when Sonder has none of its own; `SONDER_YT_DLP` and `SONDER_FFMPEG` point at specific files. On macOS Sonder only uses tools it finds (Homebrew paths included); it does not fetch them.

## Where the work happens

`src/link_download.rs` has no UI: it parses links, looks them up, fetches tools and saves files, blocking, on its own threads. `src/app/link_downloads.rs` keeps the list of lookups and downloads that outlives the popover and tells the library when a file landed. `src/app/link_popover.rs` is the dialog view. Progress reaches the screen at most every 120 ms.

`cargo test real_links -- --ignored --nocapture` with `SONDER_TEST_URL` (and optionally `SONDER_TEST_FORMATS=best,mp3,flac`) saves a real link in each format.
