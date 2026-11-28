//! The listening animation: a small floating window with a ring of dots that
//! swells with the listener's voice, falls into a point when the key is let
//! go, and then shows what was heard and what is playing.
//!
//! It is a window of its own, above everything and never taking focus, so it
//! shows over a game without pulling the game out of the foreground. It exists
//! only from the key press until a couple of seconds after the result, and
//! only when the listener turned it on in Settings. While it is open it draws
//! a frame at a time; at rest nothing runs.

use std::time::{Duration, Instant};

use super::*;
use voice_play::Stage;

/// The window's size. Everything fits inside the ring at its largest.
pub(super) const SIZE: (f32, f32) = (300., 300.);
/// How far above the bottom of the screen it sits.
const BOTTOM_MARGIN: f32 = 96.;
/// How long the result stays before the window fades.
pub(super) const RESULT_SHOWN: Duration = Duration::from_millis(2600);
/// The fade at either end.
const FADE: Duration = Duration::from_millis(220);
/// When the window is closed, counted from the result.
pub(super) const CLOSE_AFTER: Duration = Duration::from_millis(2900);

const DOTS: usize = 48;
const RING: f32 = 52.;
const SWELL: f32 = 34.;
const CENTER: (f32, f32) = (150., 100.);
const GREEN: u32 = 0x2fd36a;
/// The panel behind everything, so it reads over any picture.
const PANEL: u32 = 0x0b0c10;
const PANEL_ALPHA: f32 = 0.84;

pub(super) struct VoiceOverlay {
    voice: Entity<voice_play::VoicePlay>,
    opened: Instant,
    _redraw: Subscription,
}

impl VoiceOverlay {
    pub(super) fn new(voice: Entity<voice_play::VoicePlay>, cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe(&voice, |_, _, cx| cx.notify());
        Self {
            voice,
            opened: Instant::now(),
            _redraw: redraw,
        }
    }
}

/// Where the window goes: centred, near the bottom of the main screen.
pub(super) fn bounds(cx: &App) -> Bounds<Pixels> {
    let (width, height) = SIZE;
    let area = cx
        .primary_display()
        .map(|display| display.bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(800.))));
    Bounds::new(
        point(
            area.origin.x + (area.size.width - px(width)) / 2.,
            area.origin.y + area.size.height - px(height) - px(BOTTOM_MARGIN),
        ),
        size(px(width), px(height)),
    )
}

/// Makes the window ignore the mouse and never take the keyboard, so a game
/// underneath keeps both. Windows only.
#[cfg(target_os = "windows")]
pub(super) fn leave_input_alone(window: &Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
    };
    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the handle belongs to this live window, on its own thread.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            style | (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT) as isize,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn leave_input_alone(_window: &Window) {}

fn ease_out(x: f32) -> f32 {
    1. - (1. - x.clamp(0., 1.)).powi(3)
}

/// How far the ring has fallen into the centre, a moment after release.
fn collapse_of(released: Instant, still: bool) -> f32 {
    if still {
        1.
    } else {
        ease_out(seconds(released) / 0.45)
    }
}

fn seconds(since: Instant) -> f32 {
    since.elapsed().as_secs_f32()
}

/// A dot's place and look at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Dot {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub alpha: f32,
}

/// Where dot `index` is. `level` is how loud the listener is (0 to 1) and
/// `collapse` how far the ring has fallen into the centre (0 to 1).
pub(super) fn dot(index: usize, time: f32, level: f32, collapse: f32) -> Dot {
    let angle = index as f32 / DOTS as f32 * std::f32::consts::TAU + time * 0.25;
    // Each dot answers a little differently, so the ring ripples.
    let ripple = 0.55 + 0.45 * (index as f32 * 0.9 + time * 9.).sin();
    let push = (level * ripple).clamp(0., 1.);
    let radius = (RING + push * SWELL) * (1. - collapse) + 6. * collapse;
    Dot {
        x: CENTER.0 + angle.cos() * radius,
        y: CENTER.1 + angle.sin() * radius,
        size: 3.4 + push * 4.,
        alpha: (0.35 + push * 0.65) * (1. - collapse),
    }
}

