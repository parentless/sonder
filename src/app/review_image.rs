//! The finished album review as a wide PNG: cover top left, every song with
//! its stars and comment on the right, on a background taken from the cover.
//!
//! Drawn in plain Rust from the Inter fonts the app already embeds, so it needs
//! no window, no GPU and no extra download. It runs on a background thread
//! only when someone asks for the image, and the pixels are dropped as soon as
//! they are copied or saved: nothing is stored, the image is made again on
//! demand.

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _};
use image::{Rgba, RgbaImage};

use super::fonts;

const WIDTH: u32 = 1600;
const MARGIN: u32 = 72;
const COVER: u32 = 460;
const COLUMN_GAP: u32 = 72;
/// Albums longer than this are cut off in the image (the review keeps them all).
const MAX_ROWS: usize = 80;
const STAR_COLOR: [u8; 3] = [0xF5, 0xB9, 0x42];

pub(super) struct ShareCard {
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    /// Stars (1-5) for every track, in album order.
    pub ratings: Vec<u8>,
    pub titles: Vec<String>,
    pub comments: Vec<String>,
    pub cover: Option<RgbaImage>,
}

impl ShareCard {
    pub(super) fn score(&self) -> f32 {
        let total: u32 = self.ratings.iter().map(|r| u32::from(*r)).sum();
        total as f32 / self.ratings.len().max(1) as f32 * 2.
    }
}

struct Fonts {
    medium: FontRef<'static>,
    semibold: FontRef<'static>,
    bold: FontRef<'static>,
}

impl Fonts {
    fn load() -> anyhow::Result<Self> {
        Ok(Self {
            medium: FontRef::try_from_slice(fonts::INTER_MEDIUM)?,
            semibold: FontRef::try_from_slice(fonts::INTER_SEMIBOLD)?,
            bold: FontRef::try_from_slice(fonts::INTER_BOLD)?,
        })
    }
}

