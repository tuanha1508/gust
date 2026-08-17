//! Per-step latency aggregation for multi-endpoint scenarios.
//!
//! The overall [`Recorder`] answers "when did the system break?". Per-step
//! recorders answer "which endpoint was holding the pool?". Both use the same
//! coordinated-omission correction against the open-model send interval.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Outcome, Recorder, Summary};

/// Cumulative stats for one named step inside a scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepSummary {
    pub name: String,
    pub summary: Summary,
}

/// Overall run summary plus an optional per-step breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunBreakdown {
    pub overall: Summary,
    /// Empty for single-URL runs; one entry per named step otherwise.
    pub steps: Vec<StepSummary>,
}

/// Records every sample into an overall histogram and, when a step name is
/// provided, into a per-step histogram as well.
pub struct MultiRecorder {
    overall: Recorder,
    steps: BTreeMap<String, Recorder>,
    /// Seed interval used when lazily creating a per-step recorder.
    seed_interval: Option<Duration>,
}

impl MultiRecorder {
    pub fn new(expected_interval: Option<Duration>) -> Self {
        Self {
            overall: Recorder::new(expected_interval),
            steps: BTreeMap::new(),
            seed_interval: expected_interval,
        }
    }

    /// Record into the overall histogram and, if `step` is `Some`, the named
    /// step's histogram. Empty step names are treated as overall-only.
    pub fn record_with_interval(
        &mut self,
        latency: Duration,
        outcome: Outcome,
        expected_interval: Duration,
        step: Option<&str>,
    ) {
        self.overall
            .record_with_interval(latency, outcome, expected_interval);

        if let Some(name) = step.filter(|s| !s.is_empty()) {
            let seed = self.seed_interval;
            self.steps
                .entry(name.to_string())
                .or_insert_with(|| Recorder::new(seed))
                .record_with_interval(latency, outcome, expected_interval);
        }
    }

    pub fn take_window(&mut self) -> Option<crate::WindowStats> {
        self.overall.take_window()
    }

    pub fn summary(&self) -> Summary {
        self.overall.summary()
    }

    /// Overall + per-step summaries. Steps are ordered by descending corrected
    /// p99 so the slowest endpoint surfaces first.
    pub fn breakdown(&self) -> RunBreakdown {
        let mut steps: Vec<StepSummary> = self
            .steps
            .iter()
            .map(|(name, r)| StepSummary {
                name: name.clone(),
                summary: r.summary(),
            })
            .collect();
        steps.sort_by(|a, b| {
            b.summary
                .corrected
                .p99_ms
                .partial_cmp(&a.summary.corrected.p99_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        RunBreakdown {
            overall: self.overall.summary(),
            steps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_and_steps_accumulate_independently() {
        let mut m = MultiRecorder::new(None);
        m.record_with_interval(
            Duration::from_millis(10),
            Outcome::Success,
            Duration::from_millis(5),
            Some("fast"),
        );
        m.record_with_interval(
            Duration::from_millis(100),
            Outcome::Success,
            Duration::from_millis(5),
            Some("slow"),
        );
        m.record_with_interval(
            Duration::from_millis(100),
            Outcome::Success,
            Duration::from_millis(5),
            Some("slow"),
        );

        let b = m.breakdown();
        assert_eq!(b.overall.total, 3);
        assert_eq!(b.steps.len(), 2);
        // Slowest first.
        assert_eq!(b.steps[0].name, "slow");
        assert_eq!(b.steps[0].summary.total, 2);
        assert_eq!(b.steps[1].name, "fast");
        assert_eq!(b.steps[1].summary.total, 1);
        assert!(b.steps[0].summary.raw.p50_ms > b.steps[1].summary.raw.p50_ms);
    }

    #[test]
    fn empty_step_name_skips_breakdown() {
        let mut m = MultiRecorder::new(None);
        m.record_with_interval(
            Duration::from_millis(5),
            Outcome::Success,
            Duration::from_millis(5),
            Some(""),
        );
        m.record_with_interval(
            Duration::from_millis(5),
            Outcome::Success,
            Duration::from_millis(5),
            None,
        );
        assert!(m.breakdown().steps.is_empty());
        assert_eq!(m.summary().total, 2);
    }

    #[test]
    fn identifies_expensive_endpoint() {
        let mut m = MultiRecorder::new(Some(Duration::from_millis(10)));
        // Mimic demo-api costs: items 10ms, search 30ms, checkout 50ms.
        for _ in 0..60 {
            m.record_with_interval(
                Duration::from_millis(10),
                Outcome::Success,
                Duration::from_millis(10),
                Some("items"),
            );
        }
        for _ in 0..30 {
            m.record_with_interval(
                Duration::from_millis(30),
                Outcome::Success,
                Duration::from_millis(10),
                Some("search"),
            );
        }
        for _ in 0..10 {
            m.record_with_interval(
                Duration::from_millis(50),
                Outcome::Success,
                Duration::from_millis(10),
                Some("checkout"),
            );
        }
        let b = m.breakdown();
        assert_eq!(b.steps[0].name, "checkout");
        assert!(b.steps[0].summary.raw.p50_ms > b.steps[1].summary.raw.p50_ms);
        assert!(b.steps[1].summary.raw.p50_ms > b.steps[2].summary.raw.p50_ms);
    }
}
