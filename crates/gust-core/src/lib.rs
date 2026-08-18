//! Core measurement primitives for Gust.
//!
//! This crate is deliberately free of any I/O or async runtime so the
//! measurement logic can be unit-tested without a network. The single most
//! important thing it gets right is *coordinated omission*: when a target
//! stalls, a naive load tester stops sending requests and therefore never
//! records the latency those in-flight-but-never-sent requests would have
//! seen. The result is a latency distribution that looks great precisely when
//! the system is at its worst. Gust corrects for this by knowing the rate at
//! which requests were *supposed* to be sent.

mod compare;
mod knee;
mod profile;
mod scenario;
mod steps;

pub use compare::{
    CompareReport, Direction, MetricChange, RunMetrics, ThresholdViolation, Thresholds, Verdict,
    check_thresholds, compare,
};
pub use knee::{Knee, SAFE_FACTOR, WindowMetric, detect as detect_knee};
pub use profile::LoadProfile;
pub use scenario::{Scenario, ScenarioAuth, ScenarioMode, Step};
pub use steps::{MultiRecorder, RunBreakdown, StepSummary};

use std::time::Duration;

use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};

/// The result of a single request attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Success,
    /// Any non-2xx HTTP status, transport error, or timeout.
    Failure,
}

/// Latencies + outcome counts for one chart / knee window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowStats {
    pub percentiles: Percentiles,
    pub success: u64,
    pub failure: u64,
}

impl WindowStats {
    pub fn error_rate(&self) -> f64 {
        let total = self.success + self.failure;
        if total == 0 {
            0.0
        } else {
            self.failure as f64 / total as f64
        }
    }
}

/// Records latencies into an HDR histogram and, crucially, corrects for
/// coordinated omission against a planned sending interval.
pub struct Recorder {
    /// Raw latencies exactly as observed.
    raw: Histogram<u64>,
    /// Latencies corrected for coordinated omission.
    corrected: Histogram<u64>,
    /// A short-lived window of raw latencies, reset each time it is read. This
    /// is what makes the live chart show bands *separating* rather than a
    /// cumulative average that flattens everything out.
    window: Histogram<u64>,
    /// The interval at which requests were scheduled to be sent, in
    /// microseconds. `None` means we make no coordinated-omission correction
    /// (e.g. a closed-model run where back-pressure is intentional).
    expected_interval_us: Option<u64>,
    success: u64,
    failure: u64,
    window_success: u64,
    window_failure: u64,
}

impl Recorder {
    /// Creates a recorder tracking latencies from 1µs to 60s with three
    /// significant figures of precision.
    ///
    /// `expected_interval` is the planned gap between request sends in an
    /// open-model run; pass `None` to disable coordinated-omission correction.
    /// Under a ramp, prefer [`record_with_interval`] so each sample uses the
    /// interval that applied when it was *sent*.
    pub fn new(expected_interval: Option<Duration>) -> Self {
        // 1µs .. 60s covers everything a web request realistically hits while
        // keeping the histogram's memory footprint small.
        let raw = Histogram::new_with_bounds(1, 60_000_000, 3).expect("valid histogram bounds");
        let corrected = raw.clone();
        let window = raw.clone();
        Self {
            raw,
            corrected,
            window,
            expected_interval_us: expected_interval.map(|d| d.as_micros() as u64),
            success: 0,
            failure: 0,
            window_success: 0,
            window_failure: 0,
        }
    }

    /// Update the default CO interval (e.g. as a ramp's rate changes).
    pub fn set_expected_interval(&mut self, interval: Option<Duration>) {
        self.expected_interval_us = interval.map(|d| d.as_micros() as u64);
    }

    /// Records one completed request with its measured latency and outcome,
    /// using the recorder's current expected interval for CO correction.
    pub fn record(&mut self, latency: Duration, outcome: Outcome) {
        self.record_inner(latency, outcome, self.expected_interval_us);
    }

    /// Like [`record`], but corrects against the interval that applied when
    /// this request was scheduled (needed for ramps).
    pub fn record_with_interval(
        &mut self,
        latency: Duration,
        outcome: Outcome,
        expected_interval: Duration,
    ) {
        let us = expected_interval.as_micros() as u64;
        self.record_inner(latency, outcome, Some(us));
    }