impl Render for VoiceOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (stage, level) = {
            let voice = self.voice.read(cx);
            (voice.stage().clone(), voice.level())
        };
        let still = cx.reduce_motion();
        if !still {
            window.request_animation_frame();
        }
        let time = seconds(self.opened);
        let mut fade = ease_out(time / FADE.as_secs_f32());
        // What is drawn: the ring, how collapsed it is, the core and its pulse.
        let mut collapse = 0.;
        let mut ring_level = 0.;
        let mut core = 7.;
        let mut core_color = GREEN;
        let mut pulse: Option<(f32, f32)> = None;
        let mut heard: Option<(SharedString, Instant)> = None;
        let mut found: Option<(SharedString, SharedString, &'static str, Instant)> = None;
        let mut failure: Option<(SharedString, Instant)> = None;
        match &stage {
            Stage::Hidden => fade = 0.,
            Stage::Listening => {
                ring_level = if still { 0. } else { level };
                core = 6. + level * 6.;
            }
            Stage::Working {
                released,
                heard: text,
            } => {
                collapse = collapse_of(*released, still);
                core = 8.;
                heard = text.clone();
            }
            Stage::Found {
                released,
                heard: text,
                title,
                artist,
                source,
                at,
            } => {
                collapse = collapse_of(*released, still);
                core = 8.;
                let age = seconds(*at);
                if !still {
                    pulse = Some((10. + ease_out(age / 0.9) * 46., (0.55 - age * 0.3).max(0.)));
                }
                heard = text.clone();
                found = Some((title.clone(), artist.clone(), source, *at));
            }
            Stage::Failed {
                released,
                message,
                heard: text,
                at,
            } => {
                collapse = collapse_of(*released, still);
                core = 7.;
                core_color = 0x8a8d98;
                heard = text.clone();
                failure = Some((message.clone(), *at));
            }
        }
        // The result stays a moment, then the whole window fades.
        let result_age = match &stage {
            Stage::Found { at, .. } | Stage::Failed { at, .. } => Some(seconds(*at)),
            _ => None,
        };
        if let Some(age) = result_age {
            let out = (age - RESULT_SHOWN.as_secs_f32()) / FADE.as_secs_f32();
            fade *= 1. - out.clamp(0., 1.);
        }

        let dot_color =
            |alpha: f32| gpui_kit::rgba((0xffffff00u32) | (alpha.clamp(0., 1.) * 255.) as u32);
        let mut root = div()
            .relative()
            .size_full()
            .opacity(fade)
            .rounded(px(26.))
            .bg(gpui_kit::rgba((PANEL << 8) | (PANEL_ALPHA * 255.) as u32))
            .overflow_hidden();
        if !still || collapse < 1. {
            for index in 0..DOTS {
                let d = dot(index, if still { 0. } else { time }, ring_level, collapse);
                if d.alpha <= 0.01 {
                    continue;
                }
                root = root.child(
                    div()
                        .absolute()
                        .left(px(d.x - d.size / 2.))
                        .top(px(d.y - d.size / 2.))
                        .size(px(d.size))
                        .rounded_full()
                        .bg(dot_color(d.alpha)),
                );
            }
        }
        if let Some((radius, alpha)) = pulse.filter(|(_, alpha)| *alpha > 0.01) {
            root = root.child(
                div()
                    .absolute()
                    .left(px(CENTER.0 - radius))
                    .top(px(CENTER.1 - radius))
                    .size(px(radius * 2.))
                    .rounded_full()
                    .border_1()
                    .border_color(gpui_kit::rgba(
                        (GREEN << 8) | (alpha.clamp(0., 1.) * 255.) as u32,
                    )),
            );
        }
        root = root.child(
            div()
                .absolute()
                .left(px(CENTER.0 - core))
                .top(px(CENTER.1 - core))
                .size(px(core * 2.))
                .rounded_full()
                .bg(rgb(core_color)),
        );

        // Words, a line of what was heard, appearing one after another.
        let text_top = CENTER.1 + RING + SWELL + 22.;
        if let Some((text, since)) = heard {
            let words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
            let age = seconds(since);
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(text_top))
                    .px_4()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_x(px(5.))
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(rgb(0xffffff))
                    .children(words.into_iter().enumerate().map(|(index, word)| {
                        let shown = if still {
                            1.
                        } else {
                            ease_out((age - index as f32 * 0.16) / 0.25)
                        };
                        div().opacity(shown).child(word)
                    })),
            );
        }
        if let Some((title, artist, source, at)) = found {
            let shown = if still {
                1.
            } else {
                ease_out((seconds(at) - 0.12) / 0.35)
            };
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(text_top + 26.))
                    .px_4()
                    .opacity(shown)
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .max_w_full()
                            .text_size(px(14.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xffffff))
                            .text_ellipsis()
                            .line_clamp(1)
                            .child(title),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .text_size(px(12.))
                            .text_color(rgb(0x9a9eab))
                            .text_ellipsis()
                            .line_clamp(1)
                            .child(artist),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(10.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(rgb(GREEN))
                            .child(source),
                    ),
            );
        }
        if let Some((message, at)) = failure {
            let shown = if still {
                1.
            } else {
                ease_out((seconds(at) - 0.1) / 0.3)
            };
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(text_top + 26.))
                    .px_5()
                    .opacity(shown)
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(0xc4c7d0))
                    .text_center()
                    .child(message),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_louder_voice_pushes_the_ring_outward() {
        let radius = |level: f32| {
            let d = dot(0, 0., level, 0.);
            ((d.x - CENTER.0).powi(2) + (d.y - CENTER.1).powi(2)).sqrt()
        };
        assert!(radius(1.) > radius(0.) + 10.);
        assert!((radius(0.) - RING).abs() < 0.01);
    }

    #[test]
    fn the_ring_falls_into_the_centre_and_fades() {
        let d = dot(5, 1., 0.5, 1.);
        assert!(((d.x - CENTER.0).powi(2) + (d.y - CENTER.1).powi(2)).sqrt() < 6.1);
        assert_eq!(d.alpha, 0.);
        assert!(dot(5, 1., 0.5, 0.).alpha > 0.3);
    }

    #[test]
    fn the_ring_fits_inside_the_window() {
        for index in 0..DOTS {
            let d = dot(index, 3.7, 1., 0.);
            assert!(d.x - d.size / 2. >= 0. && d.x + d.size / 2. <= SIZE.0);
            assert!(d.y - d.size / 2. >= 0. && d.y + d.size / 2. <= SIZE.1);
        }
    }

    #[test]
    fn the_window_closes_after_the_result_has_faded() {
        assert!(CLOSE_AFTER > RESULT_SHOWN + FADE);
    }
}
