//! A five-band equalizer for the built-in player.
//!
//! The curve is five filters: a low shelf at 60 Hz, peaks at 250 Hz, 1 kHz and
//! 4 kHz, and a high shelf at 12 kHz. Gains are whole half-decibels from -12
//! to +12, so a setting is `Copy`, comparable and cheap to store.
//!
//! Cost is paid only while the sound is shaped. With the equalizer off, or
//! every band at 0 dB, [`Processor::process`] returns before touching a
//! sample, so the audio is bit-for-bit the original. Settings reach the audio
//! thread through a handful of atomics: no lock, no allocation, nothing on the
//! interface thread waits for audio or the other way round.
//!
//! The player always decodes 44.1 kHz stereo and resamples to the device
//! afterwards, so the filters run at one fixed rate whatever the output is.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

pub const BAND_COUNT: usize = 5;
/// Where each band is labelled and its point sits on the graph, in hertz.
pub const FREQUENCIES: [f32; BAND_COUNT] = [60., 250., 1_000., 4_000., 12_000.];
/// Where each filter is actually centred. The shelves turn a little further
/// in, so a shelf has nearly its full gain at its label instead of half of it
/// and the point can sit on the curve.
const CORNERS: [f32; BAND_COUNT] = [120., 250., 1_000., 4_000., 6_000.];
/// Where each label falls across the graph, as a fraction of its width.
pub const ANCHORS: [f32; BAND_COUNT] = [0.08, 0.30, 0.52, 0.74, 0.94];
/// The most a band can boost or cut, in decibels.
pub const MAX_GAIN_DB: f32 = 12.;
/// One notch of a gain, in decibels. Gains are stored in these.
pub const STEP_DB: f32 = 0.5;
const MAX_STEPS: i8 = 24;
/// The sample rate of what librespot decodes.
const SAMPLE_RATE: f32 = 44_100.;
/// Peaks are two octaves apart, so one broad bell each joins into a curve.
const PEAK_Q: f32 = 0.7;
/// Frames filtered between gain updates. A change in a gain glides over a few
/// of these instead of stepping, which would click.
const BLOCK_FRAMES: usize = 64;
/// How far a glide moves toward its target each block.
const GLIDE: f32 = 0.12;
/// Gains closer than this to their target snap to it.
const SNAP_DB: f32 = 0.01;

/// One preset: a name and the gain of each band, in decibels.
pub struct Preset {
    pub name: &'static str,
    pub gains: [f32; BAND_COUNT],
}

pub const PRESETS: [Preset; 5] = [
    Preset {
        name: "Flat",
        gains: [0., 0., 0., 0., 0.],
    },
    Preset {
        name: "Bass boost",
        gains: [6., 3., 0., 0., 0.],
    },
    Preset {
        name: "Vocal",
        gains: [-2., 1., 4., 2., 0.],
    },
    Preset {
        name: "Treble",
        gains: [0., 0., 0., 4., 6.],
    },
    Preset {
        name: "Loudness",
        gains: [5., 1., -1., 2., 4.],
    },
];

/// What the listener chose: on or off, and the gain of each band in
/// half-decibel steps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub steps: [i8; BAND_COUNT],
}

impl Settings {
    pub fn gains_db(self) -> [f32; BAND_COUNT] {
        self.steps.map(|step| f32::from(step) * STEP_DB)
    }

    /// The nearest whole step to a gain in decibels, within the range.
    pub fn step_for(db: f32) -> i8 {
        (db / STEP_DB)
            .round()
            .clamp(-f32::from(MAX_STEPS), f32::from(MAX_STEPS)) as i8
    }

    pub fn with_gains(mut self, gains: [f32; BAND_COUNT]) -> Self {
        self.steps = gains.map(Self::step_for);
        self
    }

    /// True when no band changes the sound.
    pub fn is_flat(self) -> bool {
        self.steps == [0; BAND_COUNT]
    }

