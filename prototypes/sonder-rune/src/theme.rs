//! Colours, from Sonder's own palette: a near-black sidebar and a slightly lighter page.

use rune::Color;

pub fn c(hex: &str) -> Color {
    rune::color::hex(hex).expect("a valid colour")
}

pub struct Theme {
    pub side: Color,
    pub page: Color,
    pub bar: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub hover: Color,
    pub active: Color,
    pub pill: Color,
    pub pill_text: Color,
    pub tile: Color,
    pub heart: Color,
    pub separator: Color,
}

pub fn dark() -> Theme {
    Theme {
        side: c("#121212"),
        page: c("#1a1a1a"),
        bar: c("#121212"),
        text: c("#f3f3f3"),
        muted: c("#a0a0a5"),
        faint: c("#6f6f75"),
        hover: Color::WHITE.alpha(0.06),
        active: Color::WHITE.alpha(0.10),
        pill: c("#2c2c2f"),
        pill_text: c("#ffffff"),
        tile: c("#2c2c2f"),
        heart: c("#f07aa8"),
        separator: Color::WHITE.alpha(0.08),
    }
}

/// A calm tint for the glow behind a page header, chosen from its title so each
/// page keeps the same colour.
pub fn tint(seed: &str) -> Color {
    const TINTS: [&str; 6] = [
        "#4a6fd4", "#8a5ad0", "#d0587a", "#3fa28c", "#d08a3f", "#5a9ad0",
    ];
    let hash = seed
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    c(TINTS[hash as usize % TINTS.len()])
}

/// Sonder's type: Inter, with its weights (titles and body semibold, secondary medium).
pub mod type_ {
    use rune::tok::TextStyle;
    pub const BODY: TextStyle = TextStyle::new(13.0, 600.0);
    pub const LABEL: TextStyle = TextStyle::new(12.0, 500.0);
    pub const TITLE: TextStyle = TextStyle::new(13.0, 700.0);
}
