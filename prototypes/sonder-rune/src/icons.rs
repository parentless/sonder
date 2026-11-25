//! Line icons as small SVGs, rasterised once and tinted where they are drawn.

use rune::{El, Image, ImageCache, Waker, el};

macro_rules! svg {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" "##,
            r##"stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

macro_rules! solid {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#fff" stroke="#fff" "##,
            r##"stroke-width="1.2" stroke-linejoin="round" stroke-linecap="round">"##,
            $body,
            "</svg>"
        )
    };
}

const HEART: &str = "M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z";

pub struct Icons {
    pub heart: Image,
    pub heart_fill: Image,
    pub home: Image,
    pub playlists: Image,
    pub recent: Image,
    pub play: Image,
    pub pause: Image,
    pub prev: Image,
    pub next: Image,
    pub shuffle: Image,
    pub queue: Image,
    pub back: Image,
    pub forward: Image,
    pub down: Image,
    pub search: Image,
    pub note: Image,
}

impl Icons {
    pub fn new(waker: Waker) -> Icons {
        let cache = ImageCache::new(waker);
        let make = |svg: &str, size: f32| cache.svg(svg.as_bytes(), size, 2.0);
        Icons {
            heart: make(
                svg!(
                    r#"<path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z"/>"#
                ),
                20.0,
            ),
            heart_fill: make(
                solid!(
                    r#"<path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z"/>"#
                ),
                20.0,
            ),
            home: make(
                svg!(r#"<path d="M4 11l8-7 8 7v8a1 1 0 0 1-1 1h-4v-6H9v6H5a1 1 0 0 1-1-1z"/>"#),
                20.0,
            ),
            playlists: make(
                svg!(
                    r#"<rect x="5" y="7" width="14" height="13" rx="3"/><path d="M8 3.5h8"/><path d="M10 14.5v-3l4-1v3"/>"#
                ),
                20.0,
            ),
            recent: make(
                svg!(r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#),
                20.0,
            ),
            play: make(solid!(r#"<path d="M8 5l11 7-11 7z"/>"#), 22.0),
            pause: make(solid!(r#"<path d="M8 5v14M16 5v14"/>"#), 22.0),
            prev: make(
                solid!(r#"<path d="M7 6v12"/><path d="M18 6l-9 6 9 6z"/>"#),
                20.0,
            ),
            next: make(
                solid!(r#"<path d="M17 6v12"/><path d="M6 6l9 6-9 6z"/>"#),
                20.0,
            ),
            shuffle: make(
                svg!(r#"<path d="M4 7h4l8 10h4M4 17h4l2-2.5M16 7h4M18 5l2 2-2 2M18 15l2 2-2 2"/>"#),
                20.0,
            ),
            queue: make(
                svg!(r#"<path d="M4 7h12M4 12h8M4 17h5"/><path d="M16 14l5 3-5 3z"/>"#),
                20.0,
            ),
            back: make(svg!(r#"<path d="M15 6l-6 6 6 6"/>"#), 18.0),
            forward: make(svg!(r#"<path d="M9 6l6 6-6 6"/>"#), 18.0),
            down: make(svg!(r#"<path d="M6 9l6 6 6-6"/>"#), 18.0),
            search: make(
                svg!(r#"<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4-4"/>"#),
                18.0,
            ),
            note: make(
                svg!(
                    r#"<path d="M9 18V6l10-2v12"/><circle cx="7" cy="18" r="2.4"/><circle cx="17" cy="16" r="2.4"/>"#
                ),
                24.0,
            ),
        }
    }
}

/// A tinted icon of `size` points.
pub fn icon(image: &Image, size: f32, tint: rune::Color) -> El {
    el().size(size, size)
        .pixels(image.pixels())
        .pixels_tint(tint)
}

#[allow(dead_code)]
const _: &str = HEART;
