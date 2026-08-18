//! Plain-English auto-diagnosis of a finished run.
//!
//! Load testers dump percentiles and leave the reader to eyeball a chart.
//! Gust already knows the knee, the coordinated-omission gap, and (optionally)
//! an SLO budget — this module turns those signals into a short written verdict
//! a human (or a PR comment) can read in ten seconds.

use serde::{Deserialize, Serialize};

use crate::{Knee, SloCapacity, Summary, WindowMetric};

/// Primary failure mode Gust attributes to the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// Every request failed — nothing was served; no capacity story.
    DeadTarget,
    /// p99 climbed while the system was still answering (queueing / saturation).
    LatencySaturation,
    /// Offered load outran completions (throughput flattened under the target).
    ThroughputCollapse,
    /// Errors crossed the break threshold first.
    ErrorSpike,
    /// No clear break; the run stayed healthy under the loads exercised.
    Healthy,
    /// Not enough windowed data to say anything useful.
    InsufficientData,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeadTarget => "dead_target",
            Self::LatencySaturation => "latency_saturation",
            Self::ThroughputCollapse => "throughput_collapse",
            Self::ErrorSpike => "error_spike",
            Self::Healthy => "healthy",
            Self::InsufficientData => "insufficient_data",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DeadTarget => "dead target",
            Self::LatencySaturation => "latency saturation",
            Self::ThroughputCollapse => "throughput collapse",
            Self::ErrorSpike => "error spike",
            Self::Healthy => "healthy",
            Self::InsufficientData => "insufficient data",
        }
    }
}

/// Written diagnosis of a finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub cause: Cause,
    /// One-line headline for CLI / HTML banners.
    pub headline: String,
    /// Short supporting bullets (evidence a recruiter can skim).
    pub evidence: Vec<String>,
    /// Full narrative paragraph.
    pub narrative: String,
}

/// Inputs for [`diagnose`]. Kept as a struct so callers (CLI, future `gust
/// diagnose <run.json>`) share one construction path.
#[derive(Debug, Clone)]
pub struct DiagnosisInput<'a> {
    pub summary: &'a Summary,
    pub windows: &'a [WindowMetric],
    pub knee: Option<&'a Knee>,
    pub slo: Option<&'a SloCapacity>,
    pub failure_reason: Option<&'a str>,
}

const MIN_WINDOWS: usize = 10;
const ERROR_THRESHOLD: f64 = 0.01;
const EFFICIENCY_FLOOR: f64 = 0.85;
/// Corrected/raw p99 ratio that counts as a meaningful coordinated-omission gap.
const CO_GAP_RATIO: f64 = 1.5;
const CO_GAP_ABS_MS: f64 = 20.0;

