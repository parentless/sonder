//! The first-launch intro: a glow sweeps in from the corner of the window,
//! lands on the Sonder icon, which is selected and pressed, and the icon opens
//! into the app.
//!
//! It plays once, ever: the first time the main window opens on an install
//! that has not seen it, new or updated (see `Store::launch_intro_pending`). A
//! click or Escape skips it, and with reduced motion it is not shown at all.
//!
//! Every frame is a pure function of the elapsed time ([`frame`]), built from a
//! few dozen plain elements. Nothing runs once it ends, and the overlay is
//! removed from the window then.
use super::*;
use gpui_kit::TestSupportExt as _;

/// The whole sequence. Events inside it are in milliseconds from the start.
pub(super) const DURATION: Duration = Duration::from_millis(2300);
/// The glow reaches the icon, then bursts into it.
const TRAVEL_MS: f32 = 950.;
const BURST_MS: f32 = 320.;
/// When the icon starts opening into the app, and how long that takes.
const OPEN_START_MS: f32 = 1200.;
const OPEN_MS: f32 = 720.;
/// The glow that settles out of the window corner at the end.
const EDGE_START_MS: f32 = 1580.;
const EDGE_MS: f32 = 720.;

/// The sign-in-free dark of the veil and the icon tile (the app icon's colours).
const VEIL: u32 = 0x101216;
const TILE: u32 = 0x0A0B0D;
const CREAM: u32 = 0xF7F5EF;
/// The glow's cool halo.
const HALO: u32 = 0xB0BEFF;
const HALO_DEEP: u32 = 0x7886FF;

/// The icon tile's size, in the design's 16px rems.
const TILE_SIZE: f32 = 140.;

pub(super) struct IntroDone;

pub(super) struct LaunchIntro {
    /// A fixed moment instead of a running animation, for screenshots.
    frozen: Option<f32>,
    _timer: Option<gpui_kit::Task<()>>,
}

impl EventEmitter<IntroDone> for LaunchIntro {}

impl LaunchIntro {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DURATION).await;
            this.update(cx, |_, cx| cx.emit(IntroDone)).ok();
        });
        Self {
            frozen: None,
            _timer: Some(timer),
        }
    }

    /// The intro held at `progress` (0 to 1) of its length.
    #[cfg(test)]
    pub(super) fn frozen(progress: f32) -> Self {
        Self {
            frozen: Some(progress),
            _timer: None,
        }
    }

    pub(super) fn skip(&mut self, cx: &mut Context<Self>) {
        cx.emit(IntroDone);
    }
}

impl Render for LaunchIntro {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let scale = f32::from(window.rem_size()) / 16.;
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        let overlay = div()
            .id("launch-intro")
            .test_support()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            // Nothing behind it can be clicked while it plays; a click skips it.
            .occlude()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.skip(cx)),
            );
        match self.frozen {
            Some(progress) => overlay
                .child(frame(
                    progress * DURATION.as_secs_f32() * 1000.,
                    width,
                    height,
                    scale,
                ))
                .into_any_element(),
            None => overlay
                .with_animation(
                    "launch-intro-frames",
                    Animation::new(DURATION),
                    move |overlay, progress| {
                        overlay.child(frame(
                            progress * DURATION.as_secs_f32() * 1000.,
                            width,
                            height,
                            scale,
                        ))
                    },
                )
                .into_any_element(),
        }
    }
}

fn color(hex: u32, alpha: f32) -> gpui_kit::Hsla {
    let mut color: gpui_kit::Hsla = rgb(hex).into();
    color.a = alpha.clamp(0., 1.);
    color
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0., 1.)
}

/// The CSS `cubic-bezier(x1, y1, x2, y2)` curve, solved for `x` by bisection.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = clamp01(x);
    let axis = |a: f32, b: f32, t: f32| {
        let u = 1. - t;
        3. * u * u * t * a + 3. * u * t * t * b + t * t * t
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if axis(x1, x2, mid) < x {
            low = mid;
        } else {
            high = mid;
        }
    }
    axis(y1, y2, (low + high) / 2.)
}

