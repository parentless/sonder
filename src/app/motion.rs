//! Sonder's shared motion settings, on top of `gpui-motion-kit`.
//!
//! A view keeps one `gpui_motion_kit::ValueState` (or `MotionState`) per moving
//! thing, asks it for a frame each render with the destination and a
//! transition from here, and the kit requests further frames only while the
//! value is still moving, so a resting window stays idle. Retargeting keeps
//! the current velocity, so reversing mid-flight never jumps, and reduced
//! motion snaps to the destination.
use std::time::Duration;

use gpui_motion_kit::core::{Spring, Transition};

/// How long the sidebar rail takes to settle.
const SIDEBAR_SETTLE: Duration = Duration::from_millis(200);

/// The sidebar rail: a spring with no bounce that settles in about 200ms.
pub(super) fn sidebar_spring() -> Transition {
    Transition::Spring(Spring::visual(SIDEBAR_SETTLE, 0.).unwrap_or_default())
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
}
