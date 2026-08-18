//! Compare two runs and check CI-style performance thresholds.
//!
//! These are the recruiter-facing product primitives: save a baseline, change
//! the system, prove the capacity / latency story with numbers — not vibes.

use serde::Serialize;

use crate::{Knee, SloCapacity, Summary};

/// Metrics extracted from a finished run for comparison / gates.
#[derive(Debug, Clone, PartialEq)]
pub struct RunMetrics {
    pub corrected_p99_ms: f64,
    /// Successes / total, 0.0–1.0. Zero when the run recorded nothing.
    pub success_rate: f64,
    /// Failures / total, 0.0–1.0.
    pub error_rate: f64,
    pub knee_rps: Option<f64>,
    pub recommended_rps: Option<f64>,
    /// SLO-sustainable load and the budget it was measured against.
    pub slo_rps: Option<f64>,
    pub slo_p99_ms: Option<f64>,
    pub total: u64,
}

impl RunMetrics {
    pub fn from_summary(summary: &Summary, knee: Option<&Knee>) -> Self {
        Self::from_run(summary, knee, None)
    }

    pub fn from_run(summary: &Summary, knee: Option<&Knee>, slo: Option<&SloCapacity>) -> Self {
        let total = summary.total;
        let (success_rate, error_rate) = if total == 0 {
            (0.0, 0.0)
        } else {
            (
                summary.success as f64 / total as f64,
                summary.failure as f64 / total as f64,
            )
        };
        Self {
            corrected_p99_ms: summary.corrected.p99_ms,
            success_rate,
            error_rate,
            knee_rps: knee.map(|k| k.target_rps),
            recommended_rps: knee.map(|k| k.recommended_rps),
            slo_rps: slo.map(|s| s.sustainable_rps),
            slo_p99_ms: slo.map(|s| s.slo_p99_ms),
            total,
        }
    }
}

/// How one metric moved from baseline → candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Improved,
    Regressed,
    Equivalent,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::Regressed => "regressed",
            Self::Equivalent => "equivalent",
        }
    }
}

/// One metric's absolute values plus the classified delta.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricChange {
    pub name: &'static str,
    pub baseline: f64,
    pub candidate: f64,
    /// `candidate - baseline` (positive means the raw number went up).
    pub delta: f64,
    pub direction: Direction,
}

/// Overall compare verdict for CI / humans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every comparable metric improved or stayed equivalent; at least one improved.
    Improved,
    /// Every comparable metric stayed within tolerance.
    Equivalent,
    /// At least one regression and at least one improvement.
    Mixed,
    /// At least one regression and no improvements.
    Regressed,
}

impl Verdict {
    /// Exit non-zero in CI when the candidate is not clearly as-good-or-better.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Regressed | Self::Mixed)
    }

    /// Uppercase label for headlines (`IMPROVED`, `REGRESSED`, …).
    pub fn label(self) -> &'static str {
        match self {
            Self::Improved => "IMPROVED",
            Self::Equivalent => "EQUIVALENT",
            Self::Mixed => "MIXED",
            Self::Regressed => "REGRESSED",
        }
    }
}

/// Full compare result: per-metric changes + rollup verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompareReport {
    pub p99: MetricChange,
    pub error_rate: MetricChange,
    pub knee: Option<MetricChange>,
    /// SLO-sustainable RPS delta, when both runs used the *same* p99 budget.
    pub slo: Option<MetricChange>,
    pub verdict: Verdict,
}

impl CompareReport {
    /// Metric changes in display order (skips absent knee / SLO rows).
    pub fn metrics(&self) -> Vec<&MetricChange> {
        let mut v = vec![&self.p99, &self.error_rate];
        if let Some(k) = &self.knee {
            v.push(k);
        }
        if let Some(s) = &self.slo {
            v.push(s);
        }
        v
    }

    /// GitHub-friendly Markdown, suitable for a PR comment.
    pub fn to_markdown(&self) -> String {
        let emoji = match self.verdict {
            Verdict::Improved => "✅",
            Verdict::Equivalent => "➖",
            Verdict::Mixed => "⚠️",
            Verdict::Regressed => "❌",
        };
        let mut out = String::new();
        out.push_str("### gust compare\n\n");
        out.push_str(&format!(
            "**Verdict: {} {}**\n\n",
            emoji,
            self.verdict.label()
        ));
        out.push_str("| metric | baseline | candidate | Δ | |\n");
        out.push_str("| --- | ---: | ---: | ---: | :--- |\n");
        for m in self.metrics() {
            out.push_str(&format!(
                "| {} | {:.3} | {:.3} | {:+.3} | {} |\n",
                m.name,
                m.baseline,
                m.candidate,
                m.delta,
                m.direction.as_str(),
            ));
        }
        out
    }
}