    fn record_inner(&mut self, latency: Duration, outcome: Outcome, interval_us: Option<u64>) {
        let micros = (latency.as_micros() as u64).clamp(1, 60_000_000);

        self.raw.record(micros).expect("value within bounds");
        self.window.record(micros).expect("value within bounds");

        match interval_us {
            // Correct for coordinated omission: if a request took much longer
            // than the send interval, synthesize the latencies of the requests
            // that *should* have been sent while we were blocked.
            Some(interval) if interval > 0 => {
                self.corrected
                    .record_correct(micros, interval)
                    .expect("value within bounds");
            }
            _ => {
                self.corrected.record(micros).expect("value within bounds");
            }
        }

        match outcome {
            Outcome::Success => {
                self.success += 1;
                self.window_success += 1;
            }
            Outcome::Failure => {
                self.failure += 1;
                self.window_failure += 1;
            }
        }
    }

    /// Returns stats for requests recorded *since the last call* and resets
    /// the window. Returns `None` if nothing was recorded.
    pub fn take_window(&mut self) -> Option<WindowStats> {
        if self.window.is_empty() {
            return None;
        }
        let stats = WindowStats {
            percentiles: Percentiles::from_histogram(&self.window),
            success: self.window_success,
            failure: self.window_failure,
        };
        self.window.reset();
        self.window_success = 0;
        self.window_failure = 0;
        Some(stats)
    }

    /// Produces a point-in-time summary of everything recorded so far.
    pub fn summary(&self) -> Summary {
        Summary {
            total: self.success + self.failure,
            success: self.success,
            failure: self.failure,
            raw: Percentiles::from_histogram(&self.raw),
            corrected: Percentiles::from_histogram(&self.corrected),
        }
    }
}

/// A snapshot of latency percentiles, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Percentiles {
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
}

impl Percentiles {
    fn from_histogram(h: &Histogram<u64>) -> Self {
        let ms = |us: u64| us as f64 / 1000.0;
        Self {
            min_ms: ms(h.min()),
            p50_ms: ms(h.value_at_quantile(0.50)),
            p90_ms: ms(h.value_at_quantile(0.90)),
            p99_ms: ms(h.value_at_quantile(0.99)),
            p999_ms: ms(h.value_at_quantile(0.999)),
            max_ms: ms(h.max()),
        }
    }
}

/// A full run summary carrying both the raw and coordinated-omission-corrected
/// latency distributions so the gap between them is visible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub total: u64,
    pub success: u64,
    pub failure: u64,
    pub raw: Percentiles,
    pub corrected: Percentiles,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_outcomes() {
        let mut r = Recorder::new(None);
        r.record(Duration::from_millis(5), Outcome::Success);
        r.record(Duration::from_millis(5), Outcome::Failure);
        let s = r.summary();
        assert_eq!(s.total, 2);
        assert_eq!(s.success, 1);
        assert_eq!(s.failure, 1);
    }

    #[test]
    fn window_tracks_error_rate() {
        let mut r = Recorder::new(None);
        r.record(Duration::from_millis(1), Outcome::Success);
        r.record(Duration::from_millis(1), Outcome::Failure);
        let w = r.take_window().unwrap();
        assert_eq!(w.success, 1);
        assert_eq!(w.failure, 1);
        assert!((w.error_rate() - 0.5).abs() < 1e-9);
        assert!(r.take_window().is_none());
    }

    #[test]
    fn correction_inflates_tail_after_a_stall() {
        // Plan: one request every 1ms. Then simulate a single 500ms stall.
        let interval = Duration::from_millis(1);

        let mut corrected = Recorder::new(Some(interval));
        let mut naive = Recorder::new(None);

        for _ in 0..1000 {
            corrected.record(Duration::from_millis(1), Outcome::Success);
            naive.record(Duration::from_millis(1), Outcome::Success);
        }
        corrected.record(Duration::from_millis(500), Outcome::Success);
        naive.record(Duration::from_millis(500), Outcome::Success);

        // The naive p99 barely moves; the corrected p99 exposes the stall the
        // in-flight requests would have suffered.
        let naive_p99 = naive.summary().raw.p99_ms;
        let corrected_p99 = corrected.summary().corrected.p99_ms;
        assert!(
            corrected_p99 > naive_p99 * 10.0,
            "corrected p99 ({corrected_p99}ms) should dwarf naive p99 ({naive_p99}ms)"
        );
    }

    #[test]
    fn per_sample_interval_overrides_default() {
        let mut r = Recorder::new(Some(Duration::from_millis(100)));
        // A 1ms send interval with a 50ms stall should inflate corrected.
        r.record_with_interval(
            Duration::from_millis(50),
            Outcome::Success,
            Duration::from_millis(1),
        );
        assert!(r.summary().corrected.p99_ms >= r.summary().raw.p99_ms);
    }
}
