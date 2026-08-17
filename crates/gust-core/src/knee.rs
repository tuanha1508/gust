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
    /// Time of the last *healthy* window (seconds).
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

/// p99 must exceed this multiple of the service-floor baseline to count as a
/// latency knee.
const LATENCY_MULTIPLIER: f64 = 3.0;

/// Absolute p99 rise required on top of the multiplier — kills false knees when
/// the baseline is a few hundred microseconds and jitter looks like 10×.
const MIN_ABS_P99_RISE_MS: f64 = 20.0;

/// A window is still "healthy" only while p99 stays within this multiple of the
/// service floor. Used when walking back from a break so a saturated window is
/// never reported as the last safe operating point.
const HEALTHY_LATENCY_FACTOR: f64 = 2.0;

/// Absolute headroom above the floor that still counts as healthy (covers small
/// absolute jitter when the floor itself is a few ms).
const HEALTHY_ABS_SLACK_MS: f64 = 10.0;

/// Consecutive latency-break windows required before we trust a gradual climb
/// (ramping saturation rarely has a single 1.5× jump between adjacent windows).
const SUSTAINED_LAT_WINDOWS: usize = 3;

/// Throughput / target below this (with a latency rise) counts as collapse.
const EFFICIENCY_FLOOR: f64 = 0.85;