/// Renders `card` and returns the PNG bytes.
pub(super) fn render(card: &ShareCard) -> anyhow::Result<Vec<u8>> {
    let fonts = Fonts::load()?;
    let (top, bottom) = card
        .cover
        .as_ref()
        .map_or(([70., 110., 210.], [18., 28., 56.]), palette);
    let rows = card.titles.len().min(MAX_ROWS);
    let dense = rows > 40;
    let row_height = |index: usize| -> u32 {
        let has_comment = card.comments.get(index).is_some_and(|c| !c.is_empty());
        let base = if dense { 48 } else { 62 };
        base + if has_comment { 34 } else { 0 }
    };
    let rows_height: u32 = (0..rows).map(row_height).sum();
    let left_height = COVER + 44 + 2 * 56 + 40 + 140 + 70 + 54;
    let height = MARGIN * 2 + rows_height.max(left_height);

    let mut image = RgbaImage::new(WIDTH, height);
    gradient(&mut image, top, bottom);

    // Left column: cover, title, artist, score.
    let left = MARGIN;
    if let Some(cover) = &card.cover {
        draw_cover(&mut image, cover, left, MARGIN, COVER, 32);
    } else {
        rounded(
            &mut image,
            left,
            MARGIN,
            COVER,
            COVER,
            32,
            [255, 255, 255],
            0.14,
        );
    }
    let mut y = MARGIN + COVER + 44;
    for line in wrap(&fonts.semibold, 46., &card.title, COVER as f32, 2) {
        text(
            &mut image,
            &fonts.semibold,
            46.,
            left as f32,
            y as f32 + 40.,
            [255; 3],
            1.,
            &line,
        );
        y += 56;
    }
    let mut credit = card.artist.clone();
    if let Some(year) = &card.year {
        credit.push_str(" · ");
        credit.push_str(year);
    }
    let credit = truncate(&fonts.medium, 28., &credit, COVER as f32);
    text(
        &mut image,
        &fonts.medium,
        28.,
        left as f32,
        y as f32 + 22.,
        [255; 3],
        0.75,
        &credit,
    );
    y += 40 + 16;
    let score = format!("{:.1}", card.score());
    let score_width = text(
        &mut image,
        &fonts.bold,
        132.,
        left as f32,
        y as f32 + 112.,
        [255; 3],
        1.,
        &score,
    );
    text(
        &mut image,
        &fonts.medium,
        40.,
        left as f32 + score_width + 14.,
        y as f32 + 112.,
        [255; 3],
        0.7,
        "/ 10",
    );
    y += 140;
    let filled = (card.score() / 2.).round().clamp(0., 5.) as u8;
    for star in 0..5u8 {
        draw_star(
            &mut image,
            left as f32 + 22. + f32::from(star) * 56.,
            y as f32 + 26.,
            24.,
            if star < filled { STAR_COLOR } else { [255; 3] },
            if star < filled { 1. } else { 0.22 },
        );
    }
    text(
        &mut image,
        &fonts.medium,
        24.,
        left as f32,
        (height - MARGIN) as f32 - 4.,
        [255; 3],
        0.6,
        "Reviewed in Sonder",
    );

    // Right column: every song.
    let right = MARGIN + COVER + COLUMN_GAP;
    let right_end = WIDTH - MARGIN;
    let stars_width = 5. * 34.;
    let title_x = right as f32 + 52.;
    let title_max = right_end as f32 - stars_width - 28. - title_x;
    let mut row_top = MARGIN;
    for index in 0..rows {
        let height = row_height(index);
        let centre = row_top as f32 + if dense { 24. } else { 31. };
        text(
            &mut image,
            &fonts.medium,
            24.,
            right as f32,
            centre + 9.,
            [255; 3],
            0.55,
            &(index + 1).to_string(),
        );
        let name = truncate(&fonts.semibold, 28., &card.titles[index], title_max);
        text(
            &mut image,
            &fonts.semibold,
            28.,
            title_x,
            centre + 10.,
            [255; 3],
            1.,
            &name,
        );
        let rating = card.ratings.get(index).copied().unwrap_or(0);
        for star in 0..5u8 {
            draw_star(
                &mut image,
                right_end as f32 - stars_width + 17. + f32::from(star) * 34.,
                centre,
                13.,
                if star < rating { STAR_COLOR } else { [255; 3] },
                if star < rating { 1. } else { 0.22 },
            );
        }
        if let Some(comment) = card.comments.get(index).filter(|c| !c.is_empty()) {
            let line = truncate(&fonts.medium, 22., comment, right_end as f32 - title_x);
            text(
                &mut image,
                &fonts.medium,
                22.,
                title_x,
                centre + 44.,
                [255; 3],
                0.75,
                &line,
            );
        }
        if index + 1 < rows {
            let y = row_top + height - 1;
            for x in right..right_end {
                blend(&mut image, x, y, [255; 3], 0.12);
            }
        }
        row_top += height;
    }

    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

use image::ImageEncoder as _;

/// Two colours for the background, from the cover: a vivid one for the top
/// left and a deep one for the bottom right, both dark enough for white text.
fn palette(cover: &RgbaImage) -> ([f32; 3], [f32; 3]) {
    let small = image::imageops::resize(cover, 24, 24, image::imageops::FilterType::Triangle);
    let (mut sum, mut weight) = ([0f32; 3], 0f32);
    for pixel in small.pixels() {
        let [r, g, b, _] = pixel.0;
        let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
        let high = r.max(g).max(b);
        let low = r.min(g).min(b);
        let saturation = if high > 0. { (high - low) / high } else { 0. };
        // Vivid, reasonably bright pixels decide the colour; grey ones barely count.
        let w = 0.05 + saturation * saturation * (high / 255.);
        sum[0] += r * w;
        sum[1] += g * w;
        sum[2] += b * w;
        weight += w;
    }
    let mut base = [sum[0] / weight, sum[1] / weight, sum[2] / weight];
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // Keep the top colour in a range where white text stays readable.
    let target = 105.;
    let current = luma(base).max(1.);
    if current > target {
        let scale = target / current;
        base = base.map(|c| c * scale);
    }
    let bottom = base.map(|c| c * 0.22);
    (base, bottom)
}

fn gradient(image: &mut RgbaImage, top: [f32; 3], bottom: [f32; 3]) {
    let (width, height) = (image.width() as f32, image.height() as f32);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let t = (x as f32 / width * 0.45 + y as f32 / height * 0.55).clamp(0., 1.);
        let mix = |a: f32, b: f32| (a + (b - a) * t).round().clamp(0., 255.) as u8;
        *pixel = Rgba([
            mix(top[0], bottom[0]),
            mix(top[1], bottom[1]),
            mix(top[2], bottom[2]),
            255,
        ]);
    }
}

/// Blends `color` over the pixel at `alpha`; off-canvas points are ignored.
fn blend(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f32) {
    if x >= image.width() || y >= image.height() || alpha <= 0. {
        return;
    }
    let pixel = image.get_pixel_mut(x, y);
    let a = alpha.clamp(0., 1.);
    for (slot, target) in pixel.0.iter_mut().zip(color) {
        let old = f32::from(*slot);
        *slot = (old + (f32::from(target) - old) * a).round() as u8;
    }
}

/// Coverage (0..1) of a pixel centre inside a rounded rectangle, anti-aliased.
fn rounded_coverage(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (x + w / 2., y + h / 2.);
    let (dx, dy) = (
        (px - cx).abs() - (w / 2. - r),
        (py - cy).abs() - (h / 2. - r),
    );
    let outside = (dx.max(0.).powi(2) + dy.max(0.).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.);
    (0.5 - (outside + inside - r)).clamp(0., 1.)
}