/// Strong ease-out: quick to respond, long settle.
fn ease_out(x: f32) -> f32 {
    bezier(0.23, 1., 0.32, 1., x)
}

/// Strong ease-in-out for the icon opening into the window.
fn ease_in_out(x: f32) -> f32 {
    bezier(0.77, 0., 0.175, 1., x)
}

/// Where the glow is, as a fraction of the window, after `u` of its travel: it
/// swoops in from beyond the top-left corner and lands on the icon.
fn glow_position(u: f32, target: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    let start = (-0.1 * size.0, -0.156 * size.1);
    let bend = (0.625 * size.0, -0.02 * size.1);
    let eased = ((1. - (std::f32::consts::PI * u).cos()) / 2.).powf(0.9);
    let (a, b, c) = (
        (1. - eased).powi(2),
        2. * (1. - eased) * eased,
        eased * eased,
    );
    (
        a * start.0 + b * bend.0 + c * target.0,
        a * start.1 + b * bend.1 + c * target.1,
    )
}

/// The geometry of one frame, shared by the pieces below.
struct Geometry {
    /// Pixels per design pixel (follows UI zoom).
    scale: f32,
    width: f32,
    height: f32,
    /// The icon tile's centre and size.
    centre: (f32, f32),
    tile: f32,
}

impl Geometry {
    fn new(width: f32, height: f32, scale: f32) -> Self {
        Self {
            scale,
            width,
            height,
            // The icon and its label sit together in the middle of the window.
            centre: (width / 2., height / 2. - 19. * scale),
            tile: TILE_SIZE * scale,
        }
    }
}

/// One frame of the intro at `ms` into the sequence.
fn frame(ms: f32, width: f32, height: f32, scale: f32) -> impl IntoElement {
    let g = Geometry::new(width, height, scale);
    let open = ease_in_out(clamp01((ms - OPEN_START_MS) / OPEN_MS));
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(veil(&g, open))
        .child(icon(&g, ms))
        .children(waves(&g, ms))
        .children(orbs(&g, ms))
        .child(edge_glow(&g, ms))
}

/// The dark that covers the window, with a hole that starts as the icon and
/// grows to the whole window. Drawn as one very thick rounded border around
/// the hole, so the app behind shows through it as it opens.
fn veil(g: &Geometry, open: f32) -> AnyElement {
    if open >= 1. {
        return div().into_any_element();
    }
    let hole_width = lerp(g.tile, g.width, open);
    let hole_height = lerp(g.tile, g.height, open);
    let hole_left = lerp(g.centre.0 - g.tile / 2., 0., open);
    let hole_top = lerp(g.centre.1 - g.tile / 2., 0., open);
    let radius = lerp(0.2266 * g.tile, 0., open);
    let thickness = g.width.max(g.height) + g.tile;
    div()
        .absolute()
        .left(px(hole_left - thickness))
        .top(px(hole_top - thickness))
        .w(px(hole_width + 2. * thickness))
        .h(px(hole_height + 2. * thickness))
        .rounded(px(radius + thickness))
        .border(px(thickness))
        .border_color(color(VEIL, 1.))
        .into_any_element()
}