/// Detect the knee: last *safe* window before quality degrades.
///
/// Heuristics (first match wins, scanning left → right after a short warmup):
/// 1. Error rate crosses [`ERROR_THRESHOLD`]
/// 2. p99 ≥ 3× service-floor baseline **and** ≥ baseline + 20ms **and**
///    (sustained across [`SUSTAINED_LAT_WINDOWS`] windows, a sharp jump, **or**
///    throughput efficiency collapsed)
///
/// A knee is a hand-off from working to broken, so the reported load always
/// comes from a window that was still healthy on *both* errors and latency. A
/// run with no healthy window — unreachable host, wrong port, rejected auth —
/// has no knee.
pub fn detect(series: &[WindowMetric]) -> Option<Knee> {
    if series.len() < MIN_WINDOWS {
        return None;
    }

    let baseline = estimate_baseline(series)?;
    // Skip a short prefix so a contaminated start (residual queue from a prior
    // run) does not fire on the first noisy windows while we are still finding
    // the floor.
    let start = (series.len() / 10).clamp(3, 8);

    for i in start..series.len() {
        let w = &series[i];
        let prev = &series[i - 1];

        if w.error_rate >= ERROR_THRESHOLD {
            let Some(safe) = last_healthy_before(series, i, baseline) else {
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

        let jumped = w.p99_ms > prev.p99_ms * 1.5;
        let efficiency = if w.target_rps > 1.0 {
            w.throughput / w.target_rps
        } else {
            1.0
        };
        let collapsed = efficiency < EFFICIENCY_FLOOR;
        let sustained = sustained_latency_run(series, i, baseline) >= SUSTAINED_LAT_WINDOWS;

        if !(jumped || collapsed || sustained) {
            continue;
        }
        // A lone jump that immediately recovers is not a knee.
        if jumped && !collapsed && !sustained {
            let next_hot = series
                .get(i + 1)
                .is_some_and(|n| is_latency_break(n.p99_ms, baseline));
            if !next_hot {
                continue;
            }
        }

        let reason = if collapsed {
            format!(
                "p99 {:.1}ms ({:.0}× service floor) and throughput {:.0}% of target",
                w.p99_ms,
                w.p99_ms / baseline,
                efficiency * 100.0
            )
        } else {
            format!(
                "p99 {:.1}ms rose to {:.0}× service floor ({baseline:.1}ms)",
                w.p99_ms,
                w.p99_ms / baseline
            )
        };
        let Some(safe) = last_healthy_before(series, i, baseline) else {
            continue;
        };
        return Some(knee_at(safe, reason));
    }

    None
}

/// Estimate the service-floor p99 from windows that were actually answering.
///
/// Uses the 20th percentile across low-error windows so a hot start (leftover
/// queue from a previous run) cannot inflate the baseline into the seconds and
/// silence every subsequent latency knee.
fn estimate_baseline(series: &[WindowMetric]) -> Option<f64> {
    let mut p99s: Vec<f64> = series
        .iter()
        .filter(|w| w.error_rate < ERROR_THRESHOLD)
        .map(|w| w.p99_ms)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect();
    if p99s.len() < 3 {
        return None;
    }
    p99s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (p99s.len() as f64 * 0.2).floor() as usize;
    Some(p99s[idx.min(p99s.len() - 1)].max(0.05))
}

fn is_latency_break(p99_ms: f64, baseline: f64) -> bool {
    p99_ms >= baseline * LATENCY_MULTIPLIER && p99_ms >= baseline + MIN_ABS_P99_RISE_MS
}

fn is_healthy(w: &WindowMetric, baseline: f64) -> bool {
    if w.error_rate >= ERROR_THRESHOLD {
        return false;
    }
    let latency_ceiling = (baseline * HEALTHY_LATENCY_FACTOR).max(baseline + HEALTHY_ABS_SLACK_MS);
    let latency_ok = w.p99_ms < latency_ceiling;
    // Prefer windows that were still keeping up with the intended send rate.
    let efficiency = if w.target_rps > 1.0 {
        w.throughput / w.target_rps
    } else {
        1.0
    };
    latency_ok && efficiency >= EFFICIENCY_FLOOR
}

/// Count how many consecutive windows ending at `i` (inclusive) are latency breaks.
fn sustained_latency_run(series: &[WindowMetric], i: usize, baseline: f64) -> usize {
    let mut n = 0;
    for w in series[..=i].iter().rev() {
        if is_latency_break(w.p99_ms, baseline) {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// Last window before `i` that was still serving well: low errors, p99 near the
/// service floor, and throughput keeping up. Returns `None` when the run never
/// had a healthy window.
fn last_healthy_before(series: &[WindowMetric], i: usize, baseline: f64) -> Option<&WindowMetric> {
    series[..i].iter().rev().find(|w| is_healthy(w, baseline))
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
        // Break starts around i=13; last healthy is near i=12 → target ≈ 650
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
        // Three consecutive hot windows after a healthy baseline.
        series[10] = w(10.0, 300.0, 290.0, 2.0, 80.0, 0.0);
        series[11] = w(11.0, 320.0, 310.0, 2.0, 120.0, 0.0);
        series[12] = w(12.0, 340.0, 330.0, 2.0, 150.0, 0.0);
        let knee = detect(&series).expect("sustained break");
        assert!(knee.target_rps < 320.0);
    }

    /// Residual queue from a prior run used to inflate the early-window baseline
    /// into the seconds, silencing every latency knee and leaving only a late
    /// error-rate knee at ~2× true capacity.
    #[test]
    fn hot_start_does_not_inflate_baseline_or_knee() {
        let mut series = Vec::new();
        // Contaminated start: leftover queue drains while the ramp is still low.
        for i in 0..20 {
            let target = 200.0 + i as f64 * 20.0;
            let p99 = 2000.0 - i as f64 * 80.0; // 2000 → 480
            series.push(w(
                i as f64 * 0.1,
                target,
                target * 0.9,
                50.0,
                p99.max(40.0),
                0.0,
            ));
        }
        // Quiet stretch at the service floor while load is still under capacity.
        for i in 20..50 {
            let target = 200.0 + i as f64 * 20.0; // 600 → 1180
            let (p99, thr) = if target < 720.0 {
                (11.0, target)
            } else if target < 800.0 {
                (18.0, target * 0.98)
            } else {
                // Gradual saturation: p99 climbs, throughput plateaus.
                let over = target - 800.0;
                (30.0 + over * 2.0, 720.0)
            };
            series.push(w(i as f64 * 0.1, target, thr, 10.0, p99, 0.0));
        }
        // End-of-run timeouts — the failure mode that used to become the knee.
        for i in 50..60 {
            let target = 200.0 + i as f64 * 20.0;
            series.push(w(i as f64 * 0.1, target, 400.0, 100.0, 5000.0, 0.5));
        }
        let knee = detect(&series).expect("should find the real break");
        assert!(
            knee.target_rps > 600.0 && knee.target_rps < 900.0,
            "contaminated start must not report knee at end-of-run load ({})",
            knee.target_rps
        );
        assert!(
            !knee.reason.contains("error rate") || knee.target_rps < 900.0,
            "if the error path wins, it must still walk back to the healthy band"
        );
    }

    /// Gradual ramp saturation (no single 1.5× jump) must still fire, and the
    /// reported knee must be the last window near the service floor — not the
    /// already-saturated window that tripped detection.
    #[test]
    fn gradual_ramp_reports_last_healthy_not_trip_window() {
        let mut series = Vec::new();
        for i in 0..40 {
            let target = 200.0 + i as f64 * 20.0; // 200 → 980
            let (p99, thr) = if target < 700.0 {
                (11.0, target)
            } else if target < 760.0 {
                (15.0, target)
            } else {
                // Climb ~8ms per window — never a 1.5× adjacent jump.
                let steps = ((target - 760.0) / 20.0).max(0.0);
                (25.0 + steps * 15.0, 700.0)
            };
            series.push(w(i as f64 * 0.1, target, thr, 10.0, p99, 0.0));
        }
        let knee = detect(&series).expect("gradual saturation");
        assert!(
            knee.target_rps >= 650.0 && knee.target_rps <= 780.0,
            "knee {} should sit at the last healthy load, not the trip window",
            knee.target_rps
        );
    }
}