    /// `1,-2,0,...`: the form kept in the settings file.
    pub fn encode_steps(self) -> String {
        self.steps
            .iter()
            .map(i8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Reads [`Self::encode_steps`]. Anything unreadable is `None`, so a
    /// damaged value falls back to flat rather than to a half-applied curve.
    pub fn decode_steps(text: &str) -> Option<[i8; BAND_COUNT]> {
        let mut steps = [0; BAND_COUNT];
        let mut parts = text.split(',');
        for step in &mut steps {
            *step = parts
                .next()?
                .trim()
                .parse::<i8>()
                .ok()?
                .clamp(-MAX_STEPS, MAX_STEPS);
        }
        parts.next().is_none().then_some(steps)
    }
}

/// A curve the listener saved under a name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CustomPreset {
    pub name: String,
    pub steps: [i8; BAND_COUNT],
}

/// The most saved curves kept, and the longest name shown whole.
pub const MAX_CUSTOM_PRESETS: usize = 8;
pub const MAX_PRESET_NAME_CHARS: usize = 20;

// ---------------------------------------------------------------- filters

#[derive(Clone, Copy)]
enum Kind {
    LowShelf,
    Peak,
    HighShelf,
}

const KINDS: [Kind; BAND_COUNT] = [
    Kind::LowShelf,
    Kind::Peak,
    Kind::Peak,
    Kind::Peak,
    Kind::HighShelf,
];

/// A biquad, with `a0` already divided out. Worked out in double precision:
/// a 60 Hz filter at 44.1 kHz has coefficients that differ from their
/// neighbours by parts per million, which single precision would blur.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefficients {
    const IDENTITY: Self = Self {
        b0: 1.,
        b1: 0.,
        b2: 0.,
        a1: 0.,
        a2: 0.,
    };

