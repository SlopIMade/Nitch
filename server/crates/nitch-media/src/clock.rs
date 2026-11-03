//! The pipeline clock (D26): monotonic, so it never jumps, but running at the wall clock's rate.
//!
//! Under WSL2 the monotonic clock ran ~7 % slow: the output fell behind real time, players stalled
//! and the delay grew. The wall clock has the right rate only on average, because WSL steps it
//! forward by ~3 s every ~35 s, and pacing by those steps left a silent gap in every tile's audio.
//! So the wall clock's rate against the monotonic one is measured (least squares over minutes,
//! which averages the steps out) and the pipeline clock is calibrated to it, without steps. On a
//! normal Linux host the rate is 1.0.
//!
//! The estimate needs a few minutes of samples. The server measures from its start and hands the
//! current value to every new pipeline ([`crate::Config::clock_rate`]); the pipeline refines it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gst::prelude::*;
use gstreamer as gst;

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Samples older than this are dropped.
const WINDOW_SECS: f64 = 300.0;

/// Wall-clock rate per monotonic second, from (monotonic, wall) samples in seconds.
#[derive(Default)]
pub struct RateEstimator {
    samples: VecDeque<(f64, f64)>,
}

impl RateEstimator {
    pub fn add(&mut self, monotonic: f64, wall: f64) {
        self.samples.push_back((monotonic, wall));
        while self.samples.front().is_some_and(|s| monotonic - s.0 > WINDOW_SECS) {
            self.samples.pop_front();
        }
    }

    /// The rate, once the samples span at least `min_span` seconds.
    pub fn rate(&self, min_span: f64) -> Option<f64> {
        let (first, last) = (self.samples.front()?, self.samples.back()?);
        if last.0 - first.0 < min_span {
            return None;
        }
        let n = self.samples.len() as f64;
        let (mx, my) = self.samples.iter().fold((0.0, 0.0), |a, s| (a.0 + s.0 / n, a.1 + s.1 / n));
        let (sxy, sxx) = self
            .samples
            .iter()
            .fold((0.0, 0.0), |a, s| (a.0 + (s.0 - mx) * (s.1 - my), a.1 + (s.0 - mx).powi(2)));
        Some((sxy / sxx).clamp(0.8, 1.25))
    }
}

fn wall_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

static SERVER_RATE: AtomicU64 = AtomicU64::new(0);

/// Starts measuring in the background (server). Call once.
pub fn start_measuring() {
    std::thread::spawn(|| {
        let start = Instant::now();
        let mut est = RateEstimator::default();
        loop {
            est.add(start.elapsed().as_secs_f64(), wall_now());
            if let Some(rate) = est.rate(120.0) {
                SERVER_RATE.store(rate.to_bits(), Ordering::Relaxed);
            }
            std::thread::sleep(SAMPLE_EVERY);
        }
    });
}

/// The rate measured so far (1.0 until there's enough data).
pub fn measured_rate() -> f64 {
    match SERVER_RATE.load(Ordering::Relaxed) {
        0 => 1.0,
        bits => f64::from_bits(bits),
    }
}

/// A monotonic pipeline clock running at `rate` × real speed, refined in the background.
pub fn steady_wall_clock(rate: f64) -> gst::Clock {
    let clock: gst::Clock = gst::glib::Object::builder::<gst::SystemClock>()
        .property("clock-type", gst::ClockType::Monotonic)
        .build()
        .upcast();
    let calibrate = |clock: &gst::Clock, rate: f64| {
        // Continue from the current time with the new rate: no jump.
        let (internal, external) = (clock.internal_time(), clock.time());
        clock.set_calibration(internal, external, (rate * 1e9) as u64, 1_000_000_000);
    };
    calibrate(&clock, rate);
    let weak = clock.downgrade();
    std::thread::spawn(move || {
        let mut est = RateEstimator::default();
        loop {
            std::thread::sleep(SAMPLE_EVERY);
            let Some(clock) = weak.upgrade() else { return };
            est.add(clock.internal_time().nseconds() as f64 / 1e9, wall_now());
            if let Some(rate) = est.rate(240.0) {
                calibrate(&clock, rate);
            }
        }
    });
    clock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_out_steps() {
        // Monotonic runs 7 % slow; the wall clock catches up by 2.9 s every 35 s.
        let mut est = RateEstimator::default();
        let mut wall = 1000.0;
        let mut since_step = 0.0;
        for i in 0..150 {
            let mono = i as f64 * 2.0;
            if i > 0 {
                wall += 2.0;
                since_step += 2.0;
                if since_step >= 35.0 {
                    wall += 2.9;
                    since_step = 0.0;
                }
            }
            est.add(mono, wall);
        }
        let rate = est.rate(120.0).unwrap();
        assert!((rate - 1.083).abs() < 0.01, "rate {rate}");
    }

    #[test]
    fn needs_enough_data() {
        let mut est = RateEstimator::default();
        est.add(0.0, 0.0);
        est.add(10.0, 10.0);
        assert_eq!(est.rate(120.0), None);
    }
}
