use gpui_kit::{App, Result};
use std::borrow::Cow;

pub(super) const INTER_VARIABLE: &[u8] = include_bytes!("../../assets/fonts/Inter-Variable.ttf");
pub(super) const INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
pub(super) const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
pub(super) const INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-Bold.ttf");

/// Register the same pinned family in production and native previews.
/// Bytes are embedded once; no filesystem reads or per-frame font loading.
pub(super) fn register(cx: &App) -> Result<()> {
    cx.text_system().add_fonts(vec![
        Cow::Borrowed(INTER_VARIABLE),
        Cow::Borrowed(INTER_MEDIUM),
        Cow::Borrowed(INTER_SEMIBOLD),
        Cow::Borrowed(INTER_BOLD),
    ])
}