    /// The RBJ audio-EQ cookbook filters, with a shelf slope of 1.
    fn new(kind: Kind, frequency: f32, gain_db: f32) -> Self {
        if gain_db == 0. {
            return Self::IDENTITY;
        }
        let a = 10f64.powf(f64::from(gain_db) / 40.);
        let w = std::f64::consts::TAU * f64::from(frequency) / f64::from(SAMPLE_RATE);
        let (sin, cos) = w.sin_cos();
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Peak => {
                let alpha = sin / (2. * f64::from(PEAK_Q));
                (
                    1. + alpha * a,
                    -2. * cos,
                    1. - alpha * a,
                    1. + alpha / a,
                    -2. * cos,
                    1. - alpha / a,
                )
            }
            Kind::LowShelf | Kind::HighShelf => {
                let alpha = sin / std::f64::consts::SQRT_2;
                let beta = 2. * a.sqrt() * alpha;
                if matches!(kind, Kind::LowShelf) {
                    (
                        a * ((a + 1.) - (a - 1.) * cos + beta),
                        2. * a * ((a - 1.) - (a + 1.) * cos),
                        a * ((a + 1.) - (a - 1.) * cos - beta),
                        (a + 1.) + (a - 1.) * cos + beta,
                        -2. * ((a - 1.) + (a + 1.) * cos),
                        (a + 1.) + (a - 1.) * cos - beta,
                    )
                } else {
                    (
                        a * ((a + 1.) + (a - 1.) * cos + beta),
                        -2. * a * ((a - 1.) + (a + 1.) * cos),
                        a * ((a + 1.) + (a - 1.) * cos - beta),
                        (a + 1.) - (a - 1.) * cos + beta,
                        2. * ((a - 1.) - (a + 1.) * cos),
                        (a + 1.) - (a - 1.) * cos - beta,
                    )
                }
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// The gain at `frequency`, in decibels.
    fn response_db(self, frequency: f32) -> f32 {
        let w = std::f64::consts::TAU * f64::from(frequency) / f64::from(SAMPLE_RATE);
        // |H| from the numerator and denominator as polynomials in e^-jw,
        // written as products of sums so nothing cancels badly.
        let (sin, cos) = w.sin_cos();
        let (sin2, cos2) = (2. * w).sin_cos();
        let numerator = (self.b0 + self.b1 * cos + self.b2 * cos2).powi(2)
            + (self.b1 * sin + self.b2 * sin2).powi(2);
        let denominator = (1. + self.a1 * cos + self.a2 * cos2).powi(2)
            + (self.a1 * sin + self.a2 * sin2).powi(2);
        (10. * (numerator / denominator.max(f64::MIN_POSITIVE))
            .max(f64::MIN_POSITIVE)
            .log10()) as f32
    }

    /// The same filter for the audio thread.
    fn single(self) -> [f32; 5] {
        [
            self.b0 as f32,
            self.b1 as f32,
            self.b2 as f32,
            self.a1 as f32,
            self.a2 as f32,
        ]
    }
}

/// The five filters for one set of gains, ready to be read off at any
/// frequency. This is the curve the interface draws, so it shows what the
/// filters really do.
pub struct Curve([Coefficients; BAND_COUNT]);

impl Curve {
    pub fn new(gains: [f32; BAND_COUNT]) -> Self {
        Self(std::array::from_fn(|band| {
            Coefficients::new(KINDS[band], CORNERS[band], gains[band])
        }))
    }

    /// The combined gain at `frequency`, in decibels.
    pub fn db_at(&self, frequency: f32) -> f32 {
        self.0.iter().map(|c| c.response_db(frequency)).sum()
    }
}

/// The combined gain of all five bands at `frequency`, in decibels.
pub fn response_db(gains: [f32; BAND_COUNT], frequency: f32) -> f32 {
    Curve::new(gains).db_at(frequency)
}

/// The frequency a fraction `p` of the way across the graph stands for.
/// Between labels the axis is logarithmic; past the outer two it carries on
/// at the same slope.
pub fn frequency_at(p: f32) -> f32 {
    let segment = ANCHORS
        .windows(2)
        .position(|pair| p < pair[1])
        .unwrap_or(BAND_COUNT - 2);
    let (a0, a1) = (ANCHORS[segment], ANCHORS[segment + 1]);
    let (l0, l1) = (FREQUENCIES[segment].log2(), FREQUENCIES[segment + 1].log2());
    (l0 + (p - a0) / (a1 - a0) * (l1 - l0)).exp2()
}

/// The gain band `band` needs for the curve to pass through `target_db` at
/// the band's own label, with the other bands as they are. Dragging a point
/// on the curve asks exactly this. The answer stays within the gain range.
pub fn gain_for_response(gains: [f32; BAND_COUNT], band: usize, target_db: f32) -> f32 {
    let frequency = FREQUENCIES[band];
    let others: f32 = (0..BAND_COUNT)
        .filter(|other| *other != band)
        .map(|other| {
            Coefficients::new(KINDS[other], CORNERS[other], gains[other]).response_db(frequency)
        })
        .sum();
    let own = |gain: f32| {
        Coefficients::new(KINDS[band], CORNERS[band], gain).response_db(frequency) + others
    };
    // A band's own gain only ever raises the response, so the answer can be
    // found by halving the range.
    let (mut low, mut high) = (-MAX_GAIN_DB, MAX_GAIN_DB);
    if target_db <= own(low) {
        return low;
    }
    if target_db >= own(high) {
        return high;
    }
    for _ in 0..24 {
        let middle = (low + high) / 2.;
        if own(middle) < target_db {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.
}

/// How far to lower the whole signal so the loudest boost cannot clip, in
/// decibels (zero or negative).
pub fn headroom_db(gains: [f32; BAND_COUNT]) -> f32 {
    let curve = Curve::new(gains);
    let mut peak = 0f32;
    // 48 log-spaced points from 20 Hz to 20 kHz: smooth curves, so this finds
    // the top to within a fraction of a decibel.
    for index in 0..48 {
        let frequency = 20. * 1_000f32.powf(index as f32 / 47.);
        peak = peak.max(curve.db_at(frequency));
    }
    -peak
}

// ------------------------------------------------------------- shared state

/// The listener's choice as the audio thread sees it. A write bumps
/// `generation` last, so a reader that sees a new generation sees the gains.
struct Shared {
    generation: AtomicU64,
    enabled: AtomicBool,
    /// Compare: hears the original while the settings stay put.
    bypass: AtomicBool,
    steps: [AtomicI32; BAND_COUNT],
}

impl Shared {
    const fn new() -> Self {
        Self {
            generation: AtomicU64::new(1),
            enabled: AtomicBool::new(false),
            bypass: AtomicBool::new(false),
            steps: [
                AtomicI32::new(0),
                AtomicI32::new(0),
                AtomicI32::new(0),
                AtomicI32::new(0),
                AtomicI32::new(0),
            ],
        }
    }

    fn publish(&self, settings: Settings, bypass: bool) {
        for (slot, step) in self.steps.iter().zip(settings.steps) {
            slot.store(i32::from(step), Ordering::Relaxed);
        }
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        self.bypass.store(bypass, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }
}

static SHARED: Shared = Shared::new();

/// Hands the settings to the audio thread. Cheap enough to call on every
/// pointer move of a drag.
pub fn publish(settings: Settings, bypass: bool) {
    SHARED.publish(settings, bypass);
}

// ---------------------------------------------------------------- processor

/// Filters interleaved stereo audio in place. One lives in the audio output.
pub struct Processor {
    source: &'static Shared,
    seen: u64,
    /// Where each band is heading, and where it is now, in decibels.
    target: [f32; BAND_COUNT],
    current: [f32; BAND_COUNT],
    target_headroom: f32,
    current_headroom: f32,
    coefficients: [[f32; 5]; BAND_COUNT],
    headroom_gain: f32,
    /// Direct form II transposed state: [band][channel][z1, z2].
    state: [[[f32; 2]; 2]; BAND_COUNT],
    /// True from a change until every gain has reached its target.
    gliding: bool,
    /// The next refresh jumps to the target instead of gliding.
    snap: bool,
}

impl Processor {
    pub fn new() -> Self {
        Self::reading(&SHARED)
    }

    fn reading(source: &'static Shared) -> Self {
        Self {
            source,
            seen: 0,
            target: [0.; BAND_COUNT],
            current: [0.; BAND_COUNT],
            target_headroom: 0.,
            current_headroom: 0.,
            coefficients: [Coefficients::IDENTITY.single(); BAND_COUNT],
            headroom_gain: 1.,
            state: [[[0.; 2]; 2]; BAND_COUNT],
            gliding: false,
            snap: true,
        }
    }

    /// Forgets the filters' memory and takes the current settings at once.
    /// Called when playback restarts, so a seek or a new song starts clean.
    pub fn reset(&mut self) {
        self.state = [[[0.; 2]; 2]; BAND_COUNT];
        self.seen = 0;
        self.snap = true;
    }

    fn refresh(&mut self) {
        let generation = self.source.generation.load(Ordering::Acquire);
        if generation == self.seen {
            return;
        }
        self.seen = generation;
        let active = self.source.enabled.load(Ordering::Relaxed)
            && !self.source.bypass.load(Ordering::Relaxed);
        self.target = if active {
            std::array::from_fn(|band| {
                let step = self.source.steps[band].load(Ordering::Relaxed);
                step.clamp(-i32::from(MAX_STEPS), i32::from(MAX_STEPS)) as f32 * STEP_DB
            })
        } else {
            [0.; BAND_COUNT]
        };
        self.target_headroom = headroom_db(self.target);
        if self.snap {
            self.current = self.target;
            self.current_headroom = self.target_headroom;
            self.snap = false;
            self.gliding = false;
            self.recompute(true);
        } else {
            self.gliding = true;
        }
    }

    fn recompute(&mut self, all: bool) {
        for band in 0..BAND_COUNT {
            if all || self.current[band] != self.target[band] || self.gliding {
                self.coefficients[band] =
                    Coefficients::new(KINDS[band], CORNERS[band], self.current[band]).single();
            }
        }
        self.headroom_gain = 10f32.powf(self.current_headroom / 20.);
    }

    /// One glide step toward the target. Returns false once there.
    fn glide(&mut self) -> bool {
        let mut moving = false;
        for band in 0..BAND_COUNT {
            let remaining = self.target[band] - self.current[band];
            if remaining.abs() <= SNAP_DB {
                self.current[band] = self.target[band];
            } else {
                self.current[band] += remaining * GLIDE;
                moving = true;
            }
        }
        let remaining = self.target_headroom - self.current_headroom;
        if remaining.abs() <= SNAP_DB {
            self.current_headroom = self.target_headroom;
        } else {
            self.current_headroom += remaining * GLIDE;
            moving = true;
        }
        self.recompute(false);
        moving
    }

    fn flat(&self) -> bool {
        self.current == [0.; BAND_COUNT] && self.current_headroom == 0.
    }

    /// Filters `samples` (interleaved stereo) in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        self.refresh();
        if !self.gliding && self.flat() {
            // The common case: nothing to do and nothing to pay.
            return;
        }
        for block in samples.chunks_mut(BLOCK_FRAMES * 2) {
            if self.gliding {
                self.gliding = self.glide();
            }
            self.filter(block);
        }
        if !self.gliding && self.flat() {
            // Back to the original: leave no stale state for the next start.
            self.state = [[[0.; 2]; 2]; BAND_COUNT];
        } else {
            self.flush_denormals();
        }
    }

    fn filter(&mut self, block: &mut [f32]) {
        let gain = self.headroom_gain;
        for frame in block.chunks_exact_mut(2) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let mut value = *sample * gain;
                for band in 0..BAND_COUNT {
                    let [b0, b1, b2, a1, a2] = self.coefficients[band];
                    let z = &mut self.state[band][channel];
                    let output = b0 * value + z[0];
                    z[0] = b1 * value - a1 * output + z[1];
                    z[1] = b2 * value - a2 * output;
                    value = output;
                }
                // A filter that ever went unstable would play as a blast of
                // noise; silence it and let the next block start clean.
                *sample = if value.is_finite() {
                    value.clamp(-1., 1.)
                } else {
                    self.state = [[[0.; 2]; 2]; BAND_COUNT];
                    0.
                };
            }
        }
    }

    /// Very small numbers slow some processors to a crawl; silence after a
    /// loud passage can leave a filter full of them.
    fn flush_denormals(&mut self) {
        for z in self.state.iter_mut().flatten().flatten() {
            if z.abs() < 1e-20 {
                *z = 0.;
            }
        }
    }
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let value =
                    0.25 * (std::f32::consts::TAU * frequency * n as f32 / SAMPLE_RATE).sin();
                [value, value]
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// A processor with a settings source of its own, so tests never touch
    /// the app-wide one.
    fn fixture(settings: Settings, bypass: bool) -> (Processor, &'static Shared) {
        let source: &'static Shared = Box::leak(Box::new(Shared::new()));
        source.publish(settings, bypass);
        (Processor::reading(source), source)
    }

    /// Settles a processor on `settings`, then returns the gain it gives a
    /// sine of `frequency` in decibels.
    fn measured_gain(settings: Settings, frequency: f32) -> f32 {
        let (mut processor, _) = fixture(settings, false);
        let mut warmup = sine(frequency, 44_100);
        processor.process(&mut warmup);
        let input = sine(frequency, 44_100);
        let mut output = input.clone();
        processor.process(&mut output);
        // Skip the first tenth of a second so the filter has settled.
        let skip = 8_820;
        20. * (rms(&output[skip..]) / rms(&input[skip..])).log10()
    }

    #[test]
    fn flat_and_off_leave_audio_bit_for_bit() {
        let original = sine(440., 4_096);
        for (enabled, steps) in [(false, [6; 5]), (true, [0; 5])] {
            let (mut processor, _) = fixture(Settings { enabled, steps }, false);
            let mut samples = original.clone();
            processor.process(&mut samples);
            assert_eq!(samples, original);
        }
    }

    #[test]
    fn compare_hears_the_original_while_settings_stay() {
        let original = sine(60., 4_096);
        let (mut processor, _) = fixture(
            Settings {
                enabled: true,
                steps: [12, 0, 0, 0, 0],
            },
            true,
        );
        let mut samples = original.clone();
        processor.process(&mut samples);
        assert_eq!(samples, original);
    }

    #[test]
    fn a_boosted_band_boosts_its_own_frequencies_and_not_the_others() {
        // +6 dB at 1 kHz, with the headroom the processor takes off the top.
        let settings = Settings {
            enabled: true,
            steps: [0, 0, 12, 0, 0],
        };
        let headroom = headroom_db(settings.gains_db());
        assert!(headroom < -5. && headroom > -7., "headroom {headroom}");
        let at_centre = measured_gain(settings, 1_000.);
        assert!(
            (at_centre - (6. + headroom)).abs() < 0.3,
            "centre {at_centre}"
        );
        // Far from the boost the signal is only lowered by the headroom.
        let far = measured_gain(settings, 60.);
        assert!((far - headroom).abs() < 1., "far {far}");
    }

    #[test]
    fn cuts_need_no_headroom_and_shelves_reach_their_ends() {
        let cut = Settings {
            enabled: true,
            steps: [0, 0, -12, 0, 0],
        };
        assert!(headroom_db(cut.gains_db()).abs() < 0.01);
        assert!((measured_gain(cut, 1_000.) + 6.).abs() < 0.3);
        let bass = Settings {
            enabled: true,
            steps: [24, 0, 0, 0, 0],
        };
        let headroom = headroom_db(bass.gains_db());
        // Below its corner a shelf is close to its full gain.
        let drawn = response_db(bass.gains_db(), 30.) + headroom;
        assert!(drawn > 10. + headroom);
        assert!((measured_gain(bass, 30.) - drawn).abs() < 0.5);
    }

    #[test]
    fn the_drawn_curve_matches_the_measured_filter() {
        let gains = [3., -2., 4., 0., -5.];
        let settings = Settings::default().with_gains(gains);
        let settings = Settings {
            enabled: true,
            ..settings
        };
        let headroom = headroom_db(settings.gains_db());
        for frequency in [100., 500., 2_000., 8_000.] {
            let drawn = response_db(settings.gains_db(), frequency) + headroom;
            let heard = measured_gain(settings, frequency);
            assert!(
                (drawn - heard).abs() < 0.4,
                "{frequency} Hz: {drawn} vs {heard}"
            );
        }
    }

    #[test]
    fn boosting_never_clips_a_full_scale_signal() {
        let settings = Settings {
            enabled: true,
            steps: [24; 5],
        };
        let (mut processor, _) = fixture(settings, false);
        for frequency in [40., 1_000., 10_000.] {
            let mut samples: Vec<f32> = sine(frequency, 22_050).iter().map(|s| s * 4.).collect();
            processor.process(&mut samples);
            assert!(samples.iter().all(|s| (-1. ..=1.).contains(s)));
        }
    }

    #[test]
    fn a_change_glides_instead_of_jumping() {
        let (mut processor, source) = fixture(Settings::default(), false);
        let mut warmup = sine(100., 1_024);
        processor.process(&mut warmup);
        source.publish(
            Settings {
                enabled: true,
                steps: [24, 0, 0, 0, 0],
            },
            false,
        );
        let mut samples = sine(100., 2_048);
        processor.process(&mut samples);
        // The first block is barely changed; by the end the boost has arrived.
        let start = rms(&samples[..128]);
        let reference = rms(&sine(100., 2_048)[..128]);
        assert!((start / reference - 1.).abs() < 0.2, "start {start}");
        assert!(processor.gliding || processor.current[0] > 11.9);
        // It reaches the target within a few thousand frames.
        let mut more = sine(100., 8_192);
        processor.process(&mut more);
        assert!(!processor.gliding);
        assert_eq!(processor.current[0], 12.);
    }

    #[test]
    fn labelled_frequencies_fall_on_the_anchors_and_the_axis_is_increasing() {
        for (anchor, frequency) in ANCHORS.iter().zip(FREQUENCIES) {
            assert!((frequency_at(*anchor) - frequency).abs() / frequency < 1e-3);
        }
        let mut last = 0.;
        for index in 0..=100 {
            let frequency = frequency_at(index as f32 / 100.);
            assert!(frequency > last && frequency.is_finite());
            last = frequency;
        }
        assert!(frequency_at(0.) > 20. && frequency_at(1.) < 20_000.);
    }

    #[test]
    fn dragging_a_point_puts_the_curve_through_it() {
        let gains = [3., -2., 4., 0., -5.];
        for band in 0..BAND_COUNT {
            for target in [-9., -3., 0., 2.5, 8.] {
                let gain = gain_for_response(gains, band, target);
                assert!(gain.abs() <= MAX_GAIN_DB);
                let mut moved = gains;
                moved[band] = gain;
                let reached = response_db(moved, FREQUENCIES[band]);
                let reachable = (gain.abs() - MAX_GAIN_DB).abs() > 1e-3;
                if reachable {
                    assert!(
                        (reached - target).abs() < 0.05,
                        "{band}: {reached} vs {target}"
                    );
                }
            }
        }
        // Shelves come close to their full gain at their label.
        let shelf = response_db([12., 0., 0., 0., 0.], FREQUENCIES[0]);
        assert!(shelf > 9.5, "{shelf}");
    }

    #[test]
    fn steps_round_trip_and_damaged_text_is_refused() {
        let settings = Settings {
            enabled: true,
            steps: [-24, -1, 0, 7, 24],
        };
        assert_eq!(
            Settings::decode_steps(&settings.encode_steps()),
            Some(settings.steps)
        );
        assert_eq!(Settings::decode_steps("1,2,3"), None);
        assert_eq!(Settings::decode_steps("1,2,3,4,5,6"), None);
        assert_eq!(Settings::decode_steps("a,b,c,d,e"), None);
        // Out-of-range values are pulled back in, not applied.
        assert_eq!(
            Settings::decode_steps("100,-100,0,0,0"),
            Some([24, -24, 0, 0, 0])
        );
    }

    #[test]
    fn gains_snap_to_half_decibels_within_range() {
        assert_eq!(Settings::step_for(0.2), 0);
        assert_eq!(Settings::step_for(0.3), 1);
        assert_eq!(Settings::step_for(-40.), -24);
        assert_eq!(Settings::step_for(40.), 24);
        let settings = Settings::default().with_gains(PRESETS[1].gains);
        assert_eq!(settings.gains_db(), PRESETS[1].gains);
    }

    #[test]
    fn every_preset_fits_the_range_and_flat_is_first() {
        assert_eq!(PRESETS[0].name, "Flat");
        assert!(PRESETS[0].gains.iter().all(|gain| *gain == 0.));
        for preset in PRESETS {
            assert!(preset.gains.iter().all(|gain| gain.abs() <= MAX_GAIN_DB));
        }
    }
}
