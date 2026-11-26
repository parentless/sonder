//! Sonder's shared motion: one small spring for anything that should follow
//! the listener's last action smoothly and can be redirected mid-flight.
//!
//! It is a critically damped spring (no overshoot) stepped from the frame
//! clock, so a view keeps one [`Spring`], asks it for its value each render
//! and requests another frame only while it is still moving. A resting window
//! stays idle. Retargeting keeps the current velocity, so toggling twice never
//! jumps. Reduced motion snaps straight to the target.
use std::time::{Duration, Instant};

/// Closer than this to the target, and slower than [`REST_SPEED`], is "arrived".
const REST_DISTANCE: f32 = 0.0005;
const REST_SPEED: f32 = 0.005;
/// A long frame (a stall, a breakpoint) must not fling the spring.
const MAX_STEP: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(super) struct Spring {
    value: f32,
    velocity: f32,
    last: Option<Instant>,
    /// Angular frequency: settles within about 1% in `6.64 / omega` seconds.
    omega: f32,
}

/// Where the spring is after a step.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sample {
    pub value: f32,
    /// More frames are needed to reach the target.
    pub moving: bool,
}

impl Spring {
    /// A spring at rest on `value` that settles in about `settle`.
    pub(super) fn new(value: f32, settle: Duration) -> Self {
        Self {
            value,
            velocity: 0.,
            last: None,
            omega: 6.64 / settle.as_secs_f32().max(0.01),
        }
    }

    /// Moves toward `target` as of `now` and reports where it stands.
    pub(super) fn step(&mut self, target: f32, now: Instant, reduce_motion: bool) -> Sample {
        let dt = self
            .last
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last))
            .min(MAX_STEP)
            .as_secs_f32();
        self.last = Some(now);
        let distance = self.value - target;
        if reduce_motion || (distance.abs() < REST_DISTANCE && self.velocity.abs() < REST_SPEED) {
            self.value = target;
            self.velocity = 0.;
            self.last = None;
            return Sample {
                value: target,
                moving: false,
            };
        }
        // Exact solution of a critically damped spring over `dt`.
        let w = self.omega;
        let carried = self.velocity + w * distance;
        let decay = (-w * dt).exp();
        self.value = target + (distance + carried * dt) * decay;
        self.velocity = (self.velocity - w * carried * dt) * decay;
        Sample {
            value: self.value,
            moving: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spring: &mut Spring, target: f32, from: Instant, frames: u32) -> Sample {
        let mut sample = spring.step(target, from, false);
        for frame in 1..=frames {
            sample = spring.step(target, from + Duration::from_millis(16) * frame, false);
        }
        sample
    }

    #[test]
    fn it_settles_on_the_target_without_overshooting() {
        let start = Instant::now();
        let mut spring = Spring::new(0., Duration::from_millis(260));
        let mut highest = 0f32;
        let mut sample = spring.step(1., start, false);
        for frame in 1..40 {
            sample = spring.step(1., start + Duration::from_millis(16) * frame, false);
            highest = highest.max(sample.value);
        }
        assert!(highest <= 1.0001, "overshot to {highest}");
        assert!(!sample.moving && sample.value == 1.);
    }

    #[test]
    fn it_is_mostly_there_after_the_settle_time() {
        let start = Instant::now();
        let mut spring = Spring::new(0., Duration::from_millis(260));
        let sample = run(&mut spring, 1., start, 16); // 256 ms
        assert!(sample.value > 0.98, "only {} after 256ms", sample.value);
    }

    #[test]
    fn retargeting_midway_keeps_moving_from_where_it_is() {
        let start = Instant::now();
        let mut spring = Spring::new(0., Duration::from_millis(260));
        let half = run(&mut spring, 1., start, 6);
        assert!(half.value > 0.2 && half.value < 0.95);
        // Reverse: the next sample is near where it was, not back at either end.
        let next = spring.step(0., start + Duration::from_millis(16) * 7, false);
        assert!(
            (next.value - half.value).abs() < 0.2,
            "jumped to {}",
            next.value
        );
    }

    #[test]
    fn reduced_motion_snaps() {
        let mut spring = Spring::new(0., Duration::from_millis(260));
        let sample = spring.step(1., Instant::now(), true);
        assert!(sample.value == 1. && !sample.moving);
    }

    #[test]
    fn a_long_stall_does_not_fling_it() {
        let start = Instant::now();
        let mut spring = Spring::new(0., Duration::from_millis(260));
        spring.step(1., start, false);
        let sample = spring.step(1., start + Duration::from_secs(5), false);
        assert!(sample.value <= 1.0001);
    }
}