#[allow(clippy::too_many_arguments)]
fn rounded(
    image: &mut RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    radius: u32,
    color: [u8; 3],
    alpha: f32,
) {
    for py in y..(y + h).min(image.height()) {
        for px in x..(x + w).min(image.width()) {
            let coverage = rounded_coverage(
                px as f32 + 0.5,
                py as f32 + 0.5,
                x as f32,
                y as f32,
                w as f32,
                h as f32,
                radius as f32,
            );
            blend(image, px, py, color, coverage * alpha);
        }
    }
}

/// The cover, squared, rounded and lifted off the background by a soft shadow.
fn draw_cover(image: &mut RgbaImage, cover: &RgbaImage, x: u32, y: u32, size: u32, radius: u32) {
    // Shadow: a blurred dark rounded square, drawn on a small layer.
    let pad = 56;
    let mut layer = RgbaImage::new(size + pad * 2, size + pad * 2);
    for py in 0..layer.height() {
        for px in 0..layer.width() {
            let coverage = rounded_coverage(
                px as f32 + 0.5,
                py as f32 + 0.5,
                pad as f32,
                pad as f32,
                size as f32,
                size as f32,
                radius as f32,
            );
            layer.put_pixel(px, py, Rgba([0, 0, 0, (coverage * 255.) as u8]));
        }
    }
    let layer = image::imageops::blur(&layer, 20.);
    for py in 0..layer.height() {
        for px in 0..layer.width() {
            let alpha = f32::from(layer.get_pixel(px, py).0[3]) / 255. * 0.55;
            if alpha > 0.003 {
                let (tx, ty) = (x + px, y + py + 18);
                if tx >= pad && ty >= pad {
                    blend(image, tx - pad, ty - pad, [0, 0, 0], alpha);
                }
            }
        }
    }
    // The picture itself: centre-cropped to a square, then scaled.
    let side = cover.width().min(cover.height()).max(1);
    let cropped = image::imageops::crop_imm(
        cover,
        (cover.width() - side) / 2,
        (cover.height() - side) / 2,
        side,
        side,
    )
    .to_image();
    let scaled =
        image::imageops::resize(&cropped, size, size, image::imageops::FilterType::Lanczos3);
    for py in 0..size {
        for px in 0..size {
            let coverage = rounded_coverage(
                px as f32 + 0.5,
                py as f32 + 0.5,
                0.,
                0.,
                size as f32,
                size as f32,
                radius as f32,
            );
            let [r, g, b, _] = scaled.get_pixel(px, py).0;
            blend(image, x + px, y + py, [r, g, b], coverage);
        }
    }
}

/// A five-point star centred at (`cx`, `cy`), filled at `alpha`, 4x4 supersampled.
fn draw_star(image: &mut RgbaImage, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32) {
    let mut points = [(0f32, 0f32); 10];
    for (index, point) in points.iter_mut().enumerate() {
        let angle = -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.;
        let r = if index % 2 == 0 {
            radius
        } else {
            radius * 0.46
        };
        *point = (cx + r * angle.cos(), cy + r * angle.sin());
    }
    let inside = |x: f32, y: f32| {
        let mut crossings = false;
        let mut j = points.len() - 1;
        for i in 0..points.len() {
            let (xi, yi) = points[i];
            let (xj, yj) = points[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                crossings = !crossings;
            }
            j = i;
        }
        crossings
    };
    let (x0, y0) = (
        (cx - radius - 1.).max(0.) as u32,
        (cy - radius - 1.).max(0.) as u32,
    );
    let (x1, y1) = ((cx + radius + 1.) as u32, (cy + radius + 1.) as u32);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    if inside(
                        px as f32 + (sx as f32 + 0.5) / 4.,
                        py as f32 + (sy as f32 + 0.5) / 4.,
                    ) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                blend(image, px, py, color, alpha * hits as f32 / 16.);
            }
        }
    }
}

fn advance(font: &FontRef<'_>, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.;
    let mut previous = None;
    for character in text.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            width += scaled.kern(previous, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

/// Draws one line with its baseline at `baseline`, returning its width.
#[allow(clippy::too_many_arguments)]
fn text(
    image: &mut RgbaImage,
    font: &FontRef<'_>,
    size: f32,
    x: f32,
    baseline: f32,
    color: [u8; 3],
    alpha: f32,
    line: &str,
) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut cursor = x;
    let mut previous = None;
    for character in line.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            cursor += scaled.kern(previous, id);
        }
        let glyph =
            id.with_scale_and_position(PxScale::from(size), ab_glyph::point(cursor, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 {
                    blend(image, px as u32, py as u32, color, coverage * alpha);
                }
            });
        }
        cursor += scaled.h_advance(id);
        previous = Some(id);
    }
    cursor - x
}