/// Produce a plain-English diagnosis from a finished run's signals.
pub fn diagnose(input: DiagnosisInput<'_>) -> Diagnosis {
    let DiagnosisInput {
        summary,
        windows,
        knee,
        slo,
        failure_reason,
    } = input;

    if summary.total == 0 || windows.len() < MIN_WINDOWS {
        return Diagnosis {
            cause: Cause::InsufficientData,
            headline: "Not enough data to diagnose this run.".into(),
            evidence: vec![format!(
                "{} samples across {} windows — need a longer ramp or higher rate",
                summary.total,
                windows.len()
            )],
            narrative: "Gust needs roughly a second of windowed metrics before it \
                 can name a failure mode. Re-run with a longer duration or a \
                 steeper ramp."
                .into(),
        };
    }

    if summary.success == 0 {
        let detail = failure_reason.unwrap_or("every request failed").to_string();
        return Diagnosis {
            cause: Cause::DeadTarget,
            headline: "No capacity measured — the target never served a request.".into(),
            evidence: vec![
                format!("0 / {} successes", summary.total),
                format!("first failure: {detail}"),
            ],
            narrative: format!(
                "Every request failed ({detail}). Latency numbers above do not \
                 reflect work the target did — check the URL, auth, and that \
                 the service is actually listening before trusting a knee or SLO."
            ),
        };
    }

    let co_gap = co_gap_note(summary);
    let mut evidence = Vec::new();

    if let Some(k) = knee {
        evidence.push(format!(
            "knee ≈ {:.0} req/s at t={:.1}s — {}",
            k.target_rps, k.t, k.reason
        ));
        evidence.push(format!(
            "recommended safe load ≈ {:.0} req/s (75% of knee)",
            k.recommended_rps
        ));
    }

    if let Some(s) = slo {
        if s.sustainable_rps > 0.0 {
            evidence.push(format!(
                "SLO p99 ≤ {:.0}ms sustains ≈ {:.0} req/s{}",
                s.slo_p99_ms,
                s.sustainable_rps,
                if s.breached {
                    ""
                } else {
                    " (never breached — raise the load to find the ceiling)"
                }
            ));
        } else {
            evidence.push(format!(
                "SLO p99 ≤ {:.0}ms was not met at any tested load",
                s.slo_p99_ms
            ));
        }
    }

    if let Some(note) = &co_gap {
        evidence.push(note.clone());
    }

    if let Some(peak_if) = peak_in_flight(windows) {
        evidence.push(format!("peak in-flight ≈ {peak_if:.0} requests"));
    }

    // Classify the primary cause from the knee reason / post-knee windows.
    let cause = classify(summary, windows, knee);
    let headline = headline_for(cause, knee, summary);
    let narrative = narrative_for(cause, knee, summary, &co_gap, slo);

    if evidence.is_empty() {
        evidence.push(format!(
            "success rate {:.1}%, corrected p99 {:.1}ms",
            summary.success as f64 / summary.total as f64 * 100.0,
            summary.corrected.p99_ms
        ));
    }

    Diagnosis {
        cause,
        headline,
        evidence,
        narrative,
    }
}

fn classify(summary: &Summary, windows: &[WindowMetric], knee: Option<&Knee>) -> Cause {
    let Some(k) = knee else {
        return Cause::Healthy;
    };

    let reason = k.reason.to_ascii_lowercase();
    if reason.contains("error rate") {
        return Cause::ErrorSpike;
    }
    if reason.contains("throughput") {
        return Cause::ThroughputCollapse;
    }

    // Confirm with post-knee windows when the reason is a pure latency climb.
    if let Some(post) = windows.iter().find(|w| w.t > k.t + 0.2) {
        let eff = if post.target_rps > 1.0 {
            post.throughput / post.target_rps
        } else {
            1.0
        };
        if post.error_rate >= ERROR_THRESHOLD {
            return Cause::ErrorSpike;
        }
        if eff < EFFICIENCY_FLOOR {
            return Cause::ThroughputCollapse;
        }
    }

    if reason.contains("p99") || summary.corrected.p99_ms > summary.raw.p50_ms * 3.0 {
        return Cause::LatencySaturation;
    }

    Cause::LatencySaturation
}

fn headline_for(cause: Cause, knee: Option<&Knee>, summary: &Summary) -> String {
    match cause {
        Cause::DeadTarget => "No capacity measured — the target never served a request.".into(),
        Cause::InsufficientData => "Not enough data to diagnose this run.".into(),
        Cause::Healthy => "System held under the loads exercised — no clear breaking point.".into(),
        Cause::ErrorSpike => match knee {
            Some(k) => format!(
                "Broke on errors near ≈ {:.0} req/s — the target started failing requests.",
                k.target_rps
            ),
            None => "Error rate spiked under load.".into(),
        },
        Cause::ThroughputCollapse => match knee {
            Some(k) => format!(
                "Throughput collapsed near ≈ {:.0} req/s — the system could not keep up with arrivals.",
                k.target_rps
            ),
            None => "Throughput fell behind the offered load.".into(),
        },
        Cause::LatencySaturation => match knee {
            Some(k) => format!(
                "Latency saturated near ≈ {:.0} req/s — p99 peeled away from the service floor (corrected p99 {:.0}ms).",
                k.target_rps, summary.corrected.p99_ms
            ),
            None => format!(
                "Latency climbed under load (corrected p99 {:.0}ms).",
                summary.corrected.p99_ms
            ),
        },
    }
}