/// Relative + absolute slop so noise does not flip the verdict.
const P99_REL: f64 = 0.05;
const P99_ABS_MS: f64 = 1.0;
const ERR_ABS: f64 = 0.005; // 0.5 percentage points
const KNEE_REL: f64 = 0.05;
const KNEE_ABS_RPS: f64 = 10.0;

/// Compare candidate against baseline.
///
/// - Corrected p99: lower is better
/// - Error rate: lower is better
/// - Knee RPS: higher is better (skipped if either run lacks a knee)
pub fn compare(baseline: &RunMetrics, candidate: &RunMetrics) -> CompareReport {
    let p99 = change(
        "corrected p99 (ms)",
        baseline.corrected_p99_ms,
        candidate.corrected_p99_ms,
        /*lower_is_better*/ true,
        P99_REL,
        P99_ABS_MS,
    );
    let error_rate = change(
        "error rate",
        baseline.error_rate,
        candidate.error_rate,
        true,
        0.0,
        ERR_ABS,
    );
    let knee = match (baseline.knee_rps, candidate.knee_rps) {
        (Some(b), Some(c)) => Some(change(
            "knee (req/s)",
            b,
            c,
            /*lower_is_better*/ false,
            KNEE_REL,
            KNEE_ABS_RPS,
        )),
        _ => None,
    };

    // Only compare SLO capacity when both runs measured the same budget.
    let slo = match (
        baseline.slo_rps,
        candidate.slo_rps,
        baseline.slo_p99_ms,
        candidate.slo_p99_ms,
    ) {
        (Some(b), Some(c), Some(bp), Some(cp)) if (bp - cp).abs() < 1e-6 => Some(change(
            "SLO capacity (req/s)",
            b,
            c,
            /*lower_is_better*/ false,
            KNEE_REL,
            KNEE_ABS_RPS,
        )),
        _ => None,
    };

    let directions: Vec<Direction> = std::iter::once(p99.direction)
        .chain(std::iter::once(error_rate.direction))
        .chain(knee.as_ref().map(|k| k.direction))
        .chain(slo.as_ref().map(|k| k.direction))
        .collect();

    let improved = directions.contains(&Direction::Improved);
    let regressed = directions.contains(&Direction::Regressed);

    let verdict = match (improved, regressed) {
        (true, false) => Verdict::Improved,
        (false, true) => Verdict::Regressed,
        (true, true) => Verdict::Mixed,
        (false, false) => Verdict::Equivalent,
    };

    CompareReport {
        p99,
        error_rate,
        knee,
        slo,
        verdict,
    }
}

fn change(
    name: &'static str,
    baseline: f64,
    candidate: f64,
    lower_is_better: bool,
    rel_tol: f64,
    abs_tol: f64,
) -> MetricChange {
    let delta = candidate - baseline;
    let tol = abs_tol.max(baseline.abs() * rel_tol);
    let direction = if delta.abs() <= tol {
        Direction::Equivalent
    } else if lower_is_better {
        if delta < 0.0 {
            Direction::Improved
        } else {
            Direction::Regressed
        }
    } else if delta > 0.0 {
        Direction::Improved
    } else {
        Direction::Regressed
    };

    MetricChange {
        name,
        baseline,
        candidate,
        delta,
        direction,
    }
}

/// Optional gates for `gust run` in CI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Thresholds {
    /// Fail when corrected p99 exceeds this (ms).
    pub max_corrected_p99_ms: Option<f64>,
    /// Fail when failure/total exceeds this (0.0–1.0).
    pub max_error_rate: Option<f64>,
    /// Fail when success/total is below this (0.0–1.0).
    pub min_success_rate: Option<f64>,
    /// Fail when a knee was found but sits below this RPS.
    pub min_knee_rps: Option<f64>,
    /// Fail when a knee was required (`require_knee`) but none was detected.
    pub require_knee: bool,
}

/// One failed gate, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdViolation {
    pub detail: String,
}