/// The icon, its label and the highlight that selects it.
fn icon(g: &Geometry, ms: f32) -> AnyElement {
    let appear = ease_out(clamp01(ms / 320.));
    let leaving = ease_out(clamp01((ms - 1250.) / 420.));
    let visible = appear * (1. - leaving);
    if visible <= 0.001 {
        return div().into_any_element();
    }
    // A press: down to 93% and back.
    let press = clamp01((ms - TRAVEL_MS) / 300.);
    let pressed = if press < 0.4 {
        lerp(1., 0.93, ease_out(press / 0.4))
    } else {
        lerp(0.93, 1., ease_out((press - 0.4) / 0.6))
    };
    let size = g.tile * pressed * lerp(0.96, 1., appear) * lerp(1., 1.35, leaving);
    let (cx, cy) = g.centre;
    let selected = ease_out(clamp01((ms - (TRAVEL_MS - 20.)) / 140.));
    let glow = {
        // The tile flares as it is clicked, then settles to a soft halo.
        let p = clamp01((ms - TRAVEL_MS) / 520.);
        if p < 0.4 {
            lerp(0., 1., ease_out(p / 0.4))
        } else {
            lerp(1., 0.45, ease_out((p - 0.4) / 0.6))
        }
    };
    // Rings light up outward from the centre once clicked.
    let lit = |delay: f32| {
        let p = clamp01((ms - TRAVEL_MS - delay) / 420.);
        lerp(0.35, 1., ease_out(clamp01(p / 0.35)))
    };
    let unit = size / 1024.;
    let ring = |centre_radius: f32, alpha: f32, delay: f32| {
        let diameter = (2. * centre_radius + 46.) * unit;
        div()
            .absolute()
            .left(px((size - diameter) / 2.))
            .top(px((size - diameter) / 2.))
            .size(px(diameter))
            .rounded_full()
            .border(px(46. * unit))
            .border_color(color(CREAM, alpha * lit(delay)))
    };
    let dot = 112. * unit;
    let tile = div()
        .absolute()
        .left(px(cx - size / 2.))
        .top(px(cy - size / 2.))
        .size(px(size))
        .rounded(px(0.2266 * size))
        .bg(color(TILE, 1.))
        .border_1()
        .border_color(color(0xFFFFFF, 0.1))
        .shadow(vec![
            gpui_kit::BoxShadow::new(px(0.), px(10. * g.scale), color(0x000000, 0.55))
                .blur_radius(px(22. * g.scale)),
            gpui_kit::BoxShadow::new(px(0.), px(0.), color(CREAM, 0.5 * glow))
                .blur_radius(px(46. * g.scale)),
        ])
        .child(ring(331., 0.3, 210.))
        .child(ring(235., 0.55, 140.))
        .child(ring(138., 0.85, 70.))
        .child(
            div()
                .absolute()
                .left(px((size - dot) / 2.))
                .top(px((size - dot) / 2.))
                .size(px(dot))
                .rounded_full()
                .bg(color(CREAM, lit(0.))),
        );
    let label_top = cy + g.tile / 2. + 14. * g.scale;
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .opacity(visible)
        .child(
            // The highlight a desktop gives a selected icon.
            div()
                .absolute()
                .left(px(cx - 88. * g.scale))
                .top(px(cy - g.tile / 2. - 12. * g.scale))
                .w(px(176. * g.scale))
                .h(px(200. * g.scale))
                .rounded(px(26. * g.scale))
                .bg(color(0xFFFFFF, 0.1 * selected))
                .border_1()
                .border_color(color(0xFFFFFF, 0.16 * selected)),
        )
        .child(tile)
        .child(
            div()
                .absolute()
                .left(px(cx - 60. * g.scale))
                .top(px(label_top))
                .w(px(120. * g.scale))
                .flex()
                .justify_center()
                .text_size(px(22. * g.scale))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(color(CREAM, 1.))
                .child("Sonder"),
        )
        .into_any_element()
}

/// Two rings that spread out from the icon when it is clicked.
fn waves(g: &Geometry, ms: f32) -> Vec<AnyElement> {
    [0., 140.]
        .into_iter()
        .filter_map(|delay| {
            let p = clamp01((ms - TRAVEL_MS - delay) / 760.);
            if p <= 0. || p >= 1. {
                return None;
            }
            let e = ease_out(p);
            let size = g.tile * lerp(1., 5.2, e);
            Some(
                div()
                    .absolute()
                    .left(px(g.centre.0 - size / 2.))
                    .top(px(g.centre.1 - size / 2.))
                    .size(px(size))
                    .rounded_full()
                    .border(px(3. * g.scale))
                    .border_color(color(CREAM, 0.45 * (1. - e)))
                    .into_any_element(),
            )
        })
        .collect()
}

