//! Breaking-point (knee) detection over windowed run metrics.

use serde::{Deserialize, Serialize};

/// One ~100ms window of live metrics used for knee detection and charts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowMetric {
    /// Seconds since run start.
    pub t: f64,
    /// Intended open-model send rate for this window.
    pub target_rps: f64,
    /// Observed completion rate in this window.
    pub throughput: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    /// Fraction of failures in the window (0.0–1.0).
    pub error_rate: f64,
    /// In-flight HTTP requests at window sample time (backpressure signal).
    pub in_flight: f64,
}

/// Estimated load where the system starts to fall apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Knee {
    /// Time of the first degraded window (seconds).
    pub t: f64,
    /// Target RPS at the knee (breaking point).
    pub target_rps: f64,
    /// Suggested safe operating load (~75% of knee).
    pub recommended_rps: f64,
    /// Human-readable reason.
    pub reason: String,
}

/// Fraction of knee used for the recommended safe operating point.
pub const SAFE_FACTOR: f64 = 0.75;

/// Need ~1s of windowed data before claiming a knee (100ms snapshots).
const MIN_WINDOWS: usize = 10;

/// Error-rate threshold that counts as a break (1%).
const ERROR_THRESHOLD: f64 = 0.01;

/// p99 must exceed this multiple of the early baseline to count as a latency knee.
const LATENCY_MULTIPLIER: f64 = 3.0;

/// Absolute p99 rise required on top of the multiplier — kills false knees when
/// the baseline is a few hundred microseconds and jitter looks like 10×.
const MIN_ABS_P99_RISE_MS: f64 = 20.0;

/// Throughput / target below this (with a latency rise) counts as collapse.
const EFFICIENCY_FLOOR: f64 = 0.85;

/// Detect the knee: last *safe* window before quality degrades.
///
/// Heuristics (first match wins, scanning left → right after a short baseline):
/// 1. Error rate crosses [`ERROR_THRESHOLD`]
/// 2. p99 ≥ 3× early baseline **and** ≥ baseline + 20ms **and**
///    (sustained across two windows, **or** throughput efficiency collapsed)
///
/// A knee is a hand-off from working to broken, so the reported load always
/// comes from a window that actually worked. A run with no healthy window —
/// an unreachable host, a wrong port, a rejected auth header — has no knee.
pub fn detect(series: &[WindowMetric]) -> Option<Knee> {
    if series.len() < MIN_WINDOWS {
        return None;
    }

    let baseline_n = (series.len() / 3).clamp(3, 8);
    let mut baseline_p99: Vec<f64> = series[..baseline_n].iter().map(|w| w.p99_ms).collect();
    baseline_p99.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let baseline = baseline_p99[baseline_p99.len() / 2].max(0.05);

    // Start looking after the baseline window so early noise doesn't fire.
    for i in baseline_n..series.len() {
        let w = &series[i];
        let prev = &series[i - 1];

        if w.error_rate >= ERROR_THRESHOLD {
            // No healthy window yet means nothing has broken: the target was
            // failing before load ever mattered. Keep scanning in case it
            // recovers and later breaks for real.
            let Some(safe) = last_healthy_before(series, i) else {
                continue;
            };
            return Some(knee_at(
                safe,
                format!(
                    "error rate {:.1}% ≥ {:.0}%",
                    w.error_rate * 100.0,
                    ERROR_THRESHOLD * 100.0
                ),
            ));
        }

        if !is_latency_break(w.p99_ms, baseline) {
            continue;
        }

        let jumped = w.p99_ms > prev.p99_ms * 1.5 && is_latency_break(w.p99_ms, baseline);
        let efficiency = if w.target_rps > 1.0 {
            w.throughput / w.target_rps
        } else {
            1.0
        };
        let collapsed = efficiency < EFFICIENCY_FLOOR && is_latency_break(w.p99_ms, baseline);

        // A single noisy window is not a knee: require either throughput collapse
        // or the *next* window still hot (sustained degradation).
        let sustained = series
            .get(i + 1)
            .is_some_and(|n| is_latency_break(n.p99_ms, baseline));

        if !(jumped || collapsed) {
            continue;
        }
        if !collapsed && !sustained {
            continue;
        }

        let reason = if collapsed {
            format!(
                "p99 {:.1}ms ({:.0}× baseline) and throughput {:.0}% of target",
                w.p99_ms,
                w.p99_ms / baseline,
                efficiency * 100.0
            )
        } else {
            format!(
                "p99 {:.1}ms rose to {:.0}× early baseline ({baseline:.1}ms)",
                w.p99_ms,
                w.p99_ms / baseline
            )
        };
        let Some(safe) = last_healthy_before(series, i) else {
            continue;
        };
        return Some(knee_at(safe, reason));
    }

    None
}