fn narrative_for(
    cause: Cause,
    knee: Option<&Knee>,
    _summary: &Summary,
    co_gap: &Option<String>,
    slo: Option<&SloCapacity>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    match cause {
        Cause::Healthy => {
            parts.push(
                "Across this run Gust did not see a sustained error spike, latency \
                 hockey-stick, or throughput collapse. Treat the top rate reached as a \
                 lower bound on capacity — raise the ramp to find the real ceiling."
                    .into(),
            );
        }
        Cause::ErrorSpike => {
            parts.push(
                "The first clear break was a rising error rate. That usually means the \
                 target is rejecting work (timeouts, 5xx, connection resets) rather than \
                 just queueing it. Fix reliability before chasing latency."
                    .into(),
            );
        }
        Cause::ThroughputCollapse => {
            parts.push(
                "Offered load outran completions: throughput flattened while the intended \
                 send rate kept climbing. That is classic backpressure — a bounded pool, \
                 saturated CPU, or a downstream bottleneck that cannot absorb arrivals."
                    .into(),
            );
        }
        Cause::LatencySaturation => {
            parts.push(
                "Requests kept succeeding, but p99 climbed far above the service floor. \
                 That pattern is queueing: a worker/connection pool is busy, so arrivals \
                 wait. The knee marks the last load the system was still surviving."
                    .into(),
            );
        }
        Cause::DeadTarget | Cause::InsufficientData => {}
    }

    if let Some(k) = knee
        && cause != Cause::Healthy
    {
        parts.push(format!(
            "Operate near ≈ {:.0} req/s (75% of the ≈ {:.0} req/s knee) until the \
             bottleneck is fixed.",
            k.recommended_rps, k.target_rps
        ));
    }

    if let Some(note) = co_gap {
        parts.push(format!(
            "{note} A closed-model tester that waits for each response would have \
             painted a friendlier picture of the same meltdown."
        ));
    }

    if let Some(s) = slo {
        if s.sustainable_rps > 0.0 {
            parts.push(format!(
                "Against a p99 ≤ {:.0}ms SLO the system sustains ≈ {:.0} req/s — that \
                 is the number a capacity planner can provision against.",
                s.slo_p99_ms, s.sustainable_rps
            ));
        } else {
            parts.push(format!(
                "The p99 ≤ {:.0}ms SLO was never met in this run; lower the load or \
                 raise the budget before calling the system ready.",
                s.slo_p99_ms
            ));
        }
    }

    if parts.is_empty() {
        "Gust recorded the run but has nothing further to add.".into()
    } else {
        parts.join(" ")
    }
}

fn co_gap_note(summary: &Summary) -> Option<String> {
    let raw = summary.raw.p99_ms;
    let corr = summary.corrected.p99_ms;
    if !raw.is_finite() || !corr.is_finite() || raw <= 0.0 {
        return None;
    }
    let ratio = corr / raw;
    let abs = corr - raw;
    if ratio >= CO_GAP_RATIO || abs >= CO_GAP_ABS_MS {
        Some(format!(
            "coordinated-omission gap: raw p99 {:.0}ms vs corrected {:.0}ms ({:.1}×) — \
             users feel the worse number",
            raw, corr, ratio
        ))
    } else {
        None
    }
}

