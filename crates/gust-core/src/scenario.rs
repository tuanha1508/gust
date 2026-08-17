//! HTTP scenario definitions (pure data — no I/O).

use std::collections::BTreeMap;

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

/// Shared credentials applied to every step in a scenario.
///
/// Prefer `bearer`, or `user` + `password` for HTTP Basic. Mixing both is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScenarioAuth {
    /// `Authorization: Bearer …`
    #[serde(default)]
    pub bearer: Option<String>,
    /// Basic-auth username (pair with `password`).
    #[serde(default)]
    pub user: Option<String>,
    /// Basic-auth password.
    #[serde(default)]
    pub password: Option<String>,
}

impl ScenarioAuth {
    pub fn is_empty(&self) -> bool {
        self.bearer.is_none() && self.user.is_none() && self.password.is_none()
    }

    /// Normalize and reject contradictory / incomplete auth.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(tok) = &self.bearer
            && tok.is_empty()
        {
            return Err("auth.bearer must not be empty".into());
        }
        if self.bearer.is_some() && (self.user.is_some() || self.password.is_some()) {
            return Err("auth: use bearer *or* user/password, not both".into());
        }
        match (&self.user, &self.password) {
            (None, None) => Ok(()),
            (Some(u), Some(_)) if u.is_empty() => Err("auth.user must not be empty".into()),
            (Some(_), Some(_)) => Ok(()),
            (Some(_), None) => Err("auth.password is required when auth.user is set".into()),
            (None, Some(_)) => Err("auth.user is required when auth.password is set".into()),
        }
    }
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
    pub headers: BTreeMap<String, String>,
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
    /// Credentials applied to every step unless a step overrides `Authorization`.
    #[serde(default)]
    pub auth: ScenarioAuth,
    /// Seed cookies (`name = value`) sent on every request.
    #[serde(default)]
    pub cookies: BTreeMap<String, String>,
    /// Persist `Set-Cookie` from responses and replay them (login → API journeys).
    #[serde(default)]
    pub cookie_jar: bool,
}

impl Scenario {
    /// Validate and normalize; returns a human-readable error if unusable.
    pub fn validate(mut self) -> Result<Self, String> {
        if self.steps.is_empty() {
            return Err("scenario must have at least one step".into());
        }
        self.auth.validate()?;
        for (name, value) in &self.cookies {
            if name.is_empty() {
                return Err("cookies: name must not be empty".into());
            }
            if value.contains(';') {
                return Err(format!(
                    "cookies.{name}: value must not contain `;` (put attributes in the jar via Set-Cookie)"
                ));
            }
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
            auth: ScenarioAuth::default(),
            cookies: Default::default(),
            cookie_jar: false,
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn weighted_pick_respects_weights() {
        let s = Scenario {
            name: "mix".into(),
            mode: ScenarioMode::Weighted,
            steps: vec![step("a", 1), step("b", 3)],
            auth: ScenarioAuth::default(),
            cookies: Default::default(),
            cookie_jar: false,
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
            auth: ScenarioAuth::default(),
            cookies: Default::default(),
            cookie_jar: false,
        }
        .validate()
        .unwrap();
        assert_eq!(s.name, "scenario");
        assert_eq!(s.steps[0].method, "GET");
        assert_eq!(s.steps[0].name, "step-0");
        assert_eq!(s.steps[0].weight, 1);
        assert_eq!(s.steps[0].think_ms, 25);
    }

    #[test]
    fn auth_rejects_bearer_and_basic_together() {
        let s = Scenario {
            name: "x".into(),
            mode: ScenarioMode::Sequence,
            steps: vec![step("a", 1)],
            auth: ScenarioAuth {
                bearer: Some("tok".into()),
                user: Some("u".into()),
                password: Some("p".into()),
            },
            cookies: Default::default(),
            cookie_jar: false,
        };
        assert!(s.validate().unwrap_err().contains("bearer"));
    }

    #[test]
    fn auth_basic_requires_both_fields() {
        let s = Scenario {
            name: "x".into(),
            mode: ScenarioMode::Sequence,
            steps: vec![step("a", 1)],
            auth: ScenarioAuth {
                bearer: None,
                user: Some("u".into()),
                password: None,
            },
            cookies: Default::default(),
            cookie_jar: false,
        };
        assert!(s.validate().unwrap_err().contains("password"));
    }

    #[test]
    fn cookies_and_auth_round_trip_toml() {
        let raw = r#"
name = "secure"
mode = "sequence"
cookie_jar = true

[auth]
bearer = "sekrit"

[cookies]
session = "abc"

[[steps]]
name = "ping"
url = "http://127.0.0.1:8080/api/me"
"#;
        let s: Scenario = toml::from_str(raw).unwrap();
        let s = s.validate().unwrap();
        assert!(s.cookie_jar);
        assert_eq!(s.auth.bearer.as_deref(), Some("sekrit"));
        assert_eq!(s.cookies.get("session").map(String::as_str), Some("abc"));
    }
}
