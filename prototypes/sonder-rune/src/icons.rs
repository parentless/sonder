//! Sonder's own icons (`assets/icons/cadence`), rasterised once and tinted where drawn.

use rune::{El, Image, ImageCache, Waker, el};

/// The icon file, with its `currentColor` made white so a tint can recolour it.
macro_rules! og {
    ($name:literal) => {
        include_str!(concat!("../../../assets/icons/cadence/", $name, ".svg"))
            .replace("currentColor", "#fff")
    };
}

pub struct Icons {
    pub heart: Image,
    pub heart_fill: Image,
    pub home: Image,
    pub home_fill: Image,
    pub playlists: Image,
    pub recent: Image,
    pub play: Image,
    pub pause: Image,
    pub prev: Image,
    pub next: Image,
    pub shuffle: Image,
    pub queue: Image,
    pub back: Image,
    pub down: Image,
    pub search: Image,
    pub note: Image,
    pub volume: Image,
    pub muted: Image,
    pub settings: Image,
    pub logout: Image,
    pub lyrics: Image,
    pub folder: Image,
    pub close: Image,
    pub pin: Image,
    pub more: Image,
    /// Sonder's mark in its own colours (not tinted).
    pub logo: Image,
}

impl Icons {
    pub fn new(waker: Waker) -> Icons {
        let cache = ImageCache::new(waker);
        let make = |svg: String, size: f32| cache.svg(svg.as_bytes(), size, 2.0);
        let logo = cache.svg(
            &include_bytes!("../../../assets/sonder-mark.svg")[..],
            28.0,
            3.0,
        );
        Icons {
            logo,
            heart: make(og!("heart"), 20.0),
            heart_fill: make(og!("heart-filled"), 20.0),
            home: make(og!("home"), 20.0),
            home_fill: make(og!("home-filled"), 20.0),
            playlists: make(og!("playlist"), 20.0),
            recent: make(og!("clock"), 20.0),
            play: make(og!("play"), 18.0),
            pause: make(og!("pause"), 18.0),
            prev: make(og!("skip-back"), 18.0),
            next: make(og!("skip-forward"), 18.0),
            shuffle: make(og!("shuffle"), 20.0),
            queue: make(og!("queue"), 20.0),
            back: make(og!("chevron-left"), 18.0),
            down: make(og!("chevron-down"), 18.0),
            search: make(og!("search"), 18.0),
            note: make(og!("music-note"), 24.0),
            volume: make(og!("speaker"), 20.0),
            muted: make(og!("speaker-muted"), 20.0),
            settings: make(og!("settings"), 20.0),
            logout: make(og!("sign-out"), 20.0),
            lyrics: make(og!("lyrics"), 20.0),
            folder: make(og!("folder"), 20.0),
            close: make(og!("close"), 20.0),
            pin: make(og!("pin"), 20.0),
            more: make(
                concat!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#fff">"##,
                    r##"<circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>"##
                )
                .to_owned(),
                20.0,
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
