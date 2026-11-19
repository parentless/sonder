//! Finding audio on a web page.
//!
//! A link is often a page that holds a song, not the song itself. These
//! functions read a page's markup for the files and embedded players on it,
//! and tell real audio from a web page by its first bytes. They never touch
//! the network, so they are tested on plain text.

use regex::Regex;
use std::sync::LazyLock;
use url::Url;

/// The most files from one page that are worth checking.
const MAX_CANDIDATES: usize = 8;
/// The most embedded players from one page that are worth asking a
/// downloader about.
const MAX_EMBEDS: usize = 3;

/// Keys a page's data uses for a song's address, even with no file extension.
const AUDIO_KEYS: &[&str] = &[
    "contenturl",
    "audio",
    "audiourl",
    "audio_url",
    "mp3",
    "mp3url",
    "stream",
    "streamurl",
    "stream_url",
    "previewurl",
    "preview_url",
    "downloadurl",
    "download_url",
];

/// What a page holds that could be saved.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Files that may be audio, best guess first.
    pub audio: Vec<String>,
    /// Pages of embedded players, for a downloader to try.
    pub embeds: Vec<String>,
    /// What the page calls itself.
    pub title: Option<String>,
}

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(audio|source|iframe|embed|meta|a|link)\b([^>]*)>").expect("tag pattern")
});
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)([\w:.-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
        .expect("attribute pattern")
});
static AUDIO_ELEMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<audio\b.*?(?:</audio>|$)").expect("audio element pattern"));
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("title pattern"));
static DATA_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)["']?([\w-]+)["']?\s*[:=]\s*["'](https?:[^"'\s<>]+)["']"#)
        .expect("data pattern")
});

fn file_pattern() -> String {
    super::DIRECT_EXTENSIONS.join("|")
}

static AUDIO_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(?:https?:)?//[^\s"'<>\\()]+?\.(?:{})(?:\?[^\s"'<>\\()]*)?"#,
        file_pattern()
    ))
    .expect("audio address pattern")
});