/// `text` cut to `max_width` with an ellipsis, if it does not fit.
fn truncate(font: &FontRef<'_>, size: f32, text: &str, max_width: f32) -> String {
    if advance(font, size, text) <= max_width {
        return text.to_owned();
    }
    let mut kept: Vec<char> = text.chars().collect();
    while !kept.is_empty() {
        kept.pop();
        let candidate: String = kept.iter().collect::<String>().trim_end().to_owned() + "…";
        if advance(font, size, &candidate) <= max_width {
            return candidate;
        }
    }
    "…".to_owned()
}

/// Wraps `text` on spaces into at most `max_lines` lines; the last is truncated.
fn wrap(
    font: &FontRef<'_>,
    size: f32,
    text: &str,
    max_width: f32,
    max_lines: usize,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if advance(font, size, &candidate) <= max_width || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_owned();
            if lines.len() + 1 == max_lines {
                // Everything left goes on the last line, to be truncated.
                for rest in words.by_ref() {
                    current.push(' ');
                    current.push_str(rest);
                }
                break;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if let Some(last) = lines.last_mut() {
        *last = truncate(font, size, last, max_width);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(tracks: usize, comments: bool) -> ShareCard {
        ShareCard {
            title: "A very long album title that needs to wrap onto a second line".into(),
            artist: "Sonder Ensemble".into(),
            year: Some("2026".into()),
            ratings: (0..tracks).map(|i| 1 + (i % 5) as u8).collect(),
            titles: (0..tracks)
                .map(|i| format!("Song number {} with a rather long name to cut", i + 1))
                .collect(),
            comments: (0..tracks)
                .map(|i| {
                    if comments && i % 3 == 0 {
                        "That synth drop is the peak of the whole album honestly".into()
                    } else {
                        String::new()
                    }
                })
                .collect(),
            cover: Some(RgbaImage::from_pixel(640, 640, Rgba([40, 90, 220, 255]))),
        }
    }

    fn decode(bytes: &[u8]) -> RgbaImage {
        image::load_from_memory(bytes).unwrap().to_rgba8()
    }

    #[test]
    fn renders_a_wide_png_taller_for_longer_albums() {
        let short = decode(&render(&card(8, false)).unwrap());
        let long = decode(&render(&card(30, true)).unwrap());
        assert_eq!(short.width(), WIDTH);
        assert_eq!(long.width(), WIDTH);
        assert!(long.height() > short.height());
        assert!(short.height() > 700);
    }

    #[test]
    fn the_background_comes_from_the_cover_and_text_is_drawn() {
        let image = decode(&render(&card(5, true)).unwrap());
        // Top-left corner: the blue cover's tint, not a default grey.
        let [r, _, b, _] = image.get_pixel(4, 4).0;
        assert!(
            b > r + 40,
            "expected a blue background, got {:?}",
            image.get_pixel(4, 4)
        );
        // Something light was drawn over the dark background in the track column.
        let bright = (600..1500)
            .flat_map(|x| (72..300).map(move |y| (x, y)))
            .filter(|(x, y)| image.get_pixel(*x, *y).0[0] > 200)
            .count();
        assert!(bright > 500, "no text found in the track column");
    }

    #[test]
    fn renders_without_a_cover_and_for_very_long_albums() {
        let mut plain = card(3, false);
        plain.cover = None;
        decode(&render(&plain).unwrap());
        let huge = decode(&render(&card(200, true)).unwrap());
        // Capped, so the image stays a sane size.
        assert!(huge.height() < 6000);
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis_and_titles_wrap() {
        let fonts = Fonts::load().unwrap();
        let cut = truncate(
            &fonts.semibold,
            28.,
            "An extraordinarily long song title indeed",
            200.,
        );
        assert!(cut.ends_with('…'));
        assert!(advance(&fonts.semibold, 28., &cut) <= 200.);
        assert_eq!(truncate(&fonts.semibold, 28., "Short", 200.), "Short");
        let lines = wrap(
            &fonts.semibold,
            46.,
            "One two three four five six seven eight nine",
            300.,
            2,
        );
        assert_eq!(lines.len(), 2);
    }

    #[test]
    #[ignore = "writes a sample for looking at"]
    fn write_sample() {
        let mut sample = card(10, true);
        sample.title = "Blue hours".into();
        let mut cover = RgbaImage::new(640, 640);
        for (x, y, p) in cover.enumerate_pixels_mut() {
            *p = Rgba([
                (210. - y as f32 * 0.2) as u8,
                (110. + x as f32 * 0.1) as u8,
                70,
                255,
            ]);
        }
        sample.cover = Some(cover);
        std::fs::write(
            std::env::var("SAMPLE_OUT").unwrap(),
            render(&sample).unwrap(),
        )
        .unwrap();
    }
}