fn peak_in_flight(windows: &[WindowMetric]) -> Option<f64> {
    windows
        .iter()
        .map(|w| w.in_flight)
        .filter(|v| v.is_finite())
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .filter(|v| *v >= 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Knee, Percentiles, SloCapacity, Summary, WindowMetric};

    fn pct(p99: f64) -> Percentiles {
        Percentiles {
            min_ms: 1.0,
            p50_ms: p99 / 4.0,
            p90_ms: p99 * 0.8,
            p99_ms: p99,
            p999_ms: p99,
            max_ms: p99,
        }
    }

    fn summary(raw_p99: f64, corr_p99: f64, success: u64, failure: u64) -> Summary {
        Summary {
            total: success + failure,
            success,
            failure,
            raw: pct(raw_p99),
            corrected: pct(corr_p99),
        }
    }

    fn win(t: f64, target: f64, thr: f64, p99: f64, err: f64, inflight: f64) -> WindowMetric {
        WindowMetric {
            t,
            target_rps: target,
            throughput: thr,
            p50_ms: p99 / 2.0,
            p90_ms: p99 * 0.9,
            p99_ms: p99,
            error_rate: err,
            in_flight: inflight,
        }
    }

    fn healthy_ramp() -> Vec<WindowMetric> {
        (0..20)
            .map(|i| {
                let t = i as f64 * 0.1;
                let target = 100.0 + i as f64 * 20.0;
                win(t, target, target, 10.0, 0.0, 1.0)
            })
            .collect()
    }

    fn saturating_ramp() -> (Vec<WindowMetric>, Knee) {
        let mut v = Vec::new();
        for i in 0..25 {
            let t = i as f64 * 0.1;
            let target = 200.0 + i as f64 * 40.0;
            let (p99, thr, inflight) = if target <= 800.0 {
                (11.0, target, 2.0)
            } else {
                (200.0, 750.0, 40.0)
            };
            v.push(win(t, target, thr, p99, 0.0, inflight));
        }
        let knee = Knee {
            t: 1.5,
            target_rps: 800.0,
            recommended_rps: 600.0,
            reason: "p99 200.0ms rose to 18× service floor (11.0ms)".into(),
        };
        (v, knee)
    }

    #[test]
    fn dead_target_is_named() {
        let s = summary(5.0, 5.0, 0, 50);
        let w = healthy_ramp();
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: None,
            slo: None,
            failure_reason: Some("connection refused"),
        });
        assert_eq!(d.cause, Cause::DeadTarget);
        assert!(d.headline.contains("never served"));
        assert!(d.narrative.contains("connection refused"));
    }

    #[test]
    fn latency_saturation_from_knee() {
        let (w, knee) = saturating_ramp();
        let s = summary(50.0, 400.0, 5000, 0);
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: Some(&knee),
            slo: None,
            failure_reason: None,
        });
        assert_eq!(d.cause, Cause::LatencySaturation);
        assert!(d.headline.contains("Latency saturated"));
        assert!(d.evidence.iter().any(|e| e.contains("knee")));
        assert!(
            d.evidence
                .iter()
                .any(|e| e.contains("coordinated-omission"))
        );
        assert!(d.narrative.contains("queueing"));
    }

    #[test]
    fn throughput_collapse_from_knee_reason() {
        let (w, mut knee) = saturating_ramp();
        knee.reason = "p99 39.8ms (40× service floor) and throughput 76% of target".into();
        let s = summary(40.0, 80.0, 4000, 0);
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: Some(&knee),
            slo: None,
            failure_reason: None,
        });
        assert_eq!(d.cause, Cause::ThroughputCollapse);
        assert!(d.headline.contains("Throughput collapsed"));
    }

    #[test]
    fn error_spike_from_knee_reason() {
        let w = healthy_ramp();
        let knee = Knee {
            t: 1.0,
            target_rps: 500.0,
            recommended_rps: 375.0,
            reason: "error rate 5.0% ≥ 1%".into(),
        };
        let s = summary(20.0, 25.0, 900, 100);
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: Some(&knee),
            slo: None,
            failure_reason: Some("HTTP 503"),
        });
        assert_eq!(d.cause, Cause::ErrorSpike);
        assert!(d.headline.contains("errors"));
    }

    #[test]
    fn healthy_run_without_knee() {
        let w = healthy_ramp();
        let s = summary(12.0, 12.0, 2000, 0);
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: None,
            slo: Some(&SloCapacity {
                slo_p99_ms: 50.0,
                sustainable_rps: 480.0,
                sustainable_throughput: 475.0,
                t: 1.8,
                breached: false,
            }),
            failure_reason: None,
        });
        assert_eq!(d.cause, Cause::Healthy);
        assert!(d.headline.contains("no clear breaking point"));
        assert!(d.evidence.iter().any(|e| e.contains("SLO")));
    }

    #[test]
    fn too_few_windows() {
        let w: Vec<_> = healthy_ramp().into_iter().take(3).collect();
        let s = summary(10.0, 10.0, 10, 0);
        let d = diagnose(DiagnosisInput {
            summary: &s,
            windows: &w,
            knee: None,
            slo: None,
            failure_reason: None,
        });
        assert_eq!(d.cause, Cause::InsufficientData);
    }
}