/// The glow and its two fainter trails: in from the top-left corner, onto the
/// icon, then absorbed into it.
fn orbs(g: &Geometry, ms: f32) -> Vec<AnyElement> {
    [
        (0., 1., 0.95),
        (40., 0.8, 0.6),
        (90., 0.62, 0.4),
        (150., 0.45, 0.25),
    ]
    .into_iter()
    .filter_map(|(delay, size_factor, strength)| {
        let t = ms - delay;
        if t <= 0. || t >= TRAVEL_MS + BURST_MS {
            return None;
        }
        let u = clamp01(t / TRAVEL_MS);
        let (x, y) = glow_position(u, g.centre, (g.width, g.height));
        let (scale, strength) = if t <= TRAVEL_MS {
            (0.55 + 0.45 * size_factor * u, strength * clamp01(u * 8.))
        } else {
            let burst = (t - TRAVEL_MS) / BURST_MS;
            (
                lerp(size_factor, 1.5 * size_factor, burst),
                strength * (1. - burst),
            )
        };
        let core = 30. * g.scale * scale;
        Some(
            div()
                .absolute()
                .left(px(x - core / 2.))
                .top(px(y - core / 2.))
                .size(px(core))
                .rounded_full()
                .bg(color(0xFFFDF6, 0.95 * strength))
                .shadow(vec![
                    gpui_kit::BoxShadow::new(px(0.), px(0.), color(CREAM, 0.6 * strength))
                        .blur_radius(px(34. * g.scale * scale))
                        .spread_radius(px(8. * g.scale * scale)),
                    gpui_kit::BoxShadow::new(px(0.), px(0.), color(HALO, 0.55 * strength))
                        .blur_radius(px(110. * g.scale * scale))
                        .spread_radius(px(36. * g.scale * scale)),
                    gpui_kit::BoxShadow::new(px(0.), px(0.), color(HALO_DEEP, 0.28 * strength))
                        .blur_radius(px(240. * g.scale * scale))
                        .spread_radius(px(70. * g.scale * scale)),
                ])
                .into_any_element(),
        )
    })
    .collect()
}

/// The last of the glow, fading out of the corner it arrived from.
fn edge_glow(g: &Geometry, ms: f32) -> AnyElement {
    let p = clamp01((ms - EDGE_START_MS) / EDGE_MS);
    if p <= 0. || p >= 1. {
        return div().into_any_element();
    }
    let strength = if p < 0.3 {
        lerp(0., 0.9, ease_out(p / 0.3))
    } else {
        lerp(0.9, 0., ease_out((p - 0.3) / 0.7))
    };
    div()
        .absolute()
        .left(px(-20. * g.scale))
        .top(px(-20. * g.scale))
        .size(px(40. * g.scale))
        .rounded_full()
        .shadow(vec![
            gpui_kit::BoxShadow::new(px(0.), px(0.), color(HALO, 0.45 * strength))
                .blur_radius(px(260. * g.scale))
                .spread_radius(px(110. * g.scale)),
            gpui_kit::BoxShadow::new(px(0.), px(0.), color(HALO_DEEP, 0.3 * strength))
                .blur_radius(px(420. * g.scale))
                .spread_radius(px(160. * g.scale)),
        ])
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curves_start_at_zero_and_land_on_one() {
        for curve in [ease_out, ease_in_out] {
            assert!(curve(0.).abs() < 1e-3);
            assert!((curve(1.) - 1.).abs() < 1e-3);
        }
        // Ease-out leads: well past halfway at the halfway point.
        assert!(ease_out(0.5) > 0.8);
        // Ease-in-out starts slowly and finishes ahead of a straight line.
        assert!(ease_in_out(0.25) < 0.25);
        assert!(ease_in_out(0.75) > 0.75);
    }

    #[test]
    fn the_glow_starts_beyond_the_corner_and_lands_on_the_icon() {
        let size = (1600., 1025.);
        let target = (800., 493.);
        let (x, y) = glow_position(0., target, size);
        assert!(x < 0. && y < 0.);
        let (x, y) = glow_position(1., target, size);
        assert!((x - target.0).abs() < 0.5 && (y - target.1).abs() < 0.5);
    }

    #[test]
    fn the_sequence_fits_its_duration() {
        let end = DURATION.as_secs_f32() * 1000.;
        assert!(TRAVEL_MS + BURST_MS <= end);
        assert!(OPEN_START_MS + OPEN_MS <= end);
        assert!(EDGE_START_MS + EDGE_MS <= end);
    }
}