/// Decodes the handful of character references that turn up in addresses and
/// titles.
fn unescape(text: &str) -> String {
    text.replace("\\u0026", "&")
        .replace("\\u002F", "/")
        .replace("\\u002f", "/")
        .replace("\\/", "/")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn has_audio_extension(address: &Url) -> bool {
    super::direct_extension(address.as_str()).is_some()
}

/// An address on the page as an absolute web address.
fn resolve(base: &Url, raw: &str) -> Option<Url> {
    let raw = unescape(raw.trim());
    if raw.is_empty() || raw.starts_with(['#', '{', '$']) || raw.contains("${") {
        return None;
    }
    let mut address = base.join(&raw).ok()?;
    if !matches!(address.scheme(), "http" | "https") || address.host_str().is_none() {
        return None;
    }
    address.set_fragment(None);
    Some(address)
}

fn push_unique(into: &mut Vec<String>, address: Url, limit: usize) {
    let text = address.to_string();
    if into.len() < limit && !into.contains(&text) {
        into.push(text);
    }
}

/// Reads a page for audio files and embedded players.
pub fn scan(html: &str, base: &Url) -> Found {
    let html = unescape_scripts(html);
    // Tiers keep the page's own statements ahead of guesses: a meta tag or an
    // audio element says "this is the song"; a link that ends in .mp3 might
    // be anything.
    let mut tiers: [Vec<Url>; 4] = Default::default();
    let mut embeds: Vec<String> = Vec::new();

    // Whatever an audio element lists is a song, whatever its address looks like.
    for element in AUDIO_ELEMENT.find_iter(&html) {
        for tag in TAG.captures_iter(element.as_str()) {
            if tag[1].eq_ignore_ascii_case("source") {
                tiers[1].extend(
                    ATTRIBUTE
                        .captures_iter(&tag[2])
                        .find(|attribute| attribute[1].eq_ignore_ascii_case("src"))
                        .and_then(|attribute| {
                            attribute
                                .get(2)
                                .or_else(|| attribute.get(3))
                                .or_else(|| attribute.get(4))
                                .and_then(|src| resolve(base, src.as_str()))
                        }),
                );
            }
        }
    }

    for tag in TAG.captures_iter(&html) {
        let name = tag[1].to_ascii_lowercase();
        let attributes: Vec<(String, String)> = ATTRIBUTE
            .captures_iter(&tag[2])
            .map(|attribute| {
                let value = attribute
                    .get(2)
                    .or_else(|| attribute.get(3))
                    .or_else(|| attribute.get(4))
                    .map_or("", |value| value.as_str());
                (attribute[1].to_ascii_lowercase(), value.to_owned())
            })
            .collect();
        let get = |wanted: &str| {
            attributes
                .iter()
                .find(|(key, _)| key == wanted)
                .map(|(_, value)| value.as_str())
        };
        match name.as_str() {
            "meta" => {
                let property = get("property").or_else(|| get("name")).unwrap_or("");
                let content = get("content").unwrap_or("");
                match property.to_ascii_lowercase().as_str() {
                    "og:audio"
                    | "og:audio:url"
                    | "og:audio:secure_url"
                    | "twitter:player:stream" => {
                        tiers[0].extend(resolve(base, content));
                    }
                    "og:video" | "og:video:url" | "og:video:secure_url" | "twitter:player" => {
                        embeds.extend(resolve(base, content).map(|address| address.to_string()));
                    }
                    _ => {}
                }
            }
            "audio" => tiers[1].extend(get("src").and_then(|src| resolve(base, src))),
            "source" => {
                let source = get("src").and_then(|src| resolve(base, src));
                let audible = get("type").is_some_and(|kind| kind.starts_with("audio"))
                    || source.as_ref().is_some_and(has_audio_extension);
                if audible {
                    tiers[1].extend(source);
                }
            }
            "iframe" | "embed" => {
                embeds.extend(
                    get("src")
                        .or_else(|| get("data-src"))
                        .and_then(|src| resolve(base, src))
                        .map(|address| address.to_string()),
                );
            }
            "a" | "link" => {
                if let Some(address) = get("href").and_then(|href| resolve(base, href)) {
                    let downloads = get("download").is_some();
                    let audible = get("type").is_some_and(|kind| kind.starts_with("audio"));
                    if has_audio_extension(&address) || audible || (downloads && name == "a") {
                        tiers[3].push(address);
                    }
                }
            }
            _ => {}
        }
    }

    // Players that build themselves from data name the song in a script.
    for data in DATA_KEY.captures_iter(&html) {
        let key = data[1].to_ascii_lowercase();
        if AUDIO_KEYS.contains(&key.as_str())
            && let Some(address) = resolve(base, &data[2])
        {
            tiers[2].push(address);
        }
    }
    for found in AUDIO_URL.find_iter(&html) {
        let text = found.as_str();
        let text = if text.starts_with("//") {
            format!("{}:{text}", base.scheme())
        } else {
            text.to_owned()
        };
        tiers[3].extend(resolve(base, &text));
    }

    let mut audio = Vec::new();
    for address in tiers.into_iter().flatten() {
        push_unique(&mut audio, address, MAX_CANDIDATES);
    }
    let mut kept_embeds = Vec::new();
    for embed in embeds {
        if let Ok(address) = Url::parse(&embed)
            && address.as_str() != base.as_str()
        {
            push_unique(&mut kept_embeds, address, MAX_EMBEDS);
        }
    }
    Found {
        audio,
        embeds: kept_embeds,
        title: page_title(&html),
    }
}

/// Script text often escapes slashes and ampersands; undo that so the
/// addresses inside read as addresses.
fn unescape_scripts(html: &str) -> String {
    if html.contains("\\/") || html.contains("\\u00") {
        html.replace("\\u0026", "&")
            .replace("\\u002F", "/")
            .replace("\\u002f", "/")
            .replace("\\/", "/")
    } else {
        html.to_owned()
    }
}

fn meta_title(html: &str) -> Option<String> {
    for tag in TAG.captures_iter(html) {
        if !tag[1].eq_ignore_ascii_case("meta") {
            continue;
        }
        let attributes: Vec<(String, String)> = ATTRIBUTE
            .captures_iter(&tag[2])
            .map(|attribute| {
                let value = attribute
                    .get(2)
                    .or_else(|| attribute.get(3))
                    .or_else(|| attribute.get(4))
                    .map_or("", |value| value.as_str());
                (attribute[1].to_ascii_lowercase(), value.to_owned())
            })
            .collect();
        let property = attributes
            .iter()
            .find(|(key, _)| key == "property" || key == "name")
            .map(|(_, value)| value.to_ascii_lowercase());
        if matches!(property.as_deref(), Some("og:title" | "twitter:title"))
            && let Some((_, content)) = attributes.iter().find(|(key, _)| key == "content")
        {
            return Some(content.clone());
        }
    }
    None
}

fn page_title(html: &str) -> Option<String> {
    let raw = meta_title(html).or_else(|| {
        TITLE
            .captures(html)
            .and_then(|found| found.get(1))
            .map(|title| title.as_str().to_owned())
    })?;
    let cleaned = unescape(&raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!cleaned.is_empty()).then(|| cleaned.chars().take(150).collect())
}

/// What kind of audio these first bytes are, or `None` for anything else
/// (a web page, an image, a video). `content_type` is what the server said;
/// `address_extension` is the file type the address ends in. The bytes win
/// over both, since servers and addresses are often wrong.
pub fn audio_kind(
    content_type: Option<&str>,
    head: &[u8],
    address_extension: Option<&str>,
) -> Option<String> {
    let content_type = content_type
        .map(|kind| {
            kind.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let says_audio = content_type.starts_with("audio/") || content_type == "application/ogg";

    let start = head
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(0);
    if head[start..].starts_with(b"<") {
        return None;
    }
    let kind = |extension: &str| Some(extension.to_owned());

    if head.starts_with(b"ID3") {
        return kind("mp3");
    }
    if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 {
        // 0xFFF1 and 0xFFF9 are AAC in ADTS frames; the rest are MPEG audio.
        return if head[1] & 0xF6 == 0xF0 {
            kind("aac")
        } else {
            kind("mp3")
        };
    }
    if head.starts_with(b"fLaC") {
        return kind("flac");
    }
    if head.starts_with(b"OggS") {
        let window = &head[..head.len().min(160)];
        return if window.windows(8).any(|chunk| chunk == b"OpusHead") {
            kind("opus")
        } else if says_audio || window.windows(7).any(|chunk| chunk == b"\x01vorbis") {
            kind("ogg")
        } else {
            None
        };
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return kind("wav");
    }
    if head.len() >= 12 && &head[..4] == b"FORM" && matches!(&head[8..12], b"AIFF" | b"AIFC") {
        return kind("aiff");
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let audio_brand = matches!(&head[8..12], b"M4A " | b"M4B " | b"M4P ");
        return (audio_brand || says_audio).then(|| "m4a".to_owned());
    }
    if head.starts_with(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        return kind("wma");
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return says_audio.then(|| "mka".to_owned());
    }
    // Nothing recognisable at the start: believe the server only when it
    // says audio outright.
    if !says_audio {
        return None;
    }
    let from_type = match content_type.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => Some("m4a"),
        "audio/aac" | "audio/aacp" => Some("aac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => Some("wav"),
        "audio/opus" => Some("opus"),
        _ => None,
    };
    from_type
        .map(str::to_owned)
        .or_else(|| address_extension.map(str::to_owned))
        // Anything else is something ffmpeg can read, so name it as one that
        // gets converted.
        .or_else(|| Some("mka".to_owned()))
}

/// A file name from a `Content-Disposition` header, without its folder.
pub fn disposition_name(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    let (at, encoded) = match lower.find("filename*=") {
        Some(at) => (at + "filename*=".len(), true),
        None => (lower.find("filename=")? + "filename=".len(), false),
    };
    let rest = header[at..].trim_start();
    let value = if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next()?.to_owned()
    } else {
        rest.split(';').next()?.trim().to_owned()
    };
    let value = if encoded {
        // `UTF-8''Some%20Song.mp3`
        let text = value.rsplit("''").next().unwrap_or(&value);
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    } else {
        value
    };
    let name = value.rsplit(['/', '\\']).next()?.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://music.example/albums/one/").unwrap()
    }

    #[test]
    fn an_audio_element_is_found_and_its_address_made_absolute() {
        let found = scan(
            r#"<html><head><title>Harbour &amp; Lights</title></head>
               <body><audio controls src="../files/track 1.mp3"></audio></body></html>"#,
            &base(),
        );
        assert_eq!(
            found.audio,
            ["https://music.example/albums/files/track%201.mp3"]
        );
        assert_eq!(found.title.as_deref(), Some("Harbour & Lights"));
    }

    #[test]
    fn a_source_inside_an_audio_player_counts_even_without_an_extension() {
        let found = scan(
            r#"<audio><source src="/stream/42" type="audio/mpeg"></audio>
               <video><source src="/clip/9" type="video/mp4"></video>"#,
            &base(),
        );
        assert_eq!(found.audio, ["https://music.example/stream/42"]);
    }

    #[test]
    fn the_pages_own_statements_come_before_plain_links() {
        let found = scan(
            r#"<a href="/ads/jingle.mp3">ad</a>
               <meta property="og:audio" content="https://cdn.example/real.mp3?token=a&amp;b=2">
               <audio src="/second.ogg"></audio>"#,
            &base(),
        );
        assert_eq!(
            found.audio,
            [
                "https://cdn.example/real.mp3?token=a&b=2",
                "https://music.example/second.ogg",
                "https://music.example/ads/jingle.mp3",
            ]
        );
    }

    #[test]
    fn players_that_build_from_a_script_are_read_from_the_script() {
        let found = scan(
            r#"<script>
                 player.load({"title":"Night Drive","file":"https:\/\/cdn.example\/a\/night.flac",
                              "streamUrl":"https:\/\/cdn.example\/s\/99?sig=1"});
               </script>"#,
            &base(),
        );
        assert_eq!(
            found.audio,
            [
                "https://cdn.example/s/99?sig=1",
                "https://cdn.example/a/night.flac"
            ]
        );
    }

    #[test]
    fn a_protocol_relative_address_takes_the_pages_scheme() {
        let found = scan(
            r#"<script>var a = "//cdn.example/x/y.m4a";</script>"#,
            &base(),
        );
        assert_eq!(found.audio, ["https://cdn.example/x/y.m4a"]);
    }

    #[test]
    fn embedded_players_are_kept_for_a_downloader_to_try() {
        let found = scan(
            r#"<iframe src="https://www.youtube.com/embed/abc123"></iframe>
               <iframe src="https://www.youtube.com/embed/abc123"></iframe>
               <meta property="og:video:url" content="https://player.example/v/7">"#,
            &base(),
        );
        assert_eq!(
            found.embeds,
            [
                "https://www.youtube.com/embed/abc123",
                "https://player.example/v/7"
            ]
        );
        assert!(found.audio.is_empty());
    }

    #[test]
    fn candidates_are_deduplicated_and_capped() {
        let links: String = (0..20)
            .map(|n| format!(r#"<a href="/t/{n}.mp3">t</a><a href="/t/{n}.mp3">t</a>"#))
            .collect();
        let found = scan(&links, &base());
        assert_eq!(found.audio.len(), MAX_CANDIDATES);
        assert_eq!(found.audio[0], "https://music.example/t/0.mp3");
        assert_eq!(found.audio[1], "https://music.example/t/1.mp3");
    }

    #[test]
    fn a_page_with_no_audio_finds_none() {
        let found = scan(
            r##"<title>About us</title><a href="/contact">Contact</a><a href="#top">Top</a>
               <img src="/logo.png">"##,
            &base(),
        );
        assert!(found.audio.is_empty());
        assert!(found.embeds.is_empty());
        assert_eq!(found.title.as_deref(), Some("About us"));
    }

    #[test]
    fn the_first_bytes_tell_audio_from_a_web_page() {
        assert_eq!(
            audio_kind(None, b"ID3\x04\0\0\0", None).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            audio_kind(None, &[0xFF, 0xFB, 0x90, 0], None).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            audio_kind(None, &[0xFF, 0xF1, 0x50, 0], None).as_deref(),
            Some("aac")
        );
        assert_eq!(
            audio_kind(None, b"fLaC\0\0\0\"", None).as_deref(),
            Some("flac")
        );
        assert_eq!(
            audio_kind(None, b"RIFF\0\0\0\0WAVEfmt ", None).as_deref(),
            Some("wav")
        );
        assert_eq!(
            audio_kind(None, b"OggS\0\x02\0\0\0\0\0\0\0\0\x01vorbis", None).as_deref(),
            Some("ogg")
        );
        assert_eq!(
            audio_kind(None, b"OggS\0\x02\0\0\0\0\0\0\0\0OpusHead", None).as_deref(),
            Some("opus")
        );
        assert_eq!(
            audio_kind(None, b"\0\0\0\x20ftypM4A \0\0\0\0", None).as_deref(),
            Some("m4a")
        );
        // A video container is not a song unless the server says it is audio.
        assert_eq!(audio_kind(None, b"\0\0\0\x20ftypisom\0\0\0\0", None), None);
        assert_eq!(
            audio_kind(Some("audio/mp4"), b"\0\0\0\x20ftypisom\0\0\0\0", None).as_deref(),
            Some("m4a")
        );
        assert_eq!(
            audio_kind(Some("text/html"), b"<!doctype html><html>", None),
            None
        );
        assert_eq!(
            audio_kind(Some("audio/mpeg"), b"  \n<html>Forbidden</html>", None),
            None
        );
        assert_eq!(
            audio_kind(Some("application/octet-stream"), b"\x01\x02\x03", None),
            None
        );
    }

    #[test]
    fn a_server_that_says_audio_is_believed_when_the_bytes_say_nothing() {
        assert_eq!(
            audio_kind(
                Some("audio/mpeg; charset=binary"),
                b"\x01\x02\x03\x04",
                None
            )
            .as_deref(),
            Some("mp3")
        );
        assert_eq!(
            audio_kind(Some("audio/x-unknown"), b"\x01\x02\x03\x04", Some("wma")).as_deref(),
            Some("wma")
        );
        assert_eq!(
            audio_kind(Some("audio/x-unknown"), b"\x01\x02\x03\x04", None).as_deref(),
            Some("mka")
        );
    }

    #[test]
    fn a_download_header_names_the_file() {
        assert_eq!(
            disposition_name(r#"attachment; filename="Night Drive.mp3""#).as_deref(),
            Some("Night Drive.mp3")
        );
        assert_eq!(
            disposition_name("attachment; filename*=UTF-8''Caf%C3%A9%20Noir.flac").as_deref(),
            Some("Café Noir.flac")
        );
        assert_eq!(
            disposition_name(r#"inline; filename="../../evil.mp3""#).as_deref(),
            Some("evil.mp3")
        );
        assert_eq!(disposition_name("inline"), None);
    }
}
