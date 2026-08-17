//! HTTP scenario definitions (pure data — no I/O).

use serde::{Deserialize, Serialize};

/// How arrivals pick work from a scenario file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScenarioMode {
    /// Run every step in order (API journey), honoring `think_ms` between steps.
    #[default]
    Sequence,
    /// Each arrival picks a single step by `weight`.
    Weighted,
}

/// One HTTP request template inside a scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Optional label for reports / debugging.
    #[serde(default)]
    pub name: String,
    /// GET, POST, PUT, PATCH, DELETE, HEAD (case-insensitive). Default GET.
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    /// Relative selection weight when `mode = "weighted"`. Default 1.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Pause after this step before the next (sequence mode only), milliseconds.
    #[serde(default)]
    pub think_ms: u64,
    /// Optional request body (typically with POST/PUT/PATCH).
    #[serde(default)]
    pub body: Option<String>,
    /// Extra headers as key → value.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

fn default_method() -> String {
    "GET".into()
}

fn default_weight() -> u32 {
    1
}

/// A multi-step (or weighted) load scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: ScenarioMode,
    pub steps: Vec<Step>,
}

impl Scenario {
    /// Validate and normalize; returns a human-readable error if unusable.
    pub fn validate(mut self) -> Result<Self, String> {
        if self.steps.is_empty() {
            return Err("scenario must have at least one step".into());
        }
        for (i, step) in self.steps.iter_mut().enumerate() {
            if step.url.trim().is_empty() {
                return Err(format!("step {i}: url is required"));
            }
            if step.weight == 0 {
                step.weight = 1;
            }
            let m = step.method.to_ascii_uppercase();
            match m.as_str() {
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" => step.method = m,
                other => {
                    return Err(format!(
                        "step {i}: unsupported method `{other}` (use GET/POST/PUT/PATCH/DELETE/HEAD)"
                    ));
                }
            }
            if step.name.is_empty() {
                step.name = format!("step-{i}");
            }
        }
        if self.name.is_empty() {
            self.name = "scenario".into();
        }
        Ok(self)
    }

    pub fn label(&self) -> String {
        format!(
            "{} ({} steps, {:?})",
            self.name,
            self.steps.len(),
            self.mode
        )
    }

    /// Total weight across steps (each step at least 1 after validate).
    pub fn total_weight(&self) -> u32 {
        self.steps.iter().map(|s| s.weight.max(1)).sum()
    }

    /// Pick a step by weight using a stable counter (`n` = arrival index).
    pub fn pick_weighted(&self, n: u64) -> &Step {
        let total = self.total_weight().max(1) as u64;
        let mut r = n % total;
        for step in &self.steps {
            let w = step.weight.max(1) as u64;
            if r < w {
                return step;
            }
            r -= w;
        }
        &self.steps[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, weight: u32) -> Step {
        Step {
            name: name.into(),
            method: "GET".into(),
            url: format!("http://example/{name}"),
            weight,
            think_ms: 0,
            body: None,
            headers: Default::default(),
        }
    }

    #[test]
    fn validate_rejects_empty() {
        let s = Scenario {
            name: "x".into(),
            mode: ScenarioMode::Sequence,
            steps: vec![],
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn weighted_pick_respects_weights() {
        let s = Scenario {
            name: "mix".into(),
            mode: ScenarioMode::Weighted,
            steps: vec![step("a", 1), step("b", 3)],
        }
        .validate()
        .unwrap();

        let mut counts = [0u32; 2];
        for n in 0..400 {
            let picked = s.pick_weighted(n);
            if picked.name == "a" {
                counts[0] += 1;
            } else {
                counts[1] += 1;
            }
        }
        // Expect ~1:3 ratio.
        assert!(counts[0] > 80 && counts[0] < 120, "a={}", counts[0]);
        assert!(counts[1] > 280 && counts[1] < 320, "b={}", counts[1]);
    }

    #[test]
    fn sequence_mode_parses_think() {
        let s = Scenario {
            name: String::new(),
            mode: ScenarioMode::Sequence,
            steps: vec![
                Step {
                    name: String::new(),
                    method: "get".into(),
                    url: "http://localhost/".into(),
                    weight: 0,
                    think_ms: 25,
                    body: None,
                    headers: Default::default(),
                },
                step("two", 1),
            ],
        }
        .validate()
        .unwrap();
        assert_eq!(s.name, "scenario");
        assert_eq!(s.steps[0].method, "GET");
        assert_eq!(s.steps[0].name, "step-0");
        assert_eq!(s.steps[0].weight, 1);
        assert_eq!(s.steps[0].think_ms, 25);
    }
}
