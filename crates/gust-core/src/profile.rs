//! Load profiles: how intended request rate evolves over a run.

use std::time::Duration;

/// How Gust schedules open-model arrivals over the run duration.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadProfile {
    /// Fixed RPS for the whole run.
    Constant { rate: f64, duration: Duration },
    /// Linear ramp from `from` RPS to `to` RPS over `duration`.
    Ramp {
        from: f64,
        to: f64,
        duration: Duration,
    },
}

impl LoadProfile {
    /// Intended send rate (req/s) at `elapsed` since run start.
    pub fn rate_at(&self, elapsed: Duration) -> f64 {
        match self {
            Self::Constant { rate, .. } => (*rate).max(0.0),
            Self::Ramp { from, to, duration } => {
                if duration.is_zero() {
                    return (*to).max(0.0);
                }
                let t = (elapsed.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0);
                (from + (to - from) * t).max(0.0)
            }
        }
    }

    /// Total planned run length.
    pub fn duration(&self) -> Duration {
        match self {
            Self::Constant { duration, .. } | Self::Ramp { duration, .. } => *duration,
        }
    }

    /// Rate at t=0.
    pub fn start_rate(&self) -> f64 {
        match self {
            Self::Constant { rate, .. } => *rate,
            Self::Ramp { from, .. } => *from,
        }
    }

    /// Rate at the end of the run.
    pub fn end_rate(&self) -> f64 {
        match self {
            Self::Constant { rate, .. } => *rate,
            Self::Ramp { to, .. } => *to,
        }
    }

    /// Short label for CLI / report headers.
    pub fn label(&self) -> String {
        match self {
            Self::Constant { rate, .. } => format!("constant {rate:.0} req/s"),
            Self::Ramp { from, to, .. } => format!("ramp {from:.0}→{to:.0} req/s"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_rate_is_flat() {
        let p = LoadProfile::Constant {
            rate: 100.0,
            duration: Duration::from_secs(10),
        };
        assert_eq!(p.rate_at(Duration::ZERO), 100.0);
        assert_eq!(p.rate_at(Duration::from_secs(5)), 100.0);
        assert_eq!(p.rate_at(Duration::from_secs(10)), 100.0);
    }

    #[test]
    fn ramp_is_linear() {
        let p = LoadProfile::Ramp {
            from: 0.0,
            to: 100.0,
            duration: Duration::from_secs(10),
        };
        assert!((p.rate_at(Duration::ZERO) - 0.0).abs() < 1e-9);
        assert!((p.rate_at(Duration::from_secs(5)) - 50.0).abs() < 1e-9);
        assert!((p.rate_at(Duration::from_secs(10)) - 100.0).abs() < 1e-9);
        assert!((p.rate_at(Duration::from_secs(20)) - 100.0).abs() < 1e-9);
    }
}