fn is_latency_break(p99_ms: f64, baseline: f64) -> bool {
    p99_ms >= baseline * LATENCY_MULTIPLIER && p99_ms >= baseline + MIN_ABS_P99_RISE_MS
}

/// Last window before `i` that was still serving: errors below the break
/// threshold. Returns `None` when the run never had a working window.
fn last_healthy_before(series: &[WindowMetric], i: usize) -> Option<&WindowMetric> {
    series[..i]
        .iter()
        .rev()
        .find(|w| w.error_rate < ERROR_THRESHOLD)
}

fn knee_at(safe: &WindowMetric, reason: String) -> Knee {
    let target_rps = safe.target_rps.max(0.0);
    Knee {
        t: safe.t,
        target_rps,
        recommended_rps: target_rps * SAFE_FACTOR,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: f64, target: f64, thr: f64, p50: f64, p99: f64, err: f64) -> WindowMetric {
        WindowMetric {
            t,
            target_rps: target,
            throughput: thr,
            p50_ms: p50,
            p90_ms: p50 * 1.2,
            p99_ms: p99,
            error_rate: err,
            in_flight: 0.0,
        }
    }

    /// Classic hockey-stick: flat then p99 explodes while throughput stalls.
    fn hockey_stick() -> Vec<WindowMetric> {
        let mut s = Vec::new();
        for i in 0..20 {
            let t = i as f64;
            let target = 50.0 + t * 50.0; // 50 → 1000
            let (p99, thr) = if i < 12 {
                (5.0, target * 0.98)
            } else if i == 12 {
                (8.0, target * 0.95)
            } else {
                // Break: latency hockey-sticks, completions flatten.
                (5.0 + (i as f64 - 12.0) * 40.0, 650.0)
            };
            s.push(w(t, target, thr, 2.0, p99, 0.0));
        }
        s
    }

    #[test]
    fn detects_latency_hockey_stick() {
        let series = hockey_stick();
        let knee = detect(&series).expect("should find a knee");
        // Break starts around i=13; last safe is i=12 → target ≈ 50+12*50 = 650
        assert!(
            knee.target_rps > 400.0 && knee.target_rps < 800.0,
            "unexpected knee rps {}",
            knee.target_rps
        );
        assert!((knee.recommended_rps - knee.target_rps * SAFE_FACTOR).abs() < 1e-6);
        assert!(!knee.reason.is_empty());
    }

    #[test]
    fn detects_error_spike() {
        let mut series: Vec<WindowMetric> = (0..10)
            .map(|i| w(i as f64, 100.0 + i as f64 * 10.0, 100.0, 2.0, 5.0, 0.0))
            .collect();
        series.push(w(10.0, 200.0, 180.0, 2.0, 6.0, 0.05));
        let knee = detect(&series).expect("error knee");
        assert!(knee.target_rps < 200.0);
        assert!(knee.reason.contains("error"));
    }

    #[test]
    fn stable_series_has_no_knee() {
        let series: Vec<WindowMetric> = (0..20)
            .map(|i| {
                let target = 100.0 + i as f64 * 10.0;
                w(
                    i as f64,
                    target,
                    target * 0.99,
                    2.0,
                    5.0 + (i as f64) * 0.05,
                    0.0,
                )
            })
            .collect();
        assert!(detect(&series).is_none());
    }

    #[test]
    fn too_short_returns_none() {
        let series = vec![w(0.0, 10.0, 10.0, 1.0, 2.0, 0.0); 3];
        assert!(detect(&series).is_none());
    }

    /// Reproduces the false knee seen on a 2s run against a fast static server:
    /// baseline p99 ≈ 0.5ms, one window jumps to 5ms (10×) without real saturation.
    #[test]
    fn tiny_absolute_jitter_is_not_a_knee() {
        let mut series = Vec::new();
        for i in 0..12 {
            let p99 = if i == 8 { 5.0 } else { 0.5 + (i as f64) * 0.02 };
            series.push(w(i as f64 * 0.1, 40.0, 40.0, 0.4, p99, 0.0));
        }
        assert!(
            detect(&series).is_none(),
            "microsecond-scale jitter must not report a knee"
        );
    }

    #[test]
    fn single_spike_without_sustain_is_ignored() {
        // Enough windows, big absolute jump, but only one hot window then recovery.
        let mut series: Vec<WindowMetric> = (0..12)
            .map(|i| w(i as f64, 100.0, 99.0, 2.0, 5.0, 0.0))
            .collect();
        series[8] = w(8.0, 100.0, 99.0, 2.0, 80.0, 0.0); // lone spike
        assert!(detect(&series).is_none());
    }

    /// Pointing gust at a closed port used to report the send rate as the
    /// system's capacity, complete with a "safe operating load" derived from a
    /// server that never answered.
    #[test]
    fn all_requests_failing_is_not_a_knee() {
        let series: Vec<WindowMetric> = (0..50)
            .map(|i| w(i as f64 * 0.1, 300.0, 300.0, 0.06, 0.19, 1.0))
            .collect();
        assert!(
            detect(&series).is_none(),
            "a target that never responded has no measurable capacity"
        );
    }

    /// A target that is broken from the start and then recovers must not have
    /// the dead stretch reported as its capacity.
    #[test]
    fn knee_ignores_a_dead_start_and_measures_the_real_break() {
        let mut series: Vec<WindowMetric> = Vec::new();
        // First 12 windows: nothing is listening yet.
        for i in 0..12 {
            series.push(w(i as f64 * 0.1, 100.0, 0.0, 0.05, 0.1, 1.0));
        }
        // Then it serves cleanly while load climbs.
        for i in 12..30 {
            let target = 100.0 + (i - 12) as f64 * 20.0;
            series.push(w(i as f64 * 0.1, target, target, 2.0, 5.0, 0.0));
        }
        // Then it genuinely breaks.
        for i in 30..34 {
            let target = 100.0 + (i - 12) as f64 * 20.0;
            series.push(w(i as f64 * 0.1, target, target * 0.5, 40.0, 400.0, 0.0));
        }
        let knee = detect(&series).expect("real break after recovery");
        let healthy_ceiling = 100.0 + (29 - 12) as f64 * 20.0;
        assert!(
            knee.target_rps > 100.0 && knee.target_rps <= healthy_ceiling,
            "knee {} should come from the healthy stretch",
            knee.target_rps
        );
    }

    /// Errors partway through a healthy run are still a knee — the fix must not
    /// silence genuine error-rate breaks.
    #[test]
    fn errors_after_a_healthy_baseline_still_report_a_knee() {
        let mut series: Vec<WindowMetric> = (0..12)
            .map(|i| w(i as f64, 100.0 + i as f64 * 10.0, 100.0, 2.0, 5.0, 0.0))
            .collect();
        series.push(w(12.0, 220.0, 180.0, 2.0, 6.0, 0.4));
        let knee = detect(&series).expect("error knee after healthy baseline");
        assert!(knee.reason.contains("error"));
        assert!(knee.target_rps <= 210.0, "knee {}", knee.target_rps);
    }

    #[test]
    fn sustained_latency_break_still_detected() {
        let mut series: Vec<WindowMetric> = (0..14)
            .map(|i| {
                w(
                    i as f64,
                    100.0 + i as f64 * 20.0,
                    100.0 + i as f64 * 20.0,
                    2.0,
                    5.0,
                    0.0,
                )
            })
            .collect();
        // Two consecutive hot windows after a healthy baseline.
        series[10] = w(10.0, 300.0, 290.0, 2.0, 80.0, 0.0);
        series[11] = w(11.0, 320.0, 310.0, 2.0, 120.0, 0.0);
        series[12] = w(12.0, 340.0, 330.0, 2.0, 150.0, 0.0);
        let knee = detect(&series).expect("sustained break");
        assert!(knee.target_rps < 320.0);
    }
}