/// Check metrics against thresholds. Empty vec = pass.
pub fn check_thresholds(metrics: &RunMetrics, t: &Thresholds) -> Vec<ThresholdViolation> {
    let mut out = Vec::new();

    if let Some(max) = t.max_corrected_p99_ms
        && metrics.corrected_p99_ms > max
    {
        out.push(ThresholdViolation {
            detail: format!(
                "corrected p99 {:.2}ms exceeds --max-p99-ms {max}",
                metrics.corrected_p99_ms
            ),
        });
    }

    if let Some(max) = t.max_error_rate
        && metrics.error_rate > max
    {
        out.push(ThresholdViolation {
            detail: format!(
                "error rate {:.2}% exceeds --max-error-rate {:.2}%",
                metrics.error_rate * 100.0,
                max * 100.0
            ),
        });
    }

    if let Some(min) = t.min_success_rate
        && metrics.success_rate < min
    {
        out.push(ThresholdViolation {
            detail: format!(
                "success rate {:.2}% below --min-success-rate {:.2}%",
                metrics.success_rate * 100.0,
                min * 100.0
            ),
        });
    }

    if t.require_knee && metrics.knee_rps.is_none() {
        out.push(ThresholdViolation {
            detail: "no knee detected (--require-knee)".into(),
        });
    }

    if let Some(min) = t.min_knee_rps {
        match metrics.knee_rps {
            Some(knee) if knee < min => out.push(ThresholdViolation {
                detail: format!("knee {knee:.0} req/s below --min-knee-rps {min}"),
            }),
            None => out.push(ThresholdViolation {
                detail: format!("no knee detected (needed for --min-knee-rps {min})"),
            }),
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Percentiles, Summary};

    fn summary(p99: f64, success: u64, failure: u64) -> Summary {
        let pct = Percentiles {
            min_ms: 1.0,
            p50_ms: p99 / 2.0,
            p90_ms: p99 * 0.9,
            p99_ms: p99,
            p999_ms: p99,
            max_ms: p99,
        };
        Summary {
            total: success + failure,
            success,
            failure,
            raw: pct,
            corrected: pct,
        }
    }

    fn knee(rps: f64) -> Knee {
        Knee {
            t: 1.0,
            target_rps: rps,
            recommended_rps: rps * 0.75,
            reason: "test".into(),
        }
    }

    #[test]
    fn improved_when_p99_drops_and_knee_rises() {
        let base = RunMetrics::from_summary(&summary(100.0, 100, 0), Some(&knee(700.0)));
        let cand = RunMetrics::from_summary(&summary(40.0, 100, 0), Some(&knee(900.0)));
        let r = compare(&base, &cand);
        assert_eq!(r.verdict, Verdict::Improved);
        assert_eq!(r.p99.direction, Direction::Improved);
        assert_eq!(r.knee.as_ref().unwrap().direction, Direction::Improved);
        assert!(!r.verdict.is_failure());
    }

    #[test]
    fn regressed_when_p99_climbs() {
        let base = RunMetrics::from_summary(&summary(40.0, 100, 0), None);
        let cand = RunMetrics::from_summary(&summary(80.0, 100, 0), None);
        let r = compare(&base, &cand);
        assert_eq!(r.verdict, Verdict::Regressed);
        assert!(r.verdict.is_failure());
    }

    #[test]
    fn mixed_when_p99_improves_but_errors_rise() {
        let base = RunMetrics::from_summary(&summary(100.0, 100, 0), None);
        let cand = RunMetrics::from_summary(&summary(40.0, 90, 10), None);
        let r = compare(&base, &cand);
        assert_eq!(r.verdict, Verdict::Mixed);
        assert!(r.verdict.is_failure());
    }

    #[test]
    fn equivalent_within_tolerance() {
        let base = RunMetrics::from_summary(&summary(100.0, 100, 0), Some(&knee(800.0)));
        let cand = RunMetrics::from_summary(&summary(102.0, 100, 0), Some(&knee(805.0)));
        let r = compare(&base, &cand);
        assert_eq!(r.verdict, Verdict::Equivalent);
    }

    #[test]
    fn thresholds_catch_p99_and_missing_knee() {
        let m = RunMetrics::from_summary(&summary(50.0, 95, 5), None);
        let t = Thresholds {
            max_corrected_p99_ms: Some(40.0),
            max_error_rate: Some(0.01),
            min_success_rate: Some(0.99),
            min_knee_rps: Some(500.0),
            require_knee: true,
        };
        let v = check_thresholds(&m, &t);
        assert!(v.len() >= 4);
        assert!(v.iter().any(|x| x.detail.contains("p99")));
        assert!(v.iter().any(|x| x.detail.contains("error rate")));
        assert!(v.iter().any(|x| x.detail.contains("success rate")));
        assert!(v.iter().any(|x| x.detail.contains("knee")));
    }

    #[test]
    fn markdown_has_verdict_and_rows() {
        let base = RunMetrics::from_summary(&summary(100.0, 100, 0), Some(&knee(700.0)));
        let cand = RunMetrics::from_summary(&summary(40.0, 100, 0), Some(&knee(900.0)));
        let md = compare(&base, &cand).to_markdown();
        assert!(md.contains("IMPROVED"));
        assert!(md.contains("corrected p99 (ms)"));
        assert!(md.contains("knee (req/s)"));
        assert!(md.contains("| metric |"));
    }

    #[test]
    fn thresholds_pass_clean_run() {
        let m = RunMetrics::from_summary(&summary(20.0, 100, 0), Some(&knee(800.0)));
        let t = Thresholds {
            max_corrected_p99_ms: Some(50.0),
            max_error_rate: Some(0.01),
            min_success_rate: Some(0.99),
            min_knee_rps: Some(700.0),
            require_knee: true,
        };
        assert!(check_thresholds(&m, &t).is_empty());
    }
}
