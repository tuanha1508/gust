//! SLO-driven capacity: the number capacity planners actually ask for.
//!
//! The knee answers "where does it break?". An SLO answers a sharper, more
//! actionable question: *given a p99 latency budget I promised my users, how
//! much load can this system take before it blows that budget?*
//!
//! Gust already schedules on an open-model clock and records per-window p99, so
//! it can read the sustainable rate straight off the ramp: walk the windows and
//! find the highest offered load that still held under the SLO before a
//! *sustained* breach. That is the capacity you can safely provision for.

use serde::{Deserialize, Serialize};

use crate::WindowMetric;

/// Result of an SLO capacity read over a run's window series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SloCapacity {
    /// The p99 latency budget this was measured against (ms).
    pub slo_p99_ms: f64,
    /// Highest offered load (req/s) that held under the SLO. `0.0` if the SLO
    /// was never met, even at the lightest load.
    pub sustainable_rps: f64,
    /// Observed throughput at that window (req/s) — what the system actually
    /// served while honoring the SLO.
    pub sustainable_throughput: f64,
    /// Seconds into the run for the window behind `sustainable_rps`.
    pub t: f64,
    /// Whether the SLO was breached during the run (a real ceiling was found).
    /// When `false`, the run never exceeded the budget and `sustainable_rps` is
    /// the *top rate reached*, not necessarily the true ceiling.
    pub breached: bool,
}

/// Need ~1s of windowed data (100ms snapshots) before reading a capacity.
const MIN_WINDOWS: usize = 10;

/// Error rate that disqualifies a window from "meeting the SLO".
const ERROR_THRESHOLD: f64 = 0.01;

/// Consecutive over-budget windows required before we trust a breach (a single
/// noisy window should not define the ceiling).
const SUSTAINED_BREACH_WINDOWS: usize = 3;

/// Compute SLO capacity: the max offered load that held p99 under `slo_p99_ms`.
///
/// Returns `None` only when there is too little data. When the SLO is missed
/// from the very start, returns a report with `sustainable_rps == 0.0` and
/// `breached == true`.
pub fn capacity(series: &[WindowMetric], slo_p99_ms: f64) -> Option<SloCapacity> {
    if series.len() < MIN_WINDOWS || slo_p99_ms <= 0.0 {
        return None;
    }

    // Skip a short warmup so a contaminated start does not anchor the read.
    let start = (series.len() / 10).clamp(3, 8);

    let meets = |w: &WindowMetric| w.error_rate < ERROR_THRESHOLD && w.p99_ms <= slo_p99_ms;

    // Track the best (highest offered load) window that met the SLO so far.
    let mut best: Option<&WindowMetric> = None;

    for (i, w) in series.iter().enumerate().skip(start) {
        if meets(w) {
            if best.is_none_or(|b| w.target_rps > b.target_rps) {
                best = Some(w);
            }
            continue;
        }
        // A window missed the SLO — only a *sustained* miss counts as the ceiling.
        let sustained = series[i..]
            .iter()
            .take(SUSTAINED_BREACH_WINDOWS)
            .filter(|x| !meets(x))
            .count()
            >= SUSTAINED_BREACH_WINDOWS.min(series.len() - i);
        if sustained {
            return Some(match best {
                Some(b) => SloCapacity {
                    slo_p99_ms,
                    sustainable_rps: b.target_rps,
                    sustainable_throughput: b.throughput,
                    t: b.t,
                    breached: true,
                },
                None => SloCapacity {
                    slo_p99_ms,
                    sustainable_rps: 0.0,
                    sustainable_throughput: 0.0,
                    t: w.t,
                    breached: true,
                },
            });
        }
    }

    // Never sustainably breached: report the top rate that met the SLO.
    best.map(|b| SloCapacity {
        slo_p99_ms,
        sustainable_rps: b.target_rps,
        sustainable_throughput: b.throughput,
        t: b.t,
        breached: false,
    })
    .or(Some(SloCapacity {
        slo_p99_ms,
        sustainable_rps: 0.0,
        sustainable_throughput: 0.0,
        t: series[start].t,
        breached: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(t: f64, target: f64, thr: f64, p99: f64, err: f64) -> WindowMetric {
        WindowMetric {
            t,
            target_rps: target,
            throughput: thr,
            p50_ms: p99 / 2.0,
            p90_ms: p99 * 0.9,
            p99_ms: p99,
            error_rate: err,
            in_flight: 0.0,
        }
    }

    /// A ramp that holds ~10ms until it crosses the SLO near 800 req/s.
    fn ramp() -> Vec<WindowMetric> {
        let mut v = Vec::new();
        for i in 0..20 {
            let target = 100.0 + i as f64 * 50.0; // 100 → 1050
            let p99 = if target <= 800.0 { 10.0 } else { 250.0 };
            let thr = target.min(820.0);
            v.push(win(i as f64 * 0.1, target, thr, p99, 0.0));
        }
        v
    }

    #[test]
    fn reads_sustainable_rate_under_slo() {
        let cap = capacity(&ramp(), 100.0).unwrap();
        assert!(cap.breached);
        // Last window under the SLO was target 800 (p99 10ms), next crossed.
        assert!(
            (cap.sustainable_rps - 800.0).abs() < 1e-6,
            "got {}",
            cap.sustainable_rps
        );
    }

    #[test]
    fn tighter_slo_gives_lower_capacity() {
        // With a 5ms budget nothing in the ramp qualifies (floor is 10ms).
        let cap = capacity(&ramp(), 5.0).unwrap();
        assert_eq!(cap.sustainable_rps, 0.0);
        assert!(cap.breached);
    }

    #[test]
    fn loose_slo_never_breached_reports_top_rate() {
        let cap = capacity(&ramp(), 10_000.0).unwrap();
        assert!(!cap.breached);
        // Top offered load reached in the ramp.
        assert!(cap.sustainable_rps >= 1050.0 - 1e-6);
    }

    #[test]
    fn single_noisy_window_does_not_define_ceiling() {
        let mut v = ramp();
        // Inject one isolated spike below the real breach, then recovery.
        v[10] = win(1.0, 600.0, 600.0, 250.0, 0.0);
        let cap = capacity(&v, 100.0).unwrap();
        // Real ceiling is still ~800, not 550 (the window before the spike).
        assert!(
            cap.sustainable_rps >= 800.0 - 1e-6,
            "got {}",
            cap.sustainable_rps
        );
    }

    #[test]
    fn too_short_returns_none() {
        let short: Vec<_> = ramp().into_iter().take(5).collect();
        assert!(capacity(&short, 100.0).is_none());
    }

    #[test]
    fn errors_disqualify_a_window() {
        let mut v = ramp();
        // Make the 800-target window error out; capacity should drop below it.
        for w in v.iter_mut() {
            if (w.target_rps - 800.0).abs() < 1e-6 {
                w.error_rate = 0.5;
            }
        }
        let cap = capacity(&v, 100.0).unwrap();
        assert!(
            cap.sustainable_rps <= 750.0 + 1e-6,
            "got {}",
            cap.sustainable_rps
        );
    }
}
