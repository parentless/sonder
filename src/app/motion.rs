//! Sonder's shared motion settings, on top of `gpui-motion-kit`.
//!
//! A view keeps one `gpui_motion_kit::ValueState` (or `MotionState`) per moving
//! thing, asks it for a frame each render with the destination and a
//! transition from here, and the kit requests further frames only while the
//! value is still moving, so a resting window stays idle. Retargeting keeps
//! the current velocity, so reversing mid-flight never jumps, and reduced
//! motion snaps to the destination.
use std::time::Duration;

use gpui_motion_kit::core::{Bezier, Easing, Spring, Transition};

/// How long the sidebar rail takes to settle.
const SIDEBAR_SETTLE: Duration = Duration::from_millis(200);

/// The sidebar rail: a spring with no bounce that settles in about 200ms.
pub(super) fn sidebar_spring() -> Transition {
    Transition::Spring(Spring::visual(SIDEBAR_SETTLE, 0.).unwrap_or_default())
}

/// A menu opening from its button: a short tween on a strong ease-out, so it
/// answers the click at once. Closing is quicker still, because by then the
/// listener has already moved on.
pub(super) fn popover(opening: bool) -> Transition {
    let duration = if opening { 150 } else { 100 };
    match Bezier::new(0.23, 1., 0.32, 1.) {
        Ok(curve) => Transition::Tween(Duration::from_millis(duration), Easing::Bezier(curve)),
        Err(_) => Transition::Tween(Duration::from_millis(duration), Easing::EaseOut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_motion_kit::core::MotionValue;

    /// Samples a spring from 0 toward 1 every 16ms and returns the values.
    fn run(frames: u32) -> Vec<f64> {
        let mut value = MotionValue::new(0.);
        value.animate(1., sidebar_spring(), Duration::ZERO);
        (0..=frames)
            .map(|frame| {
                value
                    .sample(Duration::from_millis(16) * frame, false)
                    .value()
            })
            .collect()
    }

    #[test]
    fn the_sidebar_spring_settles_on_one_without_overshooting() {
        let values = run(60);
        assert!(values.iter().all(|v| *v <= 1.0001), "overshot: {values:?}");
        assert!(*values.last().unwrap() > 0.9999);
    }

    #[test]
    fn the_sidebar_spring_reaches_90_percent_by_its_settle_time() {
        // The kit's `Spring::visual` is the time of first arrival at 90%.
        // 13 frames of 16ms is 208ms.
        assert!(run(13)[13] > 0.9);
    }

    #[test]
    fn reduced_motion_snaps_the_sidebar() {
        let mut value = MotionValue::new(0.);
        value.animate(1., sidebar_spring(), Duration::ZERO);
        assert_eq!(value.sample(Duration::ZERO, true).value(), 1.);
    }

    #[test]
    fn a_menu_opens_in_150ms_and_closes_in_100ms_without_overshooting() {
        for (opening, from, to, ms) in [(true, 0., 1., 150u64), (false, 1., 0., 100)] {
            let mut value = MotionValue::new(from);
            value.animate(to, popover(opening), Duration::ZERO);
            for step in 0..=ms {
                let v = value.sample(Duration::from_millis(step), false).value();
                assert!((-0.0001..=1.0001).contains(&v), "overshot at {step}ms: {v}");
            }
            let end = value.sample(Duration::from_millis(ms), false).value();
            assert!((end - to).abs() < 0.001, "not settled at {ms}ms: {end}");
        }
    }
}
